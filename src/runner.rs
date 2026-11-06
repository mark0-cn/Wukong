//! The application scheduler owns the agent tree; Rig owns each conversation.
use crate::{
    agents,
    config::AppConfig,
    coordinator::Coordinator,
    domain::*,
    llm::{AgentRunStep, DurableEngine, DurableRun, PendingToolCall},
    store::Store,
    tools::ToolService,
};
use anyhow::{Context, Result, ensure};
use rig::completion::Usage;
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::{sync::broadcast, task::JoinSet};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AssessmentRunner {
    pub store: Store,
    pub coordinator: Coordinator,
    pub tools: Arc<ToolService>,
    config: AppConfig,
    engine: Arc<DurableEngine>,
    events: broadcast::Sender<Value>,
}

impl AssessmentRunner {
    pub async fn new(config: AppConfig, store: Store) -> Result<Self> {
        config.validate()?;
        let engine = Arc::new(DurableEngine::new(config.model.clone())?);
        let tools = Arc::new(ToolService::new(config.clone(), store.clone()).await?);
        let (events, _) = broadcast::channel(1024);
        Ok(Self {
            coordinator: Coordinator::new(store.clone()),
            store,
            tools,
            config,
            engine,
            events,
        })
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }
    fn emit(&self, run_id: &str, agent_id: Option<&str>, kind: &str, data: Value) -> Result<()> {
        let seq = self.store.event(run_id, agent_id, kind, data.clone())?;
        let _ = self
            .events
            .send(json!({"seq":seq,"run_id":run_id,"agent_id":agent_id,"kind":kind,"data":data}));
        Ok(())
    }
    pub async fn drive(&self, run_id: &str, cancel: CancellationToken) -> Result<RunRecord> {
        let owner = id("controller");
        ensure!(
            self.store.acquire_lease(run_id, &owner, 20)?,
            "another controller owns this run; wait for its live lease to expire"
        );
        let result = self.drive_owned(run_id, &owner, cancel).await;
        let _ = self.store.release_lease(run_id, &owner);
        result
    }
    async fn drive_owned(
        &self,
        run_id: &str,
        owner: &str,
        cancel: CancellationToken,
    ) -> Result<RunRecord> {
        let run: RunRecord = self.store.require("run", run_id)?;
        ensure!(!run.status.terminal(), "cannot drive a terminal assessment");
        self.coordinator.resume(run_id, None)?;
        self.emit(run_id, None, "controller.started", json!({"owner":owner}))?;
        let started = std::time::Instant::now();
        let mut tasks: JoinSet<(String, Result<()>)> = JoinSet::new();
        let mut active = HashSet::new();
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        let mut renewed = std::time::Instant::now();
        let mut stopping = false;
        loop {
            if renewed.elapsed() >= Duration::from_secs(5) {
                ensure!(
                    self.store.acquire_lease(run_id, owner, 20)?,
                    "controller lease lost"
                );
                renewed = std::time::Instant::now();
            }
            let run: RunRecord = self.store.require("run", run_id)?;
            if cancel.is_cancelled() && !stopping {
                stopping = true;
                self.coordinator.pause_budget(
                    run_id,
                    "controller interrupted; resume with original config",
                )?;
                self.emit(
                    run_id,
                    None,
                    "controller.stopping",
                    json!({"boundary":"pending durable model/tool effects are retained"}),
                )?;
            }
            if !run.status.terminal() && run.status != Status::BudgetPaused && !stopping {
                if started.elapsed().as_secs() >= run.budget.max_wall_seconds {
                    self.coordinator
                        .pause_budget(run_id, "wall-clock budget reached")?;
                } else {
                    self.coordinator.wake_waiters(run_id)?;
                    let agents = self.store.list::<AgentRecord>("agent", run_id)?;
                    for agent in agents
                        .into_iter()
                        .filter(|a| matches!(a.status, Status::Queued | Status::Running))
                    {
                        if active.len() >= run.budget.max_concurrent_agents {
                            break;
                        }
                        if active.contains(&agent.id) {
                            continue;
                        }
                        self.coordinator
                            .set_status(&agent.id, Status::Running, None)?;
                        active.insert(agent.id.clone());
                        let runner = self.clone();
                        let agent_id = agent.id;
                        let slice_cancel = cancel.clone();
                        let agent_run_id = run.id.clone();
                        tasks.spawn(async move{let result=tokio::select!{
                            biased;
                            _=slice_cancel.cancelled()=>runner.emit(&agent_run_id,Some(&agent_id),"agent.interrupted",json!({"boundary":"prepared model requests and running tool records retained"})),
                            result=runner.slice(&agent_id)=>result,
                        };(agent_id,result)});
                    }
                }
            }
            if active.is_empty() {
                let run: RunRecord = self.store.require("run", run_id)?;
                if run.status.terminal() || run.status == Status::BudgetPaused || stopping {
                    self.emit(
                        run_id,
                        None,
                        "controller.finished",
                        json!({"status":run.status}),
                    )?;
                    return Ok(run);
                }
                let agents = self.store.list::<AgentRecord>("agent", run_id)?;
                if agents.iter().all(|a| a.status.terminal()) {
                    self.store.transaction(|tx|{let mut run:RunRecord=Store::tx_get(tx,"run",run_id)?.context("run missing")?;run.status=Status::Failed;run.summary=Some("All agents terminated without a successful finish_scan. Inspect errors and partial evidence.".into());run.updated_at=now();Store::tx_put(tx,"run",run_id,run_id,&run)})?;
                    continue;
                }
            }
            tokio::select! {
                joined=tasks.join_next(),if !tasks.is_empty()=>{
                    let (agent_id,result)=joined.context("agent task disappeared")?.context("agent task panicked")?;active.remove(&agent_id);
                    if let Err(error)=result {
                        let agent:AgentRecord=self.store.require("agent",&agent_id)?;
                        self.emit(run_id,Some(&agent_id),"agent.error",json!({"message":format!("{error:#}")}))?;
                        if !agent.status.terminal()&&agent.status!=Status::BudgetPaused{self.coordinator.set_status(&agent_id,Status::Failed,Some(format!("{error:#}")))?;}
                    }
                }
                _=ticker.tick()=>{}
                _=cancel.cancelled(),if !stopping=>{}
            }
        }
    }
    async fn slice(&self, agent_id: &str) -> Result<()> {
        let agent: AgentRecord = self.store.require("agent", agent_id)?;
        if agent.status.terminal() || agent.status == Status::BudgetPaused {
            return Ok(());
        }
        let run: RunRecord = self.store.require("run", &agent.run_id)?;
        let mut durable = if let Some(state) = &agent.checkpoint {
            DurableRun::from_json(&serde_json::to_string(state)?)?
        } else {
            let mut history = if let Some(state) = &agent.inherited_context {
                DurableRun::from_json(&serde_json::to_string(state)?)
                    .map(|r| r.history())
                    .unwrap_or_default()
            } else {
                vec![]
            };
            // A parent's current assistant turn can still contain open calls.
            while !history.is_empty() && rig::conversation::pairing::validate(&history).is_err() {
                history.pop();
            }
            let preamble =
                agents::preamble(&agent, &run, &self.store, self.config.skills_dir.as_deref())?;
            self.engine.new_run_with_history(
                &agent.task,
                &preamble,
                self.tools.definitions(agent.role),
                run.budget.max_turns_per_agent,
                history,
            )?
        };
        let mut step = durable.next_step()?;
        if matches!(step, AgentRunStep::CallModel { .. }) {
            if !self.reserve_budget(agent_id)? {
                return Ok(());
            }
            // A queued note belongs to a fresh provider request. Pending tool
            // effects and replayed requests cannot observe newly arrived mail.
            let inbox = if durable.has_prepared_model_call() {
                vec![]
            } else {
                self.store.inbox(agent_id)?
            };
            for message in &inbox {
                durable.queue_note(format!(
                    "from={} message_id={}\n{}",
                    message.sender, message.id, message.content
                ));
            }
            let history_chars = serde_json::to_string(&durable.history())?.len();
            if history_chars > self.config.execution.compact_after_chars {
                let snapshot = serde_json::to_vec(&durable)?;
                let evidence = self.store.artifact(
                    &run.id,
                    agent_id,
                    &format!("compact_{}", durable.turn()),
                    "context.checkpoint",
                    &snapshot,
                    "application/json",
                    json!({"history_chars":history_chars}),
                )?;
                let summary = self.structured_summary(&agent, &run, &evidence.id)?;
                if let Ok(compacted) = durable
                    .compact_before_model(&summary, self.config.execution.keep_recent_messages)
                {
                    step = compacted;
                    self.emit(
                        &run.id,
                        Some(agent_id),
                        "context.compacted",
                        json!({"checkpoint_evidence_id":evidence.id,"history_chars":history_chars}),
                    )?;
                }
            }
            self.engine.prepare_model_call(&mut durable, &step)?;
            self.store.checkpoint_and_ack(
                agent_id,
                serde_json::to_value(&durable)?,
                &inbox.iter().map(|m| m.id).collect::<Vec<_>>(),
            )?;
            self.emit(
                &run.id,
                Some(agent_id),
                "model.started",
                json!({"turn":durable.turn()}),
            )?;
            let mut attempt = 0;
            let usage = loop {
                let result = if self.config.execution.stream {
                    let events = self.events.clone();
                    let rid = run.id.clone();
                    let aid = agent_id.to_owned();
                    self.engine
                        .call_model_streaming(&mut durable, step.clone(), move |_| {
                            let _ = events.send(
                                json!({"run_id":rid,"agent_id":aid,"kind":"model.stream_progress"}),
                            );
                        })
                        .await
                } else {
                    self.engine.call_model(&mut durable, step.clone()).await
                };
                match result {
                    Ok(usage) => break usage,
                    Err(error) => {
                        if attempt >= self.config.execution.model_retries {
                            return Err(error);
                        }
                        attempt += 1;
                        self.emit(
                            &run.id,
                            Some(agent_id),
                            "model.retry",
                            json!({"attempt":attempt,"error":error.to_string()}),
                        )?;
                        tokio::time::sleep(Duration::from_millis(250 * (1 << attempt.min(5))))
                            .await;
                    }
                }
            };
            self.commit_model(agent_id, &durable, usage)?;
            self.emit(
                &run.id,
                Some(agent_id),
                "model.finished",
                json!({"turn":durable.turn(),"usage":usage}),
            )?;
        } else if let AgentRunStep::CallTools { calls } = step {
            // Persist all pending calls before dispatching the first effect.
            self.store
                .checkpoint_and_ack(agent_id, serde_json::to_value(&durable)?, &[])?;
            for call in calls {
                match call {
                    PendingToolCall::Execute(call) => {
                        let call_id = format!("{}:{}:{}", agent_id, durable.turn(), call.id());
                        let name = call.name().to_string();
                        let output = self
                            .tools
                            .execute(agent_id, &call_id, &name, call.arguments())
                            .await?;
                        durable.answer(call, output.value, output.is_error)?;
                        self.store.checkpoint_and_ack(
                            agent_id,
                            serde_json::to_value(&durable)?,
                            &[],
                        )?;
                        self.emit(
                            &run.id,
                            Some(agent_id),
                            "tool.result",
                            json!({"call_id":call_id,"name":name,"is_error":output.is_error}),
                        )?;
                    }
                    PendingToolCall::Malformed(call) => {
                        durable.answer_malformed(call)?;
                        self.store.checkpoint_and_ack(
                            agent_id,
                            serde_json::to_value(&durable)?,
                            &[],
                        )?;
                    }
                    _ => anyhow::bail!("unsupported Rig pending tool call variant"),
                }
            }
        } else if let AgentRunStep::Done(response) = step {
            self.store
                .checkpoint_and_ack(agent_id, serde_json::to_value(&durable)?, &[])?;
            let latest: AgentRecord = self.store.require("agent", agent_id)?;
            if !latest.status.terminal() {
                // Lifecycle is explicit: a free-text answer cannot silently end an assessment.
                anyhow::bail!(
                    "agent returned without lifecycle tool (agent_finish/finish_scan): {}",
                    response.output()
                );
            }
        }
        self.store.transaction(|tx| {
            let mut latest: AgentRecord =
                Store::tx_get(tx, "agent", agent_id)?.context("agent missing")?;
            if latest.status == Status::Running {
                latest.status = Status::Queued;
                latest.updated_at = now();
                Store::tx_put(tx, "agent", agent_id, &latest.run_id, &latest)?;
            }
            Ok(())
        })?;
        Ok(())
    }
    fn reserve_budget(&self, agent_id: &str) -> Result<bool> {
        let agent: AgentRecord = self.store.require("agent", agent_id)?;
        let run: RunRecord = self.store.require("run", &agent.run_id)?;
        let exhausted = run.total_tokens >= run.budget.max_total_tokens
            || run.budget.max_usd.is_some_and(|max| run.cost_usd >= max);
        if exhausted {
            self.coordinator
                .pause_budget(&run.id, "model token or USD budget reached")?;
            return Ok(false);
        }
        Ok(true)
    }
    fn commit_model(&self, agent_id: &str, durable: &DurableRun, usage: Usage) -> Result<()> {
        let tokens = usage.total_tokens.or_else(|| {
            usage
                .input_tokens
                .zip(usage.output_tokens)
                .map(|(i, o)| i.saturating_add(o))
        });
        let cost_known = (self.config.pricing.input_usd_per_million == 0.0
            || usage.input_tokens.is_some())
            && (self.config.pricing.output_usd_per_million == 0.0 || usage.output_tokens.is_some());
        let cost = (usage.input_tokens.unwrap_or(0) as f64
            * self.config.pricing.input_usd_per_million
            + usage.output_tokens.unwrap_or(0) as f64 * self.config.pricing.output_usd_per_million)
            / 1_000_000.0;
        self.store.transaction(|tx|{
            let mut agent:AgentRecord=Store::tx_get(tx,"agent",agent_id)?.context("agent missing")?;let mut run:RunRecord=Store::tx_get(tx,"run",&agent.run_id)?.context("run missing")?;
            agent.checkpoint=Some(serde_json::to_value(durable)?);agent.turns=durable.turn();agent.total_tokens=agent.total_tokens.saturating_add(tokens.unwrap_or(0));if cost_known{agent.cost_usd+=cost;}agent.updated_at=now();
            run.total_tokens=run.total_tokens.saturating_add(tokens.unwrap_or(0));if cost_known{run.cost_usd+=cost;}run.updated_at=now();Store::tx_put(tx,"agent",agent_id,&run.id,&agent)?;Store::tx_put(tx,"run",&run.id,&run.id,&run)?;
            Store::tx_event(tx,&run.id,Some(agent_id),"model.usage",json!({"usage":usage,"cost_estimate_usd":cost_known.then_some(cost),"tokens_reported":tokens.is_some()}))?;
            if tokens.is_none()||!cost_known{Store::tx_event(tx,&run.id,Some(agent_id),"model.usage_unknown",json!({"usage":usage,"tokens_known":tokens.is_some(),"cost_known":cost_known,"notice":"accumulated totals exclude unreported usage; response and pending tools are retained"}))?;}
            Ok(())
        })?;
        if tokens.is_none() || !cost_known {
            let agent: AgentRecord = self.store.require("agent", agent_id)?;
            self.coordinator.pause_budget(&agent.run_id,"provider omitted usage required for token or cost accounting; response retained, totals exclude unknown usage")?;
        }
        Ok(())
    }
    fn structured_summary(
        &self,
        agent: &AgentRecord,
        run: &RunRecord,
        checkpoint_evidence: &str,
    ) -> Result<String> {
        Ok(serde_json::to_string(
            &json!({"notice":"Structured checkpoint summary; older transcript can be retrieved with evidence_read. Preserve unknowns.","full_checkpoint_evidence_id":checkpoint_evidence,"task":agent.task,"scope":agent.scope,"target_sha256":run.target_sha256,"documents":self.store.list::<SharedDocument>("document",&run.id)?,"findings":self.store.list::<Finding>("finding",&run.id)?,"coverage":self.store.list::<Coverage>("coverage",&run.id)?}),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Provider;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    async fn fixture(base: String) -> (tempfile::TempDir, AssessmentRunner, RunRecord) {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.bin");
        std::fs::write(&target, b"scheduler fixture").unwrap();
        let mut config = AppConfig {
            state_dir: dir.path().join("state"),
            ..AppConfig::default()
        };
        config.model.provider = Provider::Ollama;
        config.model.base_url = Some(base);
        config.execution.stream = false;
        config.execution.model_retries = 0;
        let store = Store::open(&config.state_dir).unwrap();
        let runner = AssessmentRunner::new(config.clone(), store).await.unwrap();
        let run = runner
            .coordinator
            .create_run(
                &target,
                "fixture scheduler task",
                config.budget.clone(),
                serde_json::to_value(config).unwrap(),
            )
            .unwrap();
        (dir, runner, run)
    }
    async fn request(socket: &mut TcpStream) -> Value {
        let mut data = Vec::new();
        let end = loop {
            let mut chunk = [0; 4096];
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            data.extend_from_slice(&chunk[..n]);
            if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
        };
        let headers = String::from_utf8_lossy(&data[..end]);
        let length = headers
            .lines()
            .find_map(|l| {
                l.split_once(':')
                    .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while data.len() < end + length {
            let mut chunk = [0; 8192];
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            data.extend_from_slice(&chunk[..n]);
        }
        serde_json::from_slice(&data[end..end + length]).unwrap()
    }
    fn reply(name: &str, args: Value) -> Value {
        json!({"model":"fixture","created_at":"2026-11-06T00:00:00Z","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":name,"arguments":args}}]},"done":true,"done_reason":"stop","prompt_eval_count":10,"eval_count":2})
    }
    async fn respond(socket: &mut TcpStream, value: Value) {
        let body = value.to_string();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn late_mail_is_unread_until_a_fresh_model_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                requests.push(request(&mut socket).await);
                respond(
                    &mut socket,
                    reply("finish_scan", json!({"summary":"fixture complete"})),
                )
                .await;
            }
            requests
        });
        let (_dir, runner, run) = fixture(base).await;
        let mut prepared = runner
            .engine
            .new_run(
                &run.task,
                "fixture root",
                runner.tools.definitions(Role::Root),
                3,
            )
            .unwrap();
        let step = prepared.next_step().unwrap();
        runner
            .engine
            .prepare_model_call(&mut prepared, &step)
            .unwrap();
        runner
            .store
            .checkpoint_and_ack(&run.root_id, serde_json::to_value(prepared).unwrap(), &[])
            .unwrap();
        runner
            .coordinator
            .message(
                "operator",
                &run.root_id,
                "Message arriving after the request was prepared",
            )
            .unwrap();
        runner.slice(&run.root_id).await.unwrap();
        assert_eq!(
            runner.store.inbox(&run.root_id).unwrap().len(),
            1,
            "prepared request replay acknowledged unseen mail"
        );
        runner
            .coordinator
            .message(
                "operator",
                &run.root_id,
                "Review this late operator instruction before finishing",
            )
            .unwrap();
        runner.slice(&run.root_id).await.unwrap();
        assert_eq!(runner.store.inbox(&run.root_id).unwrap().len(), 2);
        assert!(
            !runner
                .store
                .require::<AgentRecord>("agent", &run.root_id)
                .unwrap()
                .status
                .terminal()
        );
        runner.slice(&run.root_id).await.unwrap();
        assert!(runner.store.inbox(&run.root_id).unwrap().is_empty());
        runner.slice(&run.root_id).await.unwrap();
        assert_eq!(
            runner
                .store
                .require::<RunRecord>("run", &run.id)
                .unwrap()
                .status,
            Status::Completed
        );
        let requests = server.await.unwrap();
        assert!(
            !requests[0]
                .to_string()
                .contains("Message arriving after the request was prepared")
        );
        assert!(
            requests[1]
                .to_string()
                .contains("Message arriving after the request was prepared")
        );
        assert!(
            requests[1]
                .to_string()
                .contains("Review this late operator instruction before finishing")
        );
        runner.tools.close().await;
    }

    #[tokio::test]
    async fn cancellation_returns_promptly_and_replays_the_prepared_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let before = request(&mut first).await;
            entered.send(()).unwrap();
            let (mut second, _) = listener.accept().await.unwrap();
            let after = request(&mut second).await;
            respond(
                &mut second,
                reply(
                    "finish_scan",
                    json!({"summary":"fixture complete after resume"}),
                ),
            )
            .await;
            (before, after)
        });
        let (_dir, runner, run) = fixture(base).await;
        let cancel = CancellationToken::new();
        let task_runner = runner.clone();
        let run_id = run.id.clone();
        let task_cancel = cancel.clone();
        let driving = tokio::spawn(async move { task_runner.drive(&run_id, task_cancel).await });
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap();
        cancel.cancel();
        let paused = tokio::time::timeout(Duration::from_secs(2), driving)
            .await
            .expect("cancellation waited for the HTTP timeout")
            .unwrap()
            .unwrap();
        assert_eq!(paused.status, Status::BudgetPaused);
        let agent: AgentRecord = runner.store.require("agent", &run.root_id).unwrap();
        let durable = DurableRun::from_json(&agent.checkpoint.unwrap().to_string()).unwrap();
        assert!(durable.has_prepared_model_call());
        assert!(
            runner
                .store
                .acquire_lease(&run.id, "check-release", 20)
                .unwrap()
        );
        runner
            .store
            .release_lease(&run.id, "check-release")
            .unwrap();
        let completed = tokio::time::timeout(
            Duration::from_secs(5),
            runner.drive(&run.id, CancellationToken::new()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(completed.status, Status::Completed);
        assert_eq!(completed.total_tokens, 12);
        let (before, after) = server.await.unwrap();
        assert_eq!(
            before, after,
            "resume changed the prepared provider request"
        );
        runner.tools.close().await;
    }

    #[tokio::test]
    async fn missing_usage_pauses_after_preserving_the_response_checkpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            respond(
                &mut socket,
                reply("finish_scan", json!({"summary":"retained model response"})),
            )
            .await;
        });
        let (_dir, runner, run) = fixture(base).await;
        let mut durable = runner
            .engine
            .new_run(
                "fixture",
                "fixture",
                runner.tools.definitions(Role::Root),
                2,
            )
            .unwrap();
        let step = durable.next_step().unwrap();
        runner.engine.call_model(&mut durable, step).await.unwrap();
        runner
            .commit_model(&run.root_id, &durable, Usage::default())
            .unwrap();
        assert_eq!(
            runner
                .store
                .require::<RunRecord>("run", &run.id)
                .unwrap()
                .status,
            Status::BudgetPaused
        );
        let agent: AgentRecord = runner.store.require("agent", &run.root_id).unwrap();
        let mut retained = DurableRun::from_json(&agent.checkpoint.unwrap().to_string()).unwrap();
        assert!(matches!(
            retained.next_step().unwrap(),
            AgentRunStep::CallTools { .. }
        ));
        assert_eq!(agent.status, Status::BudgetPaused);
        let events = runner.store.events(&run.id, 0, 100).unwrap();
        let event = events
            .iter()
            .find(|event| event.kind == "model.usage_unknown")
            .unwrap();
        assert_eq!(event.data["tokens_known"], false);
        server.await.unwrap();
        runner.tools.close().await;
    }
}
