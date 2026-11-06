use crate::{domain::*, store::Store};
use anyhow::{Context, Result, bail};
use chrono::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

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
        let root_token_cap = budget.role_token_cap(Role::Root);
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
            status: Status::Queued,
            depth: 0,
            inherit_context: false,
            inherited_context: None,
            checkpoint: None,
            wait_for: vec![],
            wait_until: None,
            turns: 0,
            total_tokens: 0,
            cost_usd: 0.0,
            token_cap: Some(root_token_cap),
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
        self.store.transaction(|tx| {
            let parent: AgentRecord =
                Store::tx_get(tx, "agent", parent_id)?.context("parent agent missing")?;
            if parent.status.terminal() {
                bail!("parent is terminal");
            }
            let run: RunRecord =
                Store::tx_get(tx, "run", &parent.run_id)?.context("run missing")?;
            if run.status.terminal() {
                bail!("run is terminal");
            }
            let agents = Store::tx_list::<AgentRecord>(tx, "agent", &run.id)?;
            if let Some(existing) = agents.iter().find(|a| {
                a.scope.trim() == request.scope.trim()
                    && a.role == request.role
                    && !matches!(a.status, Status::Stopped | Status::Crashed | Status::Failed)
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
            let timestamp = now();
            let agent = AgentRecord {
                id: id("agent"),
                run_id: run.id.clone(),
                parent_id: Some(parent.id.clone()),
                name: request.name,
                role: request.role,
                task: request.task,
                scope: request.scope,
                skills: request.skills,
                status: Status::Queued,
                depth: parent.depth + 1,
                inherit_context: request.inherit_context,
                inherited_context: if request.inherit_context {
                    parent.checkpoint.clone()
                } else {
                    None
                },
                checkpoint: None,
                wait_for: vec![],
                wait_until: None,
                turns: 0,
                total_tokens: 0,
                cost_usd: 0.0,
                token_cap: Some(run.budget.role_token_cap(request.role)),
                summary: None,
                error: None,
                created_at: timestamp,
                updated_at: timestamp,
            };
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
        })
    }
    pub fn message(&self, sender: &str, recipient: &str, content: &str) -> Result<()> {
        if content.trim().is_empty() {
            bail!("message cannot be empty");
        }
        self.store.transaction(|tx| {
            let mut target: AgentRecord =
                Store::tx_get(tx, "agent", recipient)?.context("recipient missing")?;
            if target.status.terminal() {
                bail!("recipient is terminal; create a new focused task");
            }
            if sender != "operator" {
                let from: AgentRecord =
                    Store::tx_get(tx, "agent", sender)?.context("sender missing")?;
                if from.run_id != target.run_id {
                    bail!("cross-run message denied");
                }
            }
            Store::tx_message(tx, &target.run_id, sender, recipient, content, None)?;
            if target.status == Status::Waiting {
                target.status = Status::Queued;
                target.wait_for.clear();
                target.wait_until = None;
                target.updated_at = now();
                Store::tx_put(tx, "agent", recipient, &target.run_id, &target)?;
            }
            Store::tx_event(
                tx,
                &target.run_id,
                Some(recipient),
                "message.received",
                json!({"sender":sender}),
            )?;
            Ok(())
        })
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
                let unread:i64=tx.query_row("SELECT COUNT(*) FROM messages WHERE recipient=?1 AND acknowledged=0",[&agent.id],|r|r.get(0))?;
                if ready||expired||unread>0 {
                    if ready||expired{let key=format!("wait:{}:{}",agent.id,agent.wait_until.map(|d|d.timestamp_millis()).unwrap_or_default());Store::tx_message(tx,run_id,"coordinator",&agent.id,&json!({"type":"wait_finished","timed_out":expired&&!ready,"agents":all.iter().filter(|a|agent.wait_for.contains(&a.id)).collect::<Vec<_>>()}).to_string(),Some(&key))?;}
                    agent.status=Status::Queued;agent.wait_for.clear();agent.wait_until=None;agent.updated_at=now();Store::tx_put(tx,"agent",&agent.id,run_id,&agent)?;changed+=1;
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
            Store::tx_message(tx,&agent.run_id,&agent.id,parent,&json!({"type":"agent_finished","agent_id":agent.id,"status":agent.status,"summary":agent.summary,"error":agent.error}).to_string(),Some(&format!("terminal:{}",agent.id)))?;
        }
        Ok(())
    }
    pub fn finish(&self, agent_id: &str, summary: &str) -> Result<()> {
        if summary.trim().is_empty() {
            bail!("completion summary must be nonempty");
        }
        self.store.transaction(|tx|{
            let mut agent:AgentRecord=Store::tx_get(tx,"agent",agent_id)?.context("agent missing")?;
            if agent.status==Status::Completed{return Ok(());}
            if agent.status.terminal(){bail!("agent already terminated with {}",agent.status);}
            let unread:i64=tx.query_row("SELECT COUNT(*) FROM messages WHERE recipient=?1 AND acknowledged=0",[agent_id],|r|r.get(0))?;
            if unread>0{bail!("unread coordinator messages remain; read them at the next model boundary before finishing");}
            let all=Store::tx_list::<AgentRecord>(tx,"agent",&agent.run_id)?;
            let active:Vec<_>=all.iter().filter(|a|!a.status.terminal()&&Self::is_descendant(a,agent_id,&all)).map(|a|a.id.clone()).collect();
            if !active.is_empty(){bail!("active descendants remain: {}",active.join(", "));}
            if agent.role==Role::Root {
                let unresolved=Store::tx_list::<Coverage>(tx,"coverage",&agent.run_id)?.into_iter().filter(|c|c.outcome=="needs_follow_up").map(|c|c.id).collect::<Vec<_>>();
                if !unresolved.is_empty(){bail!("coverage needs follow-up; resolve or mark inconclusive: {}",unresolved.join(", "));}
            }
            agent.status=Status::Completed;agent.summary=Some(summary.to_owned());agent.updated_at=now();Store::tx_put(tx,"agent",agent_id,&agent.run_id,&agent)?;Self::notify_terminal(tx,&agent)?;
            if agent.role==Role::Root {let mut run:RunRecord=Store::tx_get(tx,"run",&agent.run_id)?.context("run missing")?;run.status=Status::Completed;run.summary=Some(summary.to_owned());run.updated_at=now();Store::tx_put(tx,"run",&run.id,&run.id,&run)?;}
            Store::tx_event(tx,&agent.run_id,Some(agent_id),"agent.completed",json!({"summary":summary}))?;Ok(())
        })
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
        self.store.transaction(|tx| {
            let root: AgentRecord =
                Store::tx_get(tx, "agent", agent_id)?.context("agent missing")?;
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
        })
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
                if !a.status.terminal() {
                    // Role caps follow the run budget: a resumed run with an
                    // enlarged budget rescales every surviving agent's quota.
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
