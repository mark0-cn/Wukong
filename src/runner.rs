//! Application facade for the native Rig runtime and business accounting.
use crate::{
    config::AppConfig, coordinator::Coordinator, domain::*, llm::ModelEngine, store::Store,
    tools::ToolService,
};
use anyhow::{Context, Result, ensure};
use rig::completion::Usage;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AssessmentRunner {
    pub store: Store,
    pub coordinator: Coordinator,
    pub tools: Arc<ToolService>,
    config: AppConfig,
    engine: Arc<ModelEngine>,
    /// Engines for role (non-root) agents, keyed by resolved model name
    /// (`model.role_models` entries plus the legacy `model.role_model`).
    role_engines: HashMap<String, Arc<ModelEngine>>,
    events: broadcast::Sender<Value>,
}

impl AssessmentRunner {
    pub async fn new(mut config: AppConfig, store: Store) -> Result<Self> {
        config.validate()?;
        let engine = Arc::new(ModelEngine::new(config.model.clone())?);
        config.model = engine.resolved_config().clone();
        let mut explicit: HashMap<String, Option<u64>> = HashMap::new();
        if let Some(map) = &config.model.role_models {
            for spec in map.values() {
                explicit.insert(spec.model.clone(), spec.max_tokens);
            }
        }
        if let Some(name) = &config.model.role_model {
            explicit.entry(name.clone()).or_insert(None);
        }
        let mut role_engines = HashMap::new();
        for (name, spec_max) in explicit {
            if role_engines.contains_key(&name) {
                continue;
            }
            let mut role = config.model.clone();
            role.role_model = None;
            role.role_models = None;
            role.model = name.clone();
            let max_tokens = spec_max.or(config.model.role_max_tokens);
            if let Some(max_tokens) = max_tokens {
                role.max_tokens = max_tokens;
            }
            role_engines.insert(name, Arc::new(ModelEngine::new(role)?));
        }
        let tools = Arc::new(ToolService::new(config.clone(), store.clone()).await?);
        let (events, _) = broadcast::channel(1024);
        Ok(Self {
            coordinator: Coordinator::new(store.clone()),
            store,
            tools,
            config,
            engine,
            role_engines,
            events,
        })
    }
    /// The engine that serves an agent: role agents use their resolved role
    /// model (same provider/endpoint, different model name) when configured.
    /// A role explicitly mapped to the root model name gets a dedicated
    /// engine carrying `role_max_tokens`.
    pub(crate) fn engine_for(&self, role: Role) -> &ModelEngine {
        if let Some(name) = self.config.model.role_model_name(role)
            && let Some(engine) = self.role_engines.get(name)
        {
            return engine;
        }
        &self.engine
    }
    pub fn config(&self) -> &AppConfig {
        &self.config
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }
    pub(crate) fn stream_progress(&self, run_id: &str, agent_id: &str) {
        let _ = self
            .events
            .send(json!({"run_id":run_id,"agent_id":agent_id,"kind":"model.stream_progress"}));
    }
    pub(crate) fn emit(
        &self,
        run_id: &str,
        agent_id: Option<&str>,
        kind: &str,
        data: Value,
    ) -> Result<()> {
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
        let runner = self.clone();
        let run_id = run_id.to_owned();
        let owner = owner.to_owned();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            crate::kernel::drive(runner, &run_id, &owner, cancel, runtime)
        })
        .await
        .context("Rig kernel thread failed")?
    }

    /// Project a native failure into the task ledger and the existing UI event stream.
    pub(crate) fn fail_agent(
        &self,
        agent_id: &str,
        reason: String,
        summary: Option<String>,
    ) -> Result<()> {
        let run_id = self.store.transaction(|tx| {
            let mut target: AgentRecord =
                Store::tx_get(tx, "agent", agent_id)?.context("agent missing")?;
            if target.status.terminal() {
                return Ok(None);
            }
            target.status = Status::Failed;
            target.error = Some(reason.clone());
            if let Some(summary) = &summary {
                target.summary = Some(summary.clone());
            }
            target.updated_at = now();
            Store::tx_put(tx, "agent", agent_id, &target.run_id, &target)?;
            Coordinator::notify_terminal(tx, &target)?;
            Ok(Some(target.run_id))
        })?;
        if let Some(run_id) = run_id {
            self.emit(
                &run_id,
                Some(agent_id),
                "agent.error",
                json!({"message":reason}),
            )?;
        }
        Ok(())
    }

    pub(crate) fn structured_summary(
        &self,
        agent: &AgentRecord,
        run: &RunRecord,
        checkpoint_evidence: &str,
    ) -> Result<String> {
        self.store.transaction(|tx| {
            Ok(serde_json::to_string(&json!({
                "notice":"Shared analysis checkpoint; retrieve the older transcript with evidence_read. Shared context is refreshed on each request. Use context_search/context_read, view_agent_graph, findings_list and coverage_list for omitted ledger entries. Preserve unknowns.",
                "full_checkpoint_evidence_id":checkpoint_evidence,
                "task":agent.task,"scope":agent.scope,"target_sha256":run.target_sha256,
                "shared_context":crate::context::snapshot(tx,agent)?,
                "ledger_counts":{
                    "tasks":Store::tx_list::<AgentRecord>(tx,"agent",&run.id)?.len(),
                    "findings":Store::tx_list::<Finding>(tx,"finding",&run.id)?.len(),
                    "coverage":Store::tx_list::<Coverage>(tx,"coverage",&run.id)?.len()
                }
            }))?)
        })
    }

    /// Account auxiliary model usage (e.g. tool-output summarization) in the
    /// agent and run totals without touching the conversation checkpoint.
    pub(crate) fn account_usage(
        &self,
        agent_id: &str,
        usage: Usage,
        model_turn: bool,
    ) -> Result<()> {
        let tokens = usage.total_tokens.or_else(|| {
            usage
                .input_tokens
                .zip(usage.output_tokens)
                .map(|(i, o)| i.saturating_add(o))
        });
        // The configured rates are flat estimates for all input/output.
        // Rig owns normalized cache counters and avoids double counting them.
        let pricing = rig_core::catalog::Pricing::new(
            self.config.pricing.input_usd_per_million,
            self.config.pricing.output_usd_per_million,
        )
        .with_cache_read(self.config.pricing.input_usd_per_million)
        .with_cache_write(self.config.pricing.input_usd_per_million);
        let mut priced_usage = usage;
        if pricing.input == 0.0 {
            priced_usage.input_tokens.get_or_insert(0);
        }
        if pricing.output == 0.0 {
            priced_usage.output_tokens.get_or_insert(0);
        }
        let estimate = pricing.cost(&priced_usage);
        let cost_known = estimate.as_ref().is_some_and(|cost| cost.is_complete());
        let cost = estimate.as_ref().map_or(0.0, |cost| cost.total);
        self.store.transaction(|tx| {
            let mut agent: AgentRecord = Store::tx_get(tx,"agent",agent_id)?.context("agent missing")?;
            let mut run: RunRecord = Store::tx_get(tx,"run",&agent.run_id)?.context("run missing")?;
            agent.turns += usize::from(model_turn);
            agent.total_tokens = agent.total_tokens.saturating_add(tokens.unwrap_or(0));
            run.total_tokens = run.total_tokens.saturating_add(tokens.unwrap_or(0));
            if cost_known { agent.cost_usd += cost; run.cost_usd += cost; }
            agent.updated_at = now(); run.updated_at = now();
            Store::tx_put(tx,"agent",agent_id,&run.id,&agent)?;
            Store::tx_put(tx,"run",&run.id,&run.id,&run)?;
            Store::tx_event(tx,&run.id,Some(agent_id),if model_turn {"model.usage"} else {"model.usage_aux"},json!({"usage":usage,"cost_estimate_usd":cost_known.then_some(cost),"tokens_reported":tokens.is_some()}))?;
            Ok(())
        })?;
        if tokens.is_none() || !cost_known {
            let agent: AgentRecord = self.store.require("agent", agent_id)?;
            self.coordinator.pause_budget(
                &agent.run_id,
                "provider omitted usage required for accounting; response retained in Rig journal",
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Provider;
    use std::time::Duration;
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
    fn reply_text(content: &str) -> Value {
        json!({"model":"fixture","created_at":"2026-11-06T00:00:00Z","message":{"role":"assistant","content":content},"done":true,"done_reason":"stop","prompt_eval_count":10,"eval_count":2})
    }
    async fn respond(socket: &mut TcpStream, value: Value) {
        let body = value.to_string();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn native_kernel_accepts_gated_completion_without_an_extra_provider_call() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let input = request(&mut socket).await;
            respond(
                &mut socket,
                reply("finish_scan", json!({"summary":"native completion"})),
            )
            .await;
            input
        });
        let (_dir, runner, run) = fixture(base).await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            runner.drive(&run.id, CancellationToken::new()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.status, Status::Completed);
        assert_eq!(result.total_tokens, 12);
        let request = server.await.unwrap();
        assert!(request.to_string().contains("task"));
        assert!(
            !request["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["function"]["name"] == "binary_inspect")
        );
    }

    async fn drive_bounded(runner: &AssessmentRunner, run_id: &str) -> RunRecord {
        let cancel = CancellationToken::new();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            runner.drive(run_id, cancel.clone()),
        )
        .await;
        if result.is_err() {
            cancel.cancel();
        }
        result.expect("native runtime timed out").unwrap()
    }

    #[tokio::test]
    async fn native_lifecycle_rejects_a_second_plain_answer() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (dir, runner, run) =
            fixture(format!("http://{}", listener.local_addr().unwrap())).await;
        let server = tokio::spawn(async move {
            for text in ["partial observations", "still no accepted result"] {
                let (mut socket, _) = listener.accept().await.unwrap();
                request(&mut socket).await;
                respond(&mut socket, reply_text(text)).await;
            }
        });
        let result = drive_bounded(&runner, &run.id).await;
        assert_eq!(result.status, Status::Failed);
        let root: AgentRecord = runner.store.require("agent", &run.root_id).unwrap();
        assert!(root.error.unwrap().contains("lifecycle"));
        assert_eq!(root.summary.as_deref(), Some("still no accepted result"));
        assert_eq!(root.total_tokens, 24);
        server.await.unwrap();
        drop(dir);
    }

    #[tokio::test]
    async fn native_unknown_usage_preserves_the_response_without_tool_io() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (_dir, runner, run) =
            fixture(format!("http://{}", listener.local_addr().unwrap())).await;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            let mut output = reply("finish_scan", json!({"summary":"pending completion"}));
            output.as_object_mut().unwrap().remove("prompt_eval_count");
            output.as_object_mut().unwrap().remove("eval_count");
            respond(&mut socket, output).await;
        });
        assert_eq!(
            drive_bounded(&runner, &run.id).await.status,
            Status::BudgetPaused
        );
        assert!(
            runner
                .store
                .list::<ToolCallRecord>("call", &run.id)
                .unwrap()
                .is_empty()
        );
        let root: AgentRecord = runner.store.require("agent", &run.root_id).unwrap();
        assert!(
            root.checkpoint
                .unwrap()
                .to_string()
                .contains("pending completion")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn native_skills_and_usage_survive_rig_memory_compaction() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (_dir, mut runner, run) =
            fixture(format!("http://{}", listener.local_addr().unwrap())).await;
        runner.config.execution.compact_after_chars = 100;
        runner.config.execution.keep_recent_messages = 2;
        let note = SharedDocument {
            id: "compaction-shared-note".into(),
            run_id: run.id.clone(),
            agent_id: run.root_id.clone(),
            kind: "note".into(),
            title: "format dossier".into(),
            body: format!(
                "{} DO_NOT_INJECT_FULL_LEDGER",
                "bounded format preview ".repeat(2000)
            ),
            amendments: vec![],
            created_at: now(),
            updated_at: now(),
        };
        runner.store.put("note", &note.id, &run.id, &note).unwrap();
        let server = tokio::spawn(async move {
            let mut inputs = vec![];
            for (name, args) in [
                ("load_skill", json!({"name":"formats"})),
                (
                    "think",
                    json!({"content":"Format facts are linked through shared evidence; preserve unknowns."}),
                ),
                ("finish_scan", json!({"summary":"format analysis complete"})),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                inputs.push(request(&mut socket).await);
                respond(&mut socket, reply(name, args)).await;
            }
            inputs
        });
        let result = drive_bounded(&runner, &run.id).await;
        assert_eq!(result.status, Status::Completed);
        assert_eq!(result.total_tokens, 36);
        let inputs = server.await.unwrap();
        for input in &inputs {
            let text = input.to_string();
            assert!(text.contains("compaction-shared-note"));
            assert!(!text.contains("DO_NOT_INJECT_FULL_LEDGER"));
        }
        for input in &inputs[1..] {
            assert!(
                input
                    .to_string()
                    .contains("Active controller skill: formats")
            );
        }
        let root: AgentRecord = runner.store.require("agent", &run.root_id).unwrap();
        let history: rig_core::conversation::Conversation =
            serde_json::from_value(root.checkpoint.unwrap()).unwrap();
        history.validate().unwrap();
        assert!(
            history
                .asides()
                .any(|(_, aside)| aside.kind == rig::memory::COMPACTED)
        );
        let events = runner.store.events(&run.id, 0, 1000).unwrap();
        let event = events
            .iter()
            .find(|event| event.kind == "context.compacted")
            .unwrap();
        runner
            .store
            .verify_evidence_links(
                &run.id,
                &[event.data["checkpoint_evidence_id"]
                    .as_str()
                    .unwrap()
                    .into()],
            )
            .unwrap();
    }

    #[tokio::test]
    async fn native_retry_uses_rig_error_classification_and_retry_after() {
        for (status, expected) in [(401, Status::Failed), (429, Status::Completed)] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let (_dir, mut runner, run) =
                fixture(format!("http://{}", listener.local_addr().unwrap())).await;
            runner.config.execution.model_retries = 1;
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                request(&mut socket).await;
                let body = r#"{"error":"fixture provider rejected request"}"#;
                socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nRetry-After: 0\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
                if status == 429 {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    request(&mut socket).await;
                    respond(
                        &mut socket,
                        reply("finish_scan", json!({"summary":"retry succeeded"})),
                    )
                    .await;
                }
            });
            assert_eq!(drive_bounded(&runner, &run.id).await.status, expected);
            server.await.unwrap();
            if status == 401 {
                assert!(
                    runner
                        .store
                        .events(&run.id, 0, 100)
                        .unwrap()
                        .iter()
                        .any(|event| event.kind == "agent.error"
                            && event.data["message"]
                                .as_str()
                                .is_some_and(|text| text.contains("401"))),
                    "native failure was not projected to the UI event stream"
                );
                assert_eq!(
                    runner
                        .store
                        .require::<RunRecord>("run", &run.id)
                        .unwrap()
                        .total_tokens,
                    0
                );
            }
        }
    }

    #[tokio::test]
    async fn native_messages_do_not_reopen_work_and_followups_keep_history() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (_dir, mut runner, run) =
            fixture(format!("http://{}", listener.local_addr().unwrap())).await;
        let mut config = runner.config.clone();
        config.model.role_models = Some(HashMap::from([
            (
                Role::Recon,
                crate::llm::RoleModelSpec {
                    model: "role-fixture".into(),
                    max_tokens: Some(128),
                },
            ),
            (
                Role::Reverse,
                crate::llm::RoleModelSpec {
                    model: "role-fixture".into(),
                    max_tokens: Some(256),
                },
            ),
        ]));
        runner = AssessmentRunner::new(config, runner.store.clone())
            .await
            .unwrap();
        let store = runner.store.clone();
        let rid = run.id.clone();
        let server = tokio::spawn(async move {
            let mut roots = 0;
            let mut children = 0;
            while roots < 6 || children < 2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let input = request(&mut socket).await;
                let root = input.to_string().contains("role=root");
                let output = if root {
                    assert_eq!(input["model"], "gpt-5.2");
                    let child = || {
                        store
                            .list::<AgentRecord>("agent", &rid)
                            .unwrap()
                            .into_iter()
                            .find(|agent| agent.role == Role::Reverse)
                            .unwrap()
                    };
                    let output = match roots {
                        0 => reply(
                            "task",
                            json!({"description":"format analyst","prompt":"Explain the format","role":"reverse","scope":"input format","inherit_context":false}),
                        ),
                        1 => reply("wait", json!({"agent":child().id})),
                        2 => reply(
                            "send_message_to_agent",
                            json!({"agent_id":child().id,"message":"For reference only","kind":"information"}),
                        ),
                        3 => {
                            assert_eq!(children, 1, "information reopened the child");
                            assert_eq!(child().status, Status::Completed);
                            reply(
                                "follow_up_agent",
                                json!({"agent_id":child().id,"message":"Clarify the length field","recipient_generation":0}),
                            )
                        }
                        4 => reply("wait", json!({"agent":child().id})),
                        5 => reply("finish_scan", json!({"summary":"two accepted results"})),
                        _ => panic!("unexpected root call {roots}"),
                    };
                    roots += 1;
                    output
                } else {
                    assert_eq!(input["model"], "role-fixture");
                    assert_eq!(input["options"]["num_predict"], 256);
                    if children == 1 {
                        assert!(input.to_string().contains("first format result"));
                        assert!(input.to_string().contains("Clarify the length field"));
                        assert!(
                            !input.to_string().contains("For reference only"),
                            "stale informational mail was delivered into the new generation"
                        );
                    }
                    let output = reply(
                        "agent_finish",
                        json!({"summary":if children==0 {"first format result"} else {"clarified result"}}),
                    );
                    children += 1;
                    output
                };
                respond(&mut socket, output).await;
            }
        });
        assert_eq!(
            drive_bounded(&runner, &run.id).await.status,
            Status::Completed
        );
        server.await.unwrap();
        let child = runner
            .store
            .list::<AgentRecord>("agent", &run.id)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == Role::Reverse)
            .unwrap();
        assert_eq!(child.generation, 1);
        assert_eq!(child.summary.as_deref(), Some("clarified result"));
    }
    #[tokio::test]
    async fn native_streams_keep_reasoning_usage_and_ui_progress() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (_dir, mut runner, run) =
            fixture(format!("http://{}", listener.local_addr().unwrap())).await;
        runner.config.execution.stream = true;
        let mut events = runner.subscribe();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut socket).await["stream"], true);
            let parts = [
                json!({"model":"fixture","created_at":"2026-11-06T00:00:00Z","message":{"role":"assistant","thinking":"check the evidence"},"done":false}),
                json!({"model":"fixture","created_at":"2026-11-06T00:00:00Z","message":{"role":"assistant","tool_calls":[{"function":{"name":"finish_scan","arguments":{"summary":"streamed completion"}}}]},"done":false}),
                json!({"model":"fixture","created_at":"2026-11-06T00:00:00Z","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":10,"eval_count":2}),
            ];
            let body = parts
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        });
        assert_eq!(
            drive_bounded(&runner, &run.id).await.status,
            Status::Completed
        );
        assert_eq!(
            runner
                .store
                .require::<RunRecord>("run", &run.id)
                .unwrap()
                .total_tokens,
            12
        );
        let checkpoint = runner
            .store
            .require::<AgentRecord>("agent", &run.root_id)
            .unwrap()
            .checkpoint
            .unwrap();
        assert!(checkpoint.to_string().contains("check the evidence"));
        let mut progress = false;
        while let Ok(event) = events.try_recv() {
            progress |= event["kind"] == "model.stream_progress";
        }
        assert!(progress);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn native_provider_reasoning_replays_through_the_real_kernel() {
        let anthropic = [
            json!({"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"thinking","thinking":"inspect the input parser","signature":"provider-signed-thinking"},{"type":"tool_use","id":"toolu_1","name":"view_agent_graph","input":{}}],"stop_reason":"tool_use","stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":3}}),
            json!({"id":"msg_2","type":"message","role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"tool_use","id":"toolu_2","name":"finish_scan","input":{"summary":"native provider result"}}],"stop_reason":"tool_use","stop_sequence":null,"usage":{"input_tokens":12,"output_tokens":2}}),
        ];
        let responses = [
            json!({"id":"resp_1","object":"response","created_at":1,"status":"completed","model":"gpt-5.2","output":[{"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"inspect parser"}],"encrypted_content":"opaque-native-reasoning"},{"type":"function_call","id":"fc_1","call_id":"call_1","name":"view_agent_graph","arguments":"{}","status":"completed"}],"usage":{"input_tokens":10,"output_tokens":4,"total_tokens":14},"tools":[]}),
            json!({"id":"resp_2","object":"response","created_at":1,"status":"completed","model":"gpt-5.2","output":[{"type":"function_call","id":"fc_2","call_id":"call_2","name":"finish_scan","arguments":"{\"summary\":\"native provider result\"}","status":"completed"}],"usage":{"input_tokens":14,"output_tokens":2,"total_tokens":16},"tools":[]}),
        ];
        for (provider, model, outputs, marker) in [
            (
                Provider::Anthropic,
                "claude-sonnet-4-5",
                anthropic,
                "provider-signed-thinking",
            ),
            (
                Provider::OpenAiResponses,
                "gpt-5.2",
                responses,
                "opaque-native-reasoning",
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (_dir, mut runner, run) = fixture(base.clone()).await;
            runner.config.model.provider = provider;
            runner.config.model.model = model.into();
            runner.config.model.base_url = Some(base);
            runner.engine = Arc::new(
                ModelEngine::connect(runner.config.model.clone(), "fixture-not-a-secret".into())
                    .unwrap(),
            );
            let server = tokio::spawn(async move {
                let mut inputs = vec![];
                for output in outputs {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    inputs.push(request(&mut socket).await);
                    respond(&mut socket, output).await;
                }
                inputs
            });
            assert_eq!(
                drive_bounded(&runner, &run.id).await.status,
                Status::Completed
            );
            let inputs = server.await.unwrap();
            assert!(
                inputs[1].to_string().contains(marker),
                "native reasoning lost: {}",
                inputs[1]
            );
            assert!(
                !runner
                    .store
                    .require::<AgentRecord>("agent", &run.root_id)
                    .unwrap()
                    .checkpoint
                    .unwrap()
                    .to_string()
                    .contains("fixture-not-a-secret")
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn native_tasks_return_before_predecessors_finish_but_models_wait() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (_dir, runner, run) =
            fixture(format!("http://{}", listener.local_addr().unwrap())).await;
        let store = runner.store.clone();
        let rid = run.id.clone();
        let ready = Arc::new(tokio::sync::Notify::new());
        let server = tokio::spawn(async move {
            let mut handlers = vec![];
            let mut roots = 0;
            // Four root calls and one model call per child.
            for _ in 0..6 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let input = request(&mut socket).await;
                let text = input.to_string();
                let output = if text.contains("role=root") {
                    let tasks = store.list::<AgentRecord>("agent", &rid).unwrap();
                    let first = tasks.iter().find(|agent| agent.scope == "first");
                    let output = match roots {
                        0 => reply(
                            "task",
                            json!({"description":"first","prompt":"First task","scope":"first","role":"reverse","inherit_context":false}),
                        ),
                        1 => reply(
                            "task",
                            json!({"description":"second","prompt":"Second task","scope":"second","role":"reverse","inherit_context":false,"wait_for_predecessors":[first.unwrap().id]}),
                        ),
                        2 => {
                            assert!(
                                !first.unwrap().status.terminal(),
                                "task waited synchronously for its predecessor"
                            );
                            ready.notify_one();
                            reply(
                                "wait",
                                json!({"agent":tasks.iter().find(|agent|agent.scope=="second").unwrap().id}),
                            )
                        }
                        _ => reply("finish_scan", json!({"summary":"ordered tasks completed"})),
                    };
                    roots += 1;
                    Some(output)
                } else if text.contains("Scope: first.") {
                    let ready = ready.clone();
                    handlers.push(tokio::spawn(async move {
                        ready.notified().await;
                        respond(
                            &mut socket,
                            reply("agent_finish", json!({"summary":"predecessor facts"})),
                        )
                        .await;
                    }));
                    continue;
                } else {
                    let first = store
                        .list::<AgentRecord>("agent", &rid)
                        .unwrap()
                        .into_iter()
                        .find(|agent| agent.scope == "first")
                        .unwrap();
                    assert_eq!(
                        first.status,
                        Status::Completed,
                        "successor called its model too early"
                    );
                    assert!(
                        text.contains("predecessor facts"),
                        "successor did not get the dependency result"
                    );
                    Some(reply("agent_finish", json!({"summary":"successor result"})))
                };
                if let Some(output) = output {
                    respond(&mut socket, output).await;
                }
            }
            for handler in handlers {
                handler.await.unwrap();
            }
        });
        let result = drive_bounded(&runner, &run.id).await;
        assert_eq!(result.status, Status::Completed);
        server.await.unwrap();
    }
    #[tokio::test]
    async fn reported_token_totals_do_not_hide_unknown_costs() {
        let (_dir, mut runner, run) = fixture("http://127.0.0.1:1".into()).await;
        runner.config.pricing.input_usd_per_million = 10.0;
        runner.config.pricing.output_usd_per_million = 20.0;
        let mut usage = Usage::default();
        usage.total_tokens = Some(12);
        runner.account_usage(&run.root_id, usage, true).unwrap();
        let paused: RunRecord = runner.store.require("run", &run.id).unwrap();
        assert_eq!(paused.status, Status::BudgetPaused);
        assert_eq!(paused.total_tokens, 12);
        assert_eq!(paused.cost_usd, 0.0);
        let usage = runner
            .store
            .events(&run.id, 0, 100)
            .unwrap()
            .into_iter()
            .find(|event| event.kind == "model.usage")
            .unwrap();
        assert!(usage.data["cost_estimate_usd"].is_null());
        assert_eq!(usage.data["tokens_reported"], true);
    }
    #[tokio::test]
    async fn informational_mail_from_the_awaited_agent_does_not_release_native_wait() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (_dir, runner, run) =
            fixture(format!("http://{}", listener.local_addr().unwrap())).await;
        let store = runner.store.clone();
        let rid = run.id.clone();
        let root_id = run.root_id.clone();
        let server = tokio::spawn(async move {
            let mut roots = 0;
            let mut children = 0;
            let mut pending_child = None;
            while roots < 2 || children < 2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let input = request(&mut socket).await;
                if input.to_string().contains("role=root") {
                    let output = if roots == 0 {
                        reply(
                            "task",
                            json!({"description":"worker","prompt":"Analyze the format","scope":"format","role":"reverse","inherit_context":false}),
                        )
                    } else {
                        let child = store
                            .list::<AgentRecord>("agent", &rid)
                            .unwrap()
                            .into_iter()
                            .find(|agent| agent.role == Role::Reverse)
                            .unwrap();
                        reply("wait", json!({"agent":child.id}))
                    };
                    roots += 1;
                    respond(&mut socket, output).await;
                } else {
                    if children == 0 {
                        respond(&mut socket,reply("send_message_to_agent",json!({"agent_id":root_id,"message":"progress only; task is still running","kind":"information"}))).await;
                    } else {
                        pending_child = Some(socket);
                    }
                    children += 1;
                }
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err(),
                "information incorrectly woke the native waiter"
            );
            respond(
                pending_child.as_mut().unwrap(),
                reply(
                    "agent_finish",
                    json!({"summary":"finished after information"}),
                ),
            )
            .await;
            let (mut socket, _) = listener.accept().await.unwrap();
            let input = request(&mut socket).await;
            assert!(
                input
                    .to_string()
                    .contains("progress only; task is still running")
            );
            assert!(input.to_string().contains("finished after information"));
            respond(
                &mut socket,
                reply(
                    "finish_scan",
                    json!({"summary":"wait observed actual completion"}),
                ),
            )
            .await;
        });
        assert_eq!(
            drive_bounded(&runner, &run.id).await.status,
            Status::Completed
        );
        server.await.unwrap();
    }
    #[tokio::test]
    async fn following_up_a_grandchild_reactivates_and_reports_its_parent() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (_dir, runner, run) =
            fixture(format!("http://{}", listener.local_addr().unwrap())).await;
        let store = runner.store.clone();
        let rid = run.id.clone();
        let server = tokio::spawn(async move {
            let mut roots = 0;
            let mut parents = 0;
            let mut leaves = 0;
            for _ in 0..12 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let input = request(&mut socket).await;
                let text = input.to_string();
                let lookup = |scope: &str| {
                    store
                        .list::<AgentRecord>("agent", &rid)
                        .unwrap()
                        .into_iter()
                        .find(|agent| agent.scope == scope)
                        .unwrap()
                };
                let output = if text.contains("role=root") {
                    let output = match roots {
                        0 => reply(
                            "task",
                            json!({"description":"parent","prompt":"Parent objective","scope":"parent","role":"reverse","inherit_context":false}),
                        ),
                        1 | 3 => reply("wait", json!({"agent":lookup("parent").id})),
                        2 => reply(
                            "follow_up_agent",
                            json!({"agent_id":lookup("leaf").id,"message":"Update the leaf result","recipient_generation":0}),
                        ),
                        4 => {
                            assert!(text.contains("parent updated"));
                            reply("finish_scan", json!({"summary":"updated tree"}))
                        }
                        _ => panic!("unexpected root turn {roots}"),
                    };
                    roots += 1;
                    output
                } else if text.contains("Scope: parent.") {
                    let output = match parents {
                        0 => reply(
                            "task",
                            json!({"description":"leaf","prompt":"Leaf objective","scope":"leaf","role":"reverse","inherit_context":true}),
                        ),
                        1 | 3 => reply("wait", json!({"agent":lookup("leaf").id})),
                        2 => reply("agent_finish", json!({"summary":"parent original"})),
                        4 => {
                            assert!(text.contains("leaf updated"));
                            reply("agent_finish", json!({"summary":"parent updated"}))
                        }
                        _ => panic!("unexpected parent turn {parents}"),
                    };
                    parents += 1;
                    output
                } else {
                    assert!(text.contains("Scope: leaf."));
                    if leaves == 0 {
                        assert!(
                            text.contains("Parent objective"),
                            "closed parent context was not inherited"
                        );
                        assert!(
                            !input["messages"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|message| message["role"] == "tool"),
                            "inheriting a pending tool call fabricated a tool result"
                        );
                    }
                    let output = reply(
                        "agent_finish",
                        json!({"summary":if leaves==0 {"leaf original"} else {"leaf updated"}}),
                    );
                    leaves += 1;
                    output
                };
                respond(&mut socket, output).await;
            }
            assert_eq!((roots, parents, leaves), (5, 5, 2));
        });
        assert_eq!(
            drive_bounded(&runner, &run.id).await.status,
            Status::Completed
        );
        server.await.unwrap();
        let agents = runner.store.list::<AgentRecord>("agent", &run.id).unwrap();
        for scope in ["parent", "leaf"] {
            let agent = agents.iter().find(|agent| agent.scope == scope).unwrap();
            assert_eq!(agent.generation, 1);
            assert_eq!(agent.status, Status::Completed);
        }
    }
}
