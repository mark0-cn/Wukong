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
    pub contract: TaskContract,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default = "yes")]
    pub inherit_context: bool,
    /// Predecessor agents evaluated using dependency_policy before admission.
    #[serde(default)]
    pub wait_for_predecessors: Vec<String>,
    #[serde(default)]
    pub dependency_policy: DependencyPolicy,
}
fn yes() -> bool {
    true
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnResult {
    pub created: bool,
    pub agent: AgentSnapshot,
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
        let root = AgentSnapshot {
            metadata: AgentMetadata {
                id: root_id,
                run_id: run_id.clone(),
                parent_id: None,
                name: "Root".into(),
                role: Role::Root,
                task: task.to_owned(),
                scope: "assessment".into(),
                contract: TaskContract {
                    objective: task.into(),
                    ..Default::default()
                },
                task_result: None,
                skills: vec![],
                active_skills: vec![],
                status: Status::Queued,
                depth: 0,
                generation: 0,
                inherit_context: false,
                wait_for: vec![],
                wait_until: None,
                wait_policy: DependencyPolicy::AllTerminal,
                wait_for_predecessors: vec![],
                dependency_policy: DependencyPolicy::AllSuccess,
                predecessors_released: true,
                turns: 0,
                total_tokens: 0,
                cost_usd: 0.0,
                token_cap: None, // root is governed by the run budget, not a role quota
                summary: None,
                error: None,
                created_at: timestamp,
                updated_at: timestamp,
            },
            inherited_context: None,
            checkpoint: None,
        };
        self.store.write(|tx| {
            tx.put("run", &run.id, &run.id, &run)?;
            tx.put_metadata("agent", &root.id, &run.id, &root)?;
            tx.event(
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
        let parent: AgentMetadata = self.store.require_metadata("agent", parent_id)?;
        self.store
            .verify_evidence_links(&parent.run_id, &request.contract.input_evidence_ids)?;
        self.store
            .write(|tx| Self::tx_create_child(tx, parent_id, request, id("agent")))
    }
    pub(crate) fn tx_create_child(
        tx: &crate::store::StateSession<'_>,
        parent_id: &str,
        mut request: ChildRequest,
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
        crate::agents::validate_capabilities(request.skills.iter().map(String::as_str))?;
        let parent: AgentMetadata = tx
            .get("agent", parent_id)?
            .context("parent agent missing")?;
        if parent.status.terminal() {
            bail!("parent is terminal");
        }
        let run: RunRecord = tx.get("run", &parent.run_id)?.context("run missing")?;
        if run.status.terminal() {
            bail!("run is terminal");
        }
        if request.contract.objective.trim().is_empty() {
            request.contract.objective = request.task.clone();
        }
        Self::validate_contract(tx, &run.id, &request.contract)?;
        let agents = tx.list_metadata("agent", &run.id)?;
        if let Some(existing) = agents.iter().find(|a| {
            a.scope.trim() == request.scope.trim() && a.role == request.role && !a.status.terminal()
        }) {
            let mut existing_contract = existing.contract.clone();
            if existing_contract.objective.trim().is_empty() {
                existing_contract.objective = existing.task.clone();
            }
            ensure!(
                existing_contract == request.contract
                    && existing.dependency_policy == request.dependency_policy
                    && existing.wait_for_predecessors == request.wait_for_predecessors,
                "active scope has a different task contract; send a clarification or use a distinct scope"
            );
            return Ok(SpawnResult {
                created: false,
                agent: tx.get("agent", &existing.id)?.context("agent missing")?,
            });
        }
        if agents.len() >= run.budget.max_agents {
            bail!("maximum total agent count reached");
        }
        if parent.depth + 1 > run.budget.max_depth {
            bail!("maximum agent depth reached");
        }
        ensure!(
            request.wait_for_predecessors.len() <= 128
                && request
                    .wait_for_predecessors
                    .iter()
                    .collect::<HashSet<_>>()
                    .len()
                    == request.wait_for_predecessors.len(),
            "invalid or duplicate predecessors"
        );
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
        let agent = AgentSnapshot {
            metadata: AgentMetadata {
                id: child_id,
                run_id: run.id.clone(),
                parent_id: Some(parent.id.clone()),
                name: request.name,
                role: request.role,
                task: request.task,
                scope: request.scope,
                contract: request.contract,
                task_result: None,
                skills: request.skills,
                active_skills: vec![],
                status: Status::Queued,
                depth: parent.depth + 1,
                generation: 0,
                inherit_context: request.inherit_context,
                wait_for: vec![],
                wait_until: None,
                wait_policy: DependencyPolicy::AllTerminal,
                wait_for_predecessors: request.wait_for_predecessors.clone(),
                dependency_policy: request.dependency_policy,
                predecessors_released: request.wait_for_predecessors.is_empty(),
                turns: 0,
                total_tokens: 0,
                cost_usd: 0.0,
                token_cap: Some(run.budget.role_token_cap(request.role)),
                summary: None,
                error: None,
                created_at: timestamp,
                updated_at: timestamp,
            },
            inherited_context: if request.inherit_context {
                tx.agent_conversation(&parent.id)?.checkpoint
            } else {
                None
            },
            checkpoint: None,
        };
        let mut graph = agents;
        graph.push(agent.metadata.clone());
        Self::validate_dependencies(&graph)?;
        tx.put("agent", &agent.id, &run.id, &agent)?;
        tx.event(
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
    fn validate_dependencies(agents: &[AgentMetadata]) -> Result<()> {
        fn visit<'a>(
            id: &'a str,
            agents: &'a [AgentMetadata],
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
                            || (!agent.predecessors_released
                                && agent.wait_for_predecessors.contains(&other.id))
                            || agent.wait_for.contains(&other.id))
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
            .write(|tx| Self::tx_send(tx, sender, recipient, content, options, None))
    }
    pub(crate) fn tx_send(
        tx: &crate::store::StateSession<'_>,
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
        let mut target: AgentMetadata = tx
            .get_metadata("agent", recipient)?
            .context("recipient missing")?;
        let run: RunRecord = tx.get("run", &target.run_id)?.context("run missing")?;
        ensure!(!run.status.terminal(), "assessment is terminal");
        ensure!(
            !target.status.terminal() || target.status == Status::Completed,
            "recipient is stopped or failed; create a new focused task"
        );
        let sender_generation = if sender != "operator" {
            let from: AgentMetadata = tx
                .get_metadata("agent", sender)?
                .context("sender missing")?;
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
            let evidence: Evidence = tx
                .get("evidence", evidence_id)?
                .context("message evidence missing")?;
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
            let original = tx
                .messages(&target.run_id)?
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
        let (mid, inserted) = tx.enqueue_message(
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
            tx.replace_envelope(&original)?;
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
                let history: AgentSnapshot =
                    tx.agent_snapshot(&agent.id)?.context("agent missing")?;
                let mut conversation = history.into_conversation();
                agent.generation = agent
                    .generation
                    .checked_add(1)
                    .context("agent generation exhausted")?;
                tx.stale_mail_before(&agent.id, agent.generation)?;
                conversation.inherited_context = conversation
                    .checkpoint
                    .take()
                    .or(conversation.inherited_context);
                agent.status = if run.status == Status::BudgetPaused {
                    Status::BudgetPaused
                } else {
                    Status::Queued
                };
                agent.wait_for.clear();
                agent.wait_until = None;
                agent.wait_for_predecessors.clear();
                agent.predecessors_released = true;
                agent.error = None;
                agent.task_result = None;
                agent.updated_at = now();
                tx.put(
                    "agent",
                    &agent.id,
                    &agent.run_id,
                    &AgentSnapshot::from_parts(agent.clone(), conversation),
                )?;
                tx.event(
                    &agent.run_id,
                    Some(&agent.id),
                    "agent.reopened",
                    json!({"generation":agent.generation,"recipient":recipient}),
                )?;
                if agent.id == recipient {
                    target = agent.clone();
                } else {
                    tx.message(&agent.run_id,
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
                    .map(|id| tx.get_metadata("agent", id)?.context("parent missing"))
                    .transpose()?;
            }
            Self::validate_dependencies(&tx.list_metadata("agent", &target.run_id)?)?;
        } else if target.status == Status::Waiting && options.kind != MessageKind::Information {
            target.status = Status::Queued;
            // A question/reply interrupts execution, not the declared wait condition.
            if options.kind == MessageKind::FollowUp {
                target.wait_for.clear();
                target.wait_until = None;
            }
            target.updated_at = now();
            tx.put_metadata("agent", recipient, &target.run_id, &target)?;
        }
        tx.event(
            &target.run_id,
            Some(recipient),
            "message.enqueued",
            json!({"message_id":mid,"sender":sender,"kind":options.kind,"generation":generation}),
        )?;
        Ok(receipt)
    }
    pub fn dependency_state(
        policy: DependencyPolicy,
        targets: &[String],
        all: &[AgentMetadata],
    ) -> DependencyState {
        if targets.is_empty() {
            return DependencyState::Satisfied;
        }
        let mut terminal = 0;
        let mut successes = 0;
        for target in targets {
            let Some(agent) = all.iter().find(|agent| &agent.id == target) else {
                return DependencyState::Blocked {
                    reason: format!("dependency {target} is missing"),
                };
            };
            let success = agent.status == Status::Completed
                && agent
                    .task_result
                    .as_ref()
                    .is_some_and(|result| result.outcome == TaskOutcome::Success);
            if success {
                successes += 1;
            }
            if agent.status.terminal() {
                terminal += 1;
            }
            if policy == DependencyPolicy::AllSuccess && agent.status.terminal() && !success {
                return DependencyState::Blocked {
                    reason: format!(
                        "dependency {target} ended without success ({})",
                        agent.status
                    ),
                };
            }
        }
        match policy {
            DependencyPolicy::AllSuccess if successes == targets.len() => {
                DependencyState::Satisfied
            }
            DependencyPolicy::AllTerminal if terminal == targets.len() => {
                DependencyState::Satisfied
            }
            DependencyPolicy::AnySuccess if successes > 0 => DependencyState::Satisfied,
            DependencyPolicy::AnySuccess if terminal == targets.len() => DependencyState::Blocked {
                reason: "all dependency alternatives ended without success".into(),
            },
            _ => DependencyState::Pending,
        }
    }
    /// Resolve dependency admission once. Later follow-ups do not revoke work
    /// already admitted from the predecessor results recorded in this event.
    pub fn release_dependencies(&self, run_id: &str) -> Result<usize> {
        self.store.write(|tx| {
            let mut all=tx.list_metadata("agent",run_id)?;
            let mut count=0;
            loop {
                let mut changed=false;
                for index in 0..all.len() {
                    let mut agent=all[index].clone();
                    if agent.status!=Status::Queued || agent.predecessors_released {continue;}
                    let state=Self::dependency_state(agent.dependency_policy,&agent.wait_for_predecessors,&all);
                    match &state {
                        DependencyState::Pending=>continue,
                        DependencyState::Satisfied=>agent.predecessors_released=true,
                        DependencyState::Blocked{reason}=>{
                            agent.status=Status::Failed;
                            agent.error=Some(reason.clone());
                            let result=TaskResult{outcome:TaskOutcome::Blocked,answer:reason.clone(),next_actions:vec!["parent must revise the dependency, retry the prerequisite or choose a fallback".into()],..Default::default()};
                            agent.summary=Some(result.answer.clone());
                            agent.task_result=Some(result.clone());
                            tx.put("task_result",&format!("{}:{}",agent.id,agent.generation),run_id,&json!({"task_id":agent.id,"generation":agent.generation,"result":result}))?;
                            Self::notify_terminal(tx,&agent)?;
                        }
                    }
                    agent.updated_at=now();
                    tx.put_metadata("agent",&agent.id,run_id,&agent)?;
                    let predecessors:Vec<_>=all.iter().filter(|other|agent.wait_for_predecessors.contains(&other.id)).map(|other|json!({"id":other.id,"generation":other.generation,"status":other.status,"outcome":other.task_result.as_ref().map(|result|result.outcome)})).collect();
                    tx.event(run_id,Some(&agent.id),"agent.dependencies_resolved",json!({"policy":agent.dependency_policy,"result":state,"predecessors":predecessors}))?;
                    all[index]=agent;changed=true;count+=1;
                }
                if !changed {break;}
            }
            Ok(count)
        })
    }
    pub fn wait(&self, agent_id: &str, targets: &[String], timeout_seconds: u64) -> Result<Value> {
        self.wait_with_policy(
            agent_id,
            targets,
            timeout_seconds,
            DependencyPolicy::AllTerminal,
        )
    }
    pub fn wait_with_policy(
        &self,
        agent_id: &str,
        targets: &[String],
        timeout_seconds: u64,
        policy: DependencyPolicy,
    ) -> Result<Value> {
        self.store
            .write(|tx| Self::tx_wait(tx, agent_id, targets, timeout_seconds, policy))
    }
    pub(crate) fn tx_wait(
        tx: &crate::store::StateSession<'_>,
        agent_id: &str,
        targets: &[String],
        timeout_seconds: u64,
        policy: DependencyPolicy,
    ) -> Result<Value> {
        let mut agent: AgentMetadata = tx
            .get_metadata("agent", agent_id)?
            .context("agent missing")?;
        ensure!(!agent.status.terminal(), "agent is terminal");
        let all = tx.list_metadata("agent", &agent.run_id)?;
        let ids: Vec<_> = if targets.is_empty() {
            all.iter()
                .filter(|other| other.parent_id.as_deref() == Some(agent_id))
                .map(|other| other.id.clone())
                .collect()
        } else {
            targets.to_vec()
        };
        ensure!(
            ids.len() <= 128 && ids.iter().collect::<HashSet<_>>().len() == ids.len(),
            "invalid or duplicate wait targets"
        );
        ensure!(
            !ids.iter().any(|id| id == agent_id),
            "cannot wait for yourself"
        );
        for id in &ids {
            ensure!(
                all.iter().any(|other| &other.id == id),
                "wait target not in this run"
            );
            let mut parent = agent.parent_id.as_deref();
            while let Some(pid) = parent {
                ensure!(pid != id, "cannot wait for an ancestor");
                parent = all
                    .iter()
                    .find(|other| other.id == pid)
                    .and_then(|other| other.parent_id.as_deref());
            }
        }
        let state = Self::dependency_state(policy, &ids, &all);
        let states: Vec<_> = all
            .iter()
            .filter(|other| ids.contains(&other.id))
            .map(Self::wait_view)
            .collect();
        if state != DependencyState::Pending {
            return Ok(json!({"parked":false,"agents":states,"condition":state,"policy":policy}));
        }
        agent.status = Status::Waiting;
        agent.wait_for = ids;
        agent.wait_policy = policy;
        agent.wait_until = Some(now() + Duration::seconds(timeout_seconds.clamp(1, 3600) as i64));
        agent.updated_at = now();
        let mut graph = all;
        let index = graph
            .iter()
            .position(|other| other.id == agent.id)
            .context("agent missing")?;
        graph[index] = agent.clone();
        Self::validate_dependencies(&graph)?;
        tx.put_metadata("agent", agent_id, &agent.run_id, &agent)?;
        tx.event(
            &agent.run_id,
            Some(agent_id),
            "agent.waiting",
            json!({"targets":agent.wait_for,"until":agent.wait_until,"policy":policy}),
        )?;
        Ok(
            json!({"parked":true,"agents":states,"wake_at":agent.wait_until,"condition":state,"policy":policy}),
        )
    }
    fn wait_view(agent: &AgentMetadata) -> Value {
        json!({"id":agent.id,"role":agent.role,"status":agent.status,"summary":agent.summary,"result":agent.task_result,"error":agent.error,"scope":agent.scope})
    }
    pub fn wake_waiters(&self, run_id: &str) -> Result<usize> {
        self.store.write(|tx| {
            let all=tx.list_metadata("agent",run_id)?;let mut changed=0;
            for mut agent in all.iter().filter(|agent|agent.status==Status::Waiting).cloned() {
                let condition=Self::dependency_state(agent.wait_policy,&agent.wait_for,&all);
                let ready=condition!=DependencyState::Pending;
                let expired=agent.wait_until.is_some_and(|at|at<=now());
                let unread=tx.unread_messages(&agent.id,true)?;
                if !ready&&!expired&&unread==0 {continue;}
                if ready||expired {
                    let key=format!("wait:{}:{}:{}",agent.id,agent.generation,agent.wait_until.map(|date|date.timestamp_millis()).unwrap_or_default());
                    let states:Vec<_>=all.iter().filter(|other|agent.wait_for.contains(&other.id)).map(Self::wait_view).collect();
                    tx.message(run_id,"coordinator",&agent.id,&json!({"type":"wait_finished","timed_out":expired&&!ready,"condition":condition,"policy":agent.wait_policy,"agents":states,"instruction":"Inspect the dependency outcome and continue with tools: reconcile results, choose a fallback, or finish."}).to_string(),Some(&key))?;
                    agent.wait_for.clear();agent.wait_until=None;
                }
                agent.status=Status::Queued;agent.updated_at=now();
                tx.put_metadata("agent",&agent.id,run_id,&agent)?;changed+=1;
            }
            Ok(changed)
        })
    }
    pub fn set_status(&self, agent_id: &str, status: Status, error: Option<String>) -> Result<()> {
        self.store.write(|tx| {
            let mut agent: AgentMetadata = tx
                .get_metadata("agent", agent_id)?
                .context("agent missing")?;
            if agent.status.terminal() {
                bail!("cannot transition terminal agent {}", agent.id);
            }
            agent.status = status;
            agent.error = error;
            agent.updated_at = now();
            tx.put_metadata("agent", agent_id, &agent.run_id, &agent)?;
            if status.terminal() {
                Self::notify_terminal(tx, &agent)?;
            }
            tx.event(
                &agent.run_id,
                Some(agent_id),
                "agent.status",
                json!({"status":status,"error":agent.error}),
            )?;
            Ok(())
        })
    }
    pub(crate) fn notify_terminal(
        tx: &crate::store::StateSession<'_>,
        agent: &AgentMetadata,
    ) -> Result<()> {
        if let Some(parent) = &agent.parent_id {
            let key = if agent.generation == 0 {
                format!("terminal:{}", agent.id)
            } else {
                format!("terminal:{}:{}", agent.id, agent.generation)
            };
            tx.message(&agent.run_id,&agent.id,parent,&json!({"type":"agent_finished","agent_id":agent.id,"generation":agent.generation,"status":agent.status,"summary":agent.summary,"result":agent.task_result,"error":agent.error}).to_string(),Some(&key))?;
        }
        Ok(())
    }
    pub fn finish(&self, agent_id: &str, summary: &str) -> Result<()> {
        self.finish_result(
            agent_id,
            TaskResult {
                answer: summary.into(),
                ..Default::default()
            },
        )
    }
    pub fn finish_result(&self, agent_id: &str, result: TaskResult) -> Result<()> {
        let agent: AgentMetadata = self.store.require_metadata("agent", agent_id)?;
        self.store
            .verify_evidence_links(&agent.run_id, &result.evidence_links())?;
        self.store
            .write(|tx| Self::tx_finish_result(tx, agent_id, result))
    }
    pub(crate) fn tx_finish_result(
        tx: &crate::store::StateSession<'_>,
        agent_id: &str,
        result: TaskResult,
    ) -> Result<()> {
        let summary = &result.answer;
        if summary.trim().is_empty() {
            bail!("completion summary must be nonempty");
        }
        let mut agent: AgentMetadata = tx
            .get_metadata("agent", agent_id)?
            .context("agent missing")?;
        if agent.status == Status::Completed {
            return Ok(());
        }
        if agent.status.terminal() {
            bail!("agent already terminated with {}", agent.status);
        }
        Self::validate_result(tx, &agent, &result)?;
        let unread = tx.unread_messages(agent_id, false)?;
        if unread > 0 {
            bail!(
                "unread coordinator messages remain; read them at the next model boundary before finishing"
            );
        }
        let all = tx.list_metadata("agent", &agent.run_id)?;
        let active: Vec<_> = all
            .iter()
            .filter(|a| !a.status.terminal() && Self::is_descendant(a, agent_id, &all))
            .map(|a| a.id.clone())
            .collect();
        if !active.is_empty() {
            bail!("active descendants remain: {}", active.join(", "));
        }
        for kind in ["debug_session", "interact_session"] {
            let open: Vec<_> = tx
                .list::<Value>(kind, &agent.run_id)?
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
            let unresolved = tx
                .list::<Coverage>("coverage", &agent.run_id)?
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
            let running_jobs: Vec<String> = tx
                .list::<serde_json::Value>("job", &agent.run_id)?
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
        agent.status = if result.outcome == TaskOutcome::Cancelled {
            Status::Stopped
        } else {
            Status::Completed
        };
        agent.task_result = Some(result.clone());
        tx.put(
            "task_result",
            &format!("{}:{}", agent.id, agent.generation),
            &agent.run_id,
            &json!({"task_id":agent.id,"generation":agent.generation,"result":result}),
        )?;
        agent.summary = Some(summary.to_owned());
        agent.updated_at = now();
        tx.put_metadata("agent", agent_id, &agent.run_id, &agent)?;
        Self::notify_terminal(tx, &agent)?;
        if agent.role == Role::Root {
            let mut run: RunRecord = tx.get("run", &agent.run_id)?.context("run missing")?;
            run.status = agent.status;
            run.summary = Some(summary.to_owned());
            run.updated_at = now();
            tx.put("run", &run.id, &run.id, &run)?;
        }
        tx.event(
            &agent.run_id,
            Some(agent_id),
            "agent.completed",
            json!({"summary":summary}),
        )?;
        Ok(())
    }
    fn validate_contract(
        tx: &crate::store::StateSession<'_>,
        run_id: &str,
        contract: &TaskContract,
    ) -> Result<()> {
        ensure!(
            !contract.objective.trim().is_empty() && contract.objective.len() <= 65536,
            "invalid contract objective"
        );
        for values in [
            &contract.constraints,
            &contract.deliverables,
            &contract.completion_conditions,
        ] {
            ensure!(
                values.len() <= 32
                    && values
                        .iter()
                        .all(|value| !value.trim().is_empty() && value.len() <= 4096),
                "invalid contract requirement"
            );
            ensure!(
                values.iter().collect::<HashSet<_>>().len() == values.len(),
                "duplicate contract requirement"
            );
        }
        Self::validate_evidence(tx, run_id, &contract.input_evidence_ids)
    }
    fn validate_evidence(
        tx: &crate::store::StateSession<'_>,
        run_id: &str,
        links: &[String],
    ) -> Result<()> {
        ensure!(links.len() <= 128, "too many task evidence references");
        for link in links {
            let evidence: Evidence = tx.get("evidence", link)?.context("task evidence missing")?;
            ensure!(
                evidence.run_id == run_id,
                "task evidence belongs to another run"
            );
        }
        Ok(())
    }
    fn validate_result(
        tx: &crate::store::StateSession<'_>,
        agent: &AgentMetadata,
        result: &TaskResult,
    ) -> Result<()> {
        ensure!(
            !result.answer.trim().is_empty() && result.answer.len() <= 65536,
            "invalid result answer"
        );
        Self::validate_evidence(tx, &agent.run_id, &result.evidence_links())?;
        ensure!(result.artifacts.len() <= 32, "too many task artifacts");
        let names: HashSet<_> = result
            .artifacts
            .iter()
            .map(|artifact| artifact.name.as_str())
            .collect();
        ensure!(
            names.len() == result.artifacts.len()
                && names
                    .iter()
                    .all(|name| !name.trim().is_empty() && name.len() <= 4096),
            "invalid or duplicate artifact name"
        );
        for values in [
            &result.completed_conditions,
            &result.unknowns,
            &result.next_actions,
        ] {
            ensure!(
                values.len() <= 32
                    && values
                        .iter()
                        .all(|value| !value.trim().is_empty() && value.len() <= 4096),
                "invalid task result detail"
            );
        }
        ensure!(
            result
                .completed_conditions
                .iter()
                .all(|condition| agent.contract.completion_conditions.contains(condition)),
            "result claims an undeclared completion condition"
        );
        if result.outcome == TaskOutcome::Success {
            ensure!(
                agent
                    .contract
                    .deliverables
                    .iter()
                    .all(|name| names.contains(name.as_str())),
                "required task deliverables missing; provide structured result artifacts"
            );
            ensure!(
                agent
                    .contract
                    .completion_conditions
                    .iter()
                    .all(|condition| result.completed_conditions.contains(condition)),
                "required completion conditions not accounted for"
            );
        } else {
            ensure!(
                !result.unknowns.is_empty() || !result.next_actions.is_empty(),
                "non-success result requires unknowns or next_actions"
            );
        }
        Ok(())
    }
    fn is_descendant(agent: &AgentMetadata, parent: &str, all: &[AgentMetadata]) -> bool {
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
        self.store.write(|tx| Self::tx_stop(tx, agent_id, reason))
    }
    /// Transactional stop usable inside a tool-effect receipt, so a durable
    /// retry after an uncertain outcome replays the stored result instead of
    /// re-executing the lifecycle transition.
    pub(crate) fn tx_stop(
        tx: &crate::store::StateSession<'_>,
        agent_id: &str,
        reason: &str,
    ) -> Result<Vec<String>> {
        let root: AgentMetadata = tx
            .get_metadata("agent", agent_id)?
            .context("agent missing")?;
        let all = tx.list_metadata("agent", &root.run_id)?;
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
            tx.put_metadata("agent", &agent.id, &agent.run_id, &agent)?;
            Self::notify_terminal(tx, &agent)?;
            stopped.push(agent.id);
        }
        if root.role == Role::Root {
            let mut run: RunRecord = tx.get("run", &root.run_id)?.context("run missing")?;
            if !run.status.terminal() {
                run.status = Status::Stopped;
                run.updated_at = now();
                tx.put("run", &run.id, &run.id, &run)?;
            }
        }
        tx.event(
            &root.run_id,
            Some(agent_id),
            "agents.stopped",
            json!({"agents":stopped,"reason":reason}),
        )?;
        Ok(stopped)
    }
    pub fn pause_budget(&self, run_id: &str, reason: &str) -> Result<()> {
        self.store.write(|tx| {
            let mut run: RunRecord = tx.get("run", run_id)?.context("run missing")?;
            if run.status.terminal() {
                return Ok(());
            }
            run.status = Status::BudgetPaused;
            run.updated_at = now();
            tx.put("run", run_id, run_id, &run)?;
            for mut agent in tx.list_metadata("agent", run_id)? {
                if !agent.status.terminal() {
                    agent.status = Status::BudgetPaused;
                    agent.updated_at = now();
                    tx.put_metadata("agent", &agent.id, run_id, &agent)?;
                }
            }
            tx.event(run_id, None, "run.budget_paused", json!({"reason":reason}))?;
            Ok(())
        })
    }
    pub fn resume(&self, run_id: &str, budget: Option<Budget>) -> Result<()> {
        self.store.write(|tx| {
            let mut run: RunRecord = tx.get("run", run_id)?.context("run missing")?;
            if run.status.terminal() {
                bail!("terminal run cannot be resumed");
            }
            if let Some(budget) = budget {
                run.budget = budget;
            }
            run.status = Status::Running;
            run.updated_at = now();
            tx.put("run", run_id, run_id, &run)?;
            for mut a in tx.list_metadata("agent", run_id)? {
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
                    tx.put_metadata("agent", &a.id, run_id, &a)?;
                }
            }
            tx.event(run_id, None, "run.resumed", json!({}))?;
            Ok(())
        })
    }
}
