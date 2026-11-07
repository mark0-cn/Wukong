use crate::{domain::*, store::Store};
use anyhow::{Context, Result, bail, ensure};
use chrono::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, fs, path::Path};

#[derive(Clone)]
pub struct Coordinator {
    pub store: Store,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildRequest {
    pub name: String,
    pub role: Role,
    pub task: String,
    pub scope: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default = "yes")]
    pub inherit_context: bool,
    /// Agent ids this child waits for: the runner starts it only after all
    /// of them are terminal. Lets a parent express "run X only after Y
    /// finished" as a scheduler constraint instead of prompt discipline.
    #[serde(default)]
    pub wait_for_predecessors: Vec<String>,
}
fn yes() -> bool {
    true
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnResult {
    pub created: bool,
    pub agent: AgentRecord,
}

impl Coordinator {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
    pub fn create_run(
        &self,
        target: &Path,
        task: &str,
        budget: Budget,
        config: Value,
    ) -> Result<RunRecord> {
        let target_path = fs::canonicalize(target).context("target must be an existing file")?;
        if !target_path.is_file() {
            bail!("target is not a file");
        }
        let target_sha256 = hex::encode(Sha256::digest(fs::read(&target_path)?));
        let run_id = id("run");
        let root_id = id("agent");
        let timestamp = now();
        let run = RunRecord {
            id: run_id.clone(),
            target_path,
            target_sha256,
            task: task.to_owned(),
            status: Status::Running,
            root_id: root_id.clone(),
            config,
            budget,
            created_at: timestamp,
            updated_at: timestamp,
            total_tokens: 0,
            cost_usd: 0.0,
            summary: None,
        };
        let root = AgentRecord {
            id: root_id,
            run_id: run_id.clone(),
            parent_id: None,
            name: "Root".into(),
            role: Role::Root,
            task: task.to_owned(),
            scope: "assessment".into(),
            skills: vec!["root".into()],
            active_skills: vec![],
            status: Status::Queued,
            depth: 0,
            generation: 0,
            inherit_context: false,
            inherited_context: None,
            checkpoint: None,
            wait_for: vec![],
            wait_until: None,
            wait_for_predecessors: vec![],
            turns: 0,
            total_tokens: 0,
            cost_usd: 0.0,
            token_cap: None, // root is governed by the run budget, not a role quota
            summary: None,
            error: None,
            created_at: timestamp,
            updated_at: timestamp,
        };
        self.store.transaction(|tx| {
            Store::tx_put(tx, "run", &run.id, &run.id, &run)?;
            Store::tx_put(tx, "agent", &root.id, &run.id, &root)?;
            Store::tx_event(
                tx,
                &run.id,
                Some(&root.id),
                "run.created",
                json!({"target_sha256":run.target_sha256,"task":task}),
            )?;
            Ok(())
        })?;
        Ok(run)
    }
    pub fn create_child(&self, parent_id: &str, request: ChildRequest) -> Result<SpawnResult> {
        self.store
            .transaction(|tx| Self::tx_create_child(tx, parent_id, request, id("agent")))
    }
    pub(crate) fn tx_create_child(
        tx: &rusqlite::Connection,
        parent_id: &str,
        request: ChildRequest,
        child_id: String,
    ) -> Result<SpawnResult> {
        if request.role == Role::Root {
            bail!("a child cannot have root role");
        }
        if request.name.trim().is_empty()
            || request.task.trim().is_empty()
            || request.scope.trim().is_empty()
        {
            bail!("name, task and scope must be nonempty");
        }
        if request.skills.len() > 5 {
            bail!("at most 5 related skills per agent");
        }
        let parent: AgentRecord =
            Store::tx_get(tx, "agent", parent_id)?.context("parent agent missing")?;
        if parent.status.terminal() {
            bail!("parent is terminal");
        }
        let run: RunRecord = Store::tx_get(tx, "run", &parent.run_id)?.context("run missing")?;
        if run.status.terminal() {
            bail!("run is terminal");
        }
        let agents = Store::tx_list::<AgentRecord>(tx, "agent", &run.id)?;
        if let Some(existing) = agents.iter().find(|a| {
            a.scope.trim() == request.scope.trim() && a.role == request.role && !a.status.terminal()
        }) {
            return Ok(SpawnResult {
                created: false,
                agent: existing.clone(),
            });
        }
        if agents.len() >= run.budget.max_agents {
            bail!("maximum total agent count reached");
        }
        if parent.depth + 1 > run.budget.max_depth {
            bail!("maximum agent depth reached");
        }
        for predecessor in &request.wait_for_predecessors {
            let target = agents
                .iter()
                .find(|a| &a.id == predecessor)
                .with_context(|| format!("wait_for_predecessors target missing: {predecessor}"))?;
            ensure!(
                target.run_id == run.id,
                "wait_for_predecessors target belongs to another run"
            );
        }
        let timestamp = now();
        let agent = AgentRecord {
            id: child_id,
            run_id: run.id.clone(),
            parent_id: Some(parent.id.clone()),
            name: request.name,
            role: request.role,
            task: request.task,
            scope: request.scope,
            skills: request.skills,
            active_skills: vec![],
            status: Status::Queued,
            depth: parent.depth + 1,
            generation: 0,
            inherit_context: request.inherit_context,
            inherited_context: if request.inherit_context {
                parent.checkpoint.clone()
            } else {
                None
            },
            checkpoint: None,
            wait_for: vec![],
            wait_until: None,
            wait_for_predecessors: request.wait_for_predecessors.clone(),
            turns: 0,
            total_tokens: 0,
            cost_usd: 0.0,
            token_cap: Some(run.budget.role_token_cap(request.role)),
            summary: None,
            error: None,
            created_at: timestamp,
            updated_at: timestamp,
        };
        let mut graph = agents;
        graph.push(agent.clone());
        Self::validate_dependencies(&graph)?;
        Store::tx_put(tx, "agent", &agent.id, &run.id, &agent)?;
        Store::tx_event(
            tx,
            &run.id,
            Some(&agent.id),
            "agent.created",
            json!({"parent_id":parent.id,"role":agent.role,"scope":agent.scope}),
        )?;
        Ok(SpawnResult {
            created: true,
            agent,
        })
    }

    /// Completion requires children to terminate, so parent -> child edges
    /// participate in the same graph as explicit predecessor dependencies.
    fn validate_dependencies(agents: &[AgentRecord]) -> Result<()> {
        fn visit<'a>(
            id: &'a str,
            agents: &'a [AgentRecord],
            visiting: &mut HashSet<&'a str>,
            visited: &mut HashSet<&'a str>,
        ) -> Result<()> {
            if visited.contains(id) {
                return Ok(());
            }
            ensure!(
                visiting.insert(id),
                "agent dependencies would deadlock (cycle at {id})"
            );
            if let Some(agent) = agents.iter().find(|a| a.id == id && !a.status.terminal()) {
                for dependency in agents.iter().filter(|other| {
                    !other.status.terminal()
                        && (other.parent_id.as_deref() == Some(id)
                            || agent.wait_for_predecessors.contains(&other.id))
                }) {
                    visit(&dependency.id, agents, visiting, visited)?;
                }
            }
            visiting.remove(id);
            visited.insert(id);
            Ok(())
        }
        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        for agent in agents.iter().filter(|a| !a.status.terminal()) {
            visit(&agent.id, agents, &mut visiting, &mut visited)?;
        }
        Ok(())
    }
    /// Informational messages never reopen a completed task or interrupt a wait.
    pub fn message(&self, sender: &str, recipient: &str, content: &str) -> Result<()> {
        self.send(sender, recipient, content, MessageOptions::default())
            .map(|_| ())
    }
    pub fn follow_up(&self, sender: &str, recipient: &str, content: &str) -> Result<()> {
        self.send(
            sender,
            recipient,
            content,
            MessageOptions {
                kind: MessageKind::FollowUp,
                ..Default::default()
            },
        )
        .map(|_| ())
    }
    pub fn send(
        &self,
        sender: &str,
        recipient: &str,
        content: &str,
        options: MessageOptions,
    ) -> Result<MessageReceipt> {
        self.store
            .transaction(|tx| Self::tx_send(tx, sender, recipient, content, options, None))
    }
    pub(crate) fn tx_send(
        tx: &rusqlite::Connection,
        sender: &str,
        recipient: &str,
        content: &str,
        mut options: MessageOptions,
        dedupe_key: Option<&str>,
    ) -> Result<MessageReceipt> {
        ensure!(
            !content.trim().is_empty() && content.len() <= 65536,
            "message must be nonempty and at most 64 KiB"
        );
        ensure!(
            options.evidence_ids.len() <= 32,
            "at most 32 evidence references per message"
        );
        let mut target: AgentRecord =
            Store::tx_get(tx, "agent", recipient)?.context("recipient missing")?;
        let run: RunRecord = Store::tx_get(tx, "run", &target.run_id)?.context("run missing")?;
        ensure!(!run.status.terminal(), "assessment is terminal");
        ensure!(
            !target.status.terminal() || target.status == Status::Completed,
            "recipient is stopped or failed; create a new focused task"
        );
        let sender_generation = if sender != "operator" {
            let from: AgentRecord =
                Store::tx_get(tx, "agent", sender)?.context("sender missing")?;
            ensure!(from.run_id == target.run_id, "cross-run message denied");
            ensure!(
                !from.status.terminal(),
                "terminal sender cannot issue new messages"
            );
            Some(from.generation)
        } else {
            None
        };
        for evidence_id in &options.evidence_ids {
            let evidence: Evidence =
                Store::tx_get(tx, "evidence", evidence_id)?.context("message evidence missing")?;
            ensure!(
                evidence.run_id == target.run_id,
                "cross-run evidence denied"
            );
        }
        let mut stale = options
            .recipient_generation
            .is_some_and(|value| value != target.generation);
        let original = if options.kind == MessageKind::Reply {
            let mid = options.reply_to.context("reply requires reply_to")?;
            let original = Store::tx_messages(tx, &target.run_id)?
                .into_iter()
                .find(|message| message.id == mid)
                .context("reply target not in this run")?;
            ensure!(
                original.envelope.options.kind == MessageKind::Question
                    && original.sender == recipient
                    && original.recipient == sender,
                "reply must answer a question addressed to this sender"
            );
            stale |= original
                .envelope
                .sender_generation
                .is_some_and(|generation| generation != target.generation)
                || original.envelope.options.recipient_generation != sender_generation;
            if let Some(correlation) = &options.correlation_id {
                ensure!(
                    Some(correlation) == original.envelope.options.correlation_id.as_ref(),
                    "reply correlation mismatch"
                );
            }
            options.correlation_id = original.envelope.options.correlation_id.clone();
            Some(original)
        } else {
            ensure!(
                options.reply_to.is_none(),
                "reply_to is only valid for replies"
            );
            None
        };
        let correlation = options
            .correlation_id
            .get_or_insert_with(|| id("conversation"))
            .clone();
        ensure!(
            !correlation.trim().is_empty() && correlation.len() <= 256,
            "invalid correlation_id"
        );
        let generation = if options.kind == MessageKind::FollowUp
            && target.status == Status::Completed
            && !stale
        {
            target
                .generation
                .checked_add(1)
                .context("agent generation exhausted")?
        } else {
            options.recipient_generation.unwrap_or(target.generation)
        };
        options.recipient_generation = Some(generation);
        let envelope = MessageEnvelope {
            options: options.clone(),
            task_id: target.id.clone(),
            sender_generation,
            delivery: if stale {
                MessageDelivery::Stale
            } else {
                MessageDelivery::Queued
            },
            stale_reason: stale.then(|| "message belongs to a different task generation".into()),
            ..Default::default()
        };
        let (mid, inserted) = Store::tx_enqueue_message(
            tx,
            &target.run_id,
            sender,
            recipient,
            content,
            dedupe_key,
            &envelope,
        )?;
        let receipt = MessageReceipt {
            message_id: mid,
            delivery: envelope.delivery,
            recipient_generation: generation,
            correlation_id: correlation,
        };
        if !inserted || stale {
            return Ok(receipt);
        }
        if let Some(mut original) = original {
            original.envelope.delivery = MessageDelivery::Replied;
            original.envelope.replied_by = Some(mid);
            tx.execute(
                "UPDATE messages SET envelope=?1 WHERE id=?2",
                rusqlite::params![serde_json::to_string(&original.envelope)?, original.id],
            )?;
        }
        if target.status == Status::Completed && options.kind == MessageKind::FollowUp {
            // Keep the old conversation as background, but start a new
            // Rig run so its lifecycle call cannot be replayed as new work.
            let mut current = Some(target.clone());
            while let Some(mut agent) = current {
                if agent.status != Status::Completed {
                    ensure!(!agent.status.terminal(), "ancestor is stopped or failed");
                    break;
                }
                agent.generation = agent
                    .generation
                    .checked_add(1)
                    .context("agent generation exhausted")?;
                tx.execute("UPDATE messages SET acknowledged=1,envelope=json_set(envelope,'$.delivery','stale','$.stale_reason','task reopened in a new generation') WHERE recipient=?1 AND acknowledged=0 AND COALESCE(json_extract(envelope,'$.recipient_generation'),-1)<?2",
                    rusqlite::params![agent.id,agent.generation])?;
                agent.inherited_context = agent.checkpoint.take().or(agent.inherited_context);
                agent.status = if run.status == Status::BudgetPaused {
                    Status::BudgetPaused
                } else {
                    Status::Queued
                };
                agent.wait_for.clear();
                agent.wait_until = None;
                agent.wait_for_predecessors.clear();
                agent.error = None;
                agent.updated_at = now();
                Store::tx_put(tx, "agent", &agent.id, &agent.run_id, &agent)?;
                Store::tx_event(
                    tx,
                    &agent.run_id,
                    Some(&agent.id),
                    "agent.reopened",
                    json!({"generation":agent.generation,"recipient":recipient}),
                )?;
                if agent.id == recipient {
                    target = agent.clone();
                } else {
                    Store::tx_message(
                        tx,
                        &agent.run_id,
                        "coordinator",
                        &agent.id,
                        &format!(
                            "A completed descendant {recipient} was reopened for a follow-up. Reconcile its new result before finishing."
                        ),
                        None,
                    )?;
                }
                current = agent
                    .parent_id
                    .as_ref()
                    .map(|id| Store::tx_get(tx, "agent", id)?.context("parent missing"))
                    .transpose()?;
            }
            Self::validate_dependencies(&Store::tx_list::<AgentRecord>(
                tx,
                "agent",
                &target.run_id,
            )?)?;
        } else if target.status == Status::Waiting && options.kind != MessageKind::Information {
            target.status = Status::Queued;
            // A question/reply interrupts execution, not the declared wait condition.
            if options.kind == MessageKind::FollowUp {
                target.wait_for.clear();
                target.wait_until = None;
            }
            target.updated_at = now();
            Store::tx_put(tx, "agent", recipient, &target.run_id, &target)?;
        }
        Store::tx_event(
            tx,
            &target.run_id,
            Some(recipient),
            "message.enqueued",
            json!({"message_id":mid,"sender":sender,"kind":options.kind,"generation":generation}),
        )?;
        Ok(receipt)
    }
    pub fn wait(&self, agent_id: &str, targets: &[String], timeout_seconds: u64) -> Result<Value> {
        self.store.transaction(|tx| {
            let mut agent: AgentRecord =
                Store::tx_get(tx, "agent", agent_id)?.context("agent missing")?;
            if agent.status.terminal() {
                bail!("agent is terminal");
            }
            let all = Store::tx_list::<AgentRecord>(tx, "agent", &agent.run_id)?;
            let ids: Vec<String> = if targets.is_empty() {
                all.iter()
                    .filter(|a| a.parent_id.as_deref() == Some(agent_id))
                    .map(|a| a.id.clone())
                    .collect()
            } else {
                targets.to_vec()
            };
            if ids.iter().any(|i| i == agent_id) {
                bail!("cannot wait for yourself");
            }
            let mut states = Vec::new();
            let mut pending = Vec::new();
            for target in &ids {
                let a = all
                    .iter()
                    .find(|a| &a.id == target)
                    .context("wait target not in this run")?;
                // Waiting on an ancestor produces a dependency cycle.
                let mut parent = agent.parent_id.clone();
                while let Some(pid) = parent {
                    if pid == a.id {
                        bail!("cannot wait for an ancestor");
                    }
                    parent = all
                        .iter()
                        .find(|a| a.id == pid)
                        .and_then(|a| a.parent_id.clone());
                }
                states
                    .push(json!({"id":a.id,"status":a.status,"summary":a.summary,"error":a.error}));
                if !a.status.terminal() {
                    pending.push(a.id.clone());
                }
            }
            if pending.is_empty() {
                return Ok(json!({"parked":false,"agents":states}));
            }
            agent.status = Status::Waiting;
            agent.wait_for = pending;
            agent.wait_until =
                Some(now() + Duration::seconds(timeout_seconds.clamp(1, 3600) as i64));
            agent.updated_at = now();
            Store::tx_put(tx, "agent", agent_id, &agent.run_id, &agent)?;
            Store::tx_event(
                tx,
                &agent.run_id,
                Some(agent_id),
                "agent.waiting",
                json!({"targets":agent.wait_for,"until":agent.wait_until}),
            )?;
            Ok(json!({"parked":true,"agents":states,"wake_at":agent.wait_until}))
        })
    }
    pub fn wake_waiters(&self, run_id: &str) -> Result<usize> {
        self.store.transaction(|tx|{
            let all=Store::tx_list::<AgentRecord>(tx,"agent",run_id)?;let mut changed=0;
            for mut agent in all.iter().filter(|a|a.status==Status::Waiting).cloned(){
                let ready=agent.wait_for.iter().all(|i|all.iter().find(|a|&a.id==i).is_none_or(|a|a.status.terminal()));
                let expired=agent.wait_until.is_some_and(|at|at<=now());
                let unread:i64=tx.query_row("SELECT COUNT(*) FROM messages WHERE recipient=?1 AND acknowledged=0 AND COALESCE(json_extract(envelope,'$.kind'),'information')!='information'",[&agent.id],|r|r.get(0))?;
                if ready||expired||unread>0 {
                    if ready||expired{
                        let key=format!("wait:{}:{}",agent.id,agent.wait_until.map(|d|d.timestamp_millis()).unwrap_or_default());
                        // Project the waiter's targets into bounded fields; a full
                        // AgentRecord would embed their conversation checkpoints
                        // into the parent's model context.
                        let states:Vec<_>=all.iter().filter(|a|agent.wait_for.contains(&a.id)).map(|a|json!({"id":a.id,"role":a.role,"status":a.status,"summary":a.summary,"error":a.error,"scope":a.scope})).collect();
                        Store::tx_message(tx,run_id,"coordinator",&agent.id,&json!({"type":"wait_finished","timed_out":expired&&!ready,"agents":states,"instruction":"Your wait finished. Act now: inspect these agent states and continue with tool calls (reconcile, spawn the next stage, or finish). Never answer with plain text — end the turn with a tool call."}).to_string(),Some(&key))?;
                    }
                    agent.status=Status::Queued;if ready||expired {agent.wait_for.clear();agent.wait_until=None;}agent.updated_at=now();Store::tx_put(tx,"agent",&agent.id,run_id,&agent)?;changed+=1;
                }
            }Ok(changed)
        })
    }
    pub fn set_status(&self, agent_id: &str, status: Status, error: Option<String>) -> Result<()> {
        self.store.transaction(|tx| {
            let mut agent: AgentRecord =
                Store::tx_get(tx, "agent", agent_id)?.context("agent missing")?;
            if agent.status.terminal() {
                bail!("cannot transition terminal agent {}", agent.id);
            }
            agent.status = status;
            agent.error = error;
            agent.updated_at = now();
            Store::tx_put(tx, "agent", agent_id, &agent.run_id, &agent)?;
            if status.terminal() {
                Self::notify_terminal(tx, &agent)?;
            }
            Store::tx_event(
                tx,
                &agent.run_id,
                Some(agent_id),
                "agent.status",
                json!({"status":status,"error":agent.error}),
            )?;
            Ok(())
        })
    }
    pub(crate) fn notify_terminal(tx: &rusqlite::Connection, agent: &AgentRecord) -> Result<()> {
        if let Some(parent) = &agent.parent_id {
            let key = if agent.generation == 0 {
                format!("terminal:{}", agent.id)
            } else {
                format!("terminal:{}:{}", agent.id, agent.generation)
            };
            Store::tx_message(tx,&agent.run_id,&agent.id,parent,&json!({"type":"agent_finished","agent_id":agent.id,"generation":agent.generation,"status":agent.status,"summary":agent.summary,"error":agent.error}).to_string(),Some(&key))?;
        }
        Ok(())
    }
    pub fn finish(&self, agent_id: &str, summary: &str) -> Result<()> {
        self.store
            .transaction(|tx| Self::tx_finish(tx, agent_id, summary))
    }
    /// Transactional finish usable inside a tool-effect receipt, so a durable
    /// retry after an uncertain outcome replays the stored result instead of
    /// re-executing the lifecycle transition.
    pub(crate) fn tx_finish(
        tx: &rusqlite::Connection,
        agent_id: &str,
        summary: &str,
    ) -> Result<()> {
        if summary.trim().is_empty() {
            bail!("completion summary must be nonempty");
        }
        let mut agent: AgentRecord =
            Store::tx_get(tx, "agent", agent_id)?.context("agent missing")?;
        if agent.status == Status::Completed {
            return Ok(());
        }
        if agent.status.terminal() {
            bail!("agent already terminated with {}", agent.status);
        }
        let unread: i64 = tx.query_row(
            "SELECT COUNT(*) FROM messages WHERE recipient=?1 AND acknowledged=0",
            [agent_id],
            |r| r.get(0),
        )?;
        if unread > 0 {
            bail!(
                "unread coordinator messages remain; read them at the next model boundary before finishing"
            );
        }
        let all = Store::tx_list::<AgentRecord>(tx, "agent", &agent.run_id)?;
        let active: Vec<_> = all
            .iter()
            .filter(|a| !a.status.terminal() && Self::is_descendant(a, agent_id, &all))
            .map(|a| a.id.clone())
            .collect();
        if !active.is_empty() {
            bail!("active descendants remain: {}", active.join(", "));
        }
        for kind in ["debug_session", "interact_session"] {
            let open: Vec<_> = Store::tx_list::<Value>(tx, kind, &agent.run_id)?
                .into_iter()
                .filter(|session| {
                    (agent.role == Role::Root || session["agent_id"] == agent.id)
                        && !matches!(session["state"].as_str(), Some("closed" | "lost"))
                })
                .map(|session| format!("{} (owner {})", session["id"], session["agent_id"]))
                .collect();
            ensure!(
                open.is_empty(),
                "open {kind} sessions remain; close them before finishing: {}",
                open.join(", ")
            );
        }
        if agent.role == Role::Root {
            let unresolved = Store::tx_list::<Coverage>(tx, "coverage", &agent.run_id)?
                .into_iter()
                .filter(|c| c.outcome == "needs_follow_up")
                .map(|c| c.id)
                .collect::<Vec<_>>();
            if !unresolved.is_empty() {
                bail!(
                    "coverage needs follow-up; resolve or mark inconclusive: {}",
                    unresolved.join(", ")
                );
            }
            // Worker jobs (fuzz, replay, minimize, verification, angr) outlive
            // their agents and keep running after finish_scan; completing the
            // assessment while they run would strand them unaccounted.
            let running_jobs: Vec<String> =
                Store::tx_list::<serde_json::Value>(tx, "job", &agent.run_id)?
                    .into_iter()
                    .filter(|job| {
                        let state = job["status"]["state"].as_str().unwrap_or_default();
                        !matches!(
                            state,
                            "completed" | "failed" | "cancelled" | "timed_out" | "lost"
                        )
                    })
                    .map(|job| job["id"].as_str().unwrap_or("unnamed").to_owned())
                    .collect();
            if !running_jobs.is_empty() {
                bail!(
                    "running worker jobs remain; wait for or cancel them before finish_scan: {}",
                    running_jobs.join(", ")
                );
            }
        }
        agent.status = Status::Completed;
        agent.summary = Some(summary.to_owned());
        agent.updated_at = now();
        Store::tx_put(tx, "agent", agent_id, &agent.run_id, &agent)?;
        Self::notify_terminal(tx, &agent)?;
        if agent.role == Role::Root {
            let mut run: RunRecord =
                Store::tx_get(tx, "run", &agent.run_id)?.context("run missing")?;
            run.status = Status::Completed;
            run.summary = Some(summary.to_owned());
            run.updated_at = now();
            Store::tx_put(tx, "run", &run.id, &run.id, &run)?;
        }
        Store::tx_event(
            tx,
            &agent.run_id,
            Some(agent_id),
            "agent.completed",
            json!({"summary":summary}),
        )?;
        Ok(())
    }
    fn is_descendant(agent: &AgentRecord, parent: &str, all: &[AgentRecord]) -> bool {
        let mut current = agent.parent_id.as_deref();
        let mut traversed = 0;
        while let Some(id) = current {
            if id == parent {
                return true;
            }
            current = all
                .iter()
                .find(|a| a.id == id)
                .and_then(|a| a.parent_id.as_deref());
            traversed += 1;
            if traversed > all.len() {
                break;
            }
        }
        false
    }
    pub fn stop(&self, agent_id: &str, reason: &str) -> Result<Vec<String>> {
        self.store
            .transaction(|tx| Self::tx_stop(tx, agent_id, reason))
    }
    /// Transactional stop usable inside a tool-effect receipt, so a durable
    /// retry after an uncertain outcome replays the stored result instead of
    /// re-executing the lifecycle transition.
    pub(crate) fn tx_stop(
        tx: &rusqlite::Connection,
        agent_id: &str,
        reason: &str,
    ) -> Result<Vec<String>> {
        let root: AgentRecord = Store::tx_get(tx, "agent", agent_id)?.context("agent missing")?;
        let all = Store::tx_list::<AgentRecord>(tx, "agent", &root.run_id)?;
        let mut stopped = Vec::new();
        for mut agent in all
            .iter()
            .filter(|a| a.id == agent_id || Self::is_descendant(a, agent_id, &all))
            .cloned()
        {
            if agent.status.terminal() {
                continue;
            }
            agent.status = Status::Stopped;
            agent.error = Some(reason.to_owned());
            agent.updated_at = now();
            Store::tx_put(tx, "agent", &agent.id, &agent.run_id, &agent)?;
            Self::notify_terminal(tx, &agent)?;
            stopped.push(agent.id);
        }
        if root.role == Role::Root {
            let mut run: RunRecord =
                Store::tx_get(tx, "run", &root.run_id)?.context("run missing")?;
            if !run.status.terminal() {
                run.status = Status::Stopped;
                run.updated_at = now();
                Store::tx_put(tx, "run", &run.id, &run.id, &run)?;
            }
        }
        Store::tx_event(
            tx,
            &root.run_id,
            Some(agent_id),
            "agents.stopped",
            json!({"agents":stopped,"reason":reason}),
        )?;
        Ok(stopped)
    }
    pub fn pause_budget(&self, run_id: &str, reason: &str) -> Result<()> {
        self.store.transaction(|tx| {
            let mut run: RunRecord = Store::tx_get(tx, "run", run_id)?.context("run missing")?;
            if run.status.terminal() {
                return Ok(());
            }
            run.status = Status::BudgetPaused;
            run.updated_at = now();
            Store::tx_put(tx, "run", run_id, run_id, &run)?;
            for mut agent in Store::tx_list::<AgentRecord>(tx, "agent", run_id)? {
                if !agent.status.terminal() {
                    agent.status = Status::BudgetPaused;
                    agent.updated_at = now();
                    Store::tx_put(tx, "agent", &agent.id, run_id, &agent)?;
                }
            }
            Store::tx_event(
                tx,
                run_id,
                None,
                "run.budget_paused",
                json!({"reason":reason}),
            )?;
            Ok(())
        })
    }
    pub fn resume(&self, run_id: &str, budget: Option<Budget>) -> Result<()> {
        self.store.transaction(|tx| {
            let mut run: RunRecord = Store::tx_get(tx, "run", run_id)?.context("run missing")?;
            if run.status.terminal() {
                bail!("terminal run cannot be resumed");
            }
            if let Some(budget) = budget {
                run.budget = budget;
            }
            run.status = Status::Running;
            run.updated_at = now();
            Store::tx_put(tx, "run", run_id, run_id, &run)?;
            for mut a in Store::tx_list::<AgentRecord>(tx, "agent", run_id)? {
                let mut changed = false;
                if !a.status.terminal() && a.role != Role::Root {
                    // Role caps follow the run budget: a resumed run with an
                    // enlarged budget rescales every surviving worker's quota.
                    // The root is governed by the run budget alone.
                    a.token_cap = Some(run.budget.role_token_cap(a.role));
                    changed = true;
                }
                if matches!(a.status, Status::Running | Status::BudgetPaused) {
                    a.status = if a.wait_until.is_some() {
                        Status::Waiting
                    } else {
                        Status::Queued
                    };
                    changed = true;
                }
                if changed {
                    a.updated_at = now();
                    Store::tx_put(tx, "agent", &a.id, run_id, &a)?;
                }
            }
            Store::tx_event(tx, run_id, None, "run.resumed", json!({}))?;
            Ok(())
        })
    }
}
