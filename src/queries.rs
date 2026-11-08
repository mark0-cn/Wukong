//! Read-only application queries and stable frontend/report data transfer objects.
use crate::{domain::*, store::Store};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunSummary {
    pub id: String,
    pub target_path: PathBuf,
    pub target_sha256: String,
    pub task: String,
    pub status: Status,
    pub root_id: String,
    pub budget: Budget,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub parent_id: Option<String>,
    pub scope: String,
    pub generation: u64,
    pub task: String,
    pub status: Status,
    pub depth: u32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub turns: usize,
    pub total_tokens: u64,
    pub summary: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub public_replies: Vec<String>,
}

impl From<AgentMetadata> for AgentSummary {
    fn from(agent: AgentMetadata) -> Self {
        Self {
            id: agent.id,
            name: agent.name,
            role: agent.role,
            parent_id: agent.parent_id,
            scope: agent.scope,
            generation: agent.generation,
            task: agent.task,
            status: agent.status,
            depth: agent.depth,
            created_at: agent.created_at,
            turns: agent.turns,
            total_tokens: agent.total_tokens,
            summary: agent.summary,
            error: agent.error,
            public_replies: vec![],
        }
    }
}

#[derive(Serialize)]
pub struct Change<T> {
    pub id: String,
    pub value: Option<T>,
}

#[derive(Serialize)]
pub struct WorkspaceDelta {
    pub selected: Option<String>,
    pub reset: bool,
    pub runs: Vec<Change<RunSummary>>,
    pub agents: Vec<Change<AgentSummary>>,
    pub notes: Vec<Change<SharedDocument>>,
    pub findings: Vec<Change<Finding>>,
    pub events: Option<Vec<Event>>,
    pub calls: Option<Vec<ToolCallRecord>>,
    pub narration_snapshots: Vec<AgentSnapshot>,
    pub conversation: Option<Option<AgentSnapshot>>,
}

#[derive(Default)]
pub struct WorkspaceQueries {
    pub(crate) root: PathBuf,
    pub(crate) revision: i64,
    pub(crate) selected: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) events: Vec<Event>,
    pub(crate) event_limit: usize,
    pub(crate) conversation_revision: Option<i64>,
    pub(crate) calls: BTreeMap<String, ToolCallRecord>,
}

#[derive(Serialize)]
pub struct ReportSnapshot {
    pub run: RunRecord,
    pub agents: Vec<AgentSnapshot>,
    pub findings: Vec<Finding>,
    pub verifications: Vec<Verification>,
    pub coverage: Vec<Coverage>,
    pub evidence: Vec<Evidence>,
    pub calls: Vec<ToolCallRecord>,
    pub documents: Vec<SharedDocument>,
    pub jobs: Vec<Value>,
    pub worker_files: Vec<Value>,
    pub debug_sessions: Vec<Value>,
    pub events: Vec<Event>,
    pub messages: Vec<Message>,
    pub task_results: Vec<Value>,
}

pub struct ReportQueries<'a> {
    store: &'a Store,
}
impl<'a> ReportQueries<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }
    pub fn snapshot(&self, run: &str) -> Result<ReportSnapshot> {
        crate::store::report_queries::snapshot(self.store, run)
    }
}
#[derive(Serialize)]
pub struct RunStatus {
    pub run: RunSummary,
    pub agents: Vec<AgentSummary>,
    pub events: Vec<Event>,
}
pub struct AssessmentQueries<'a> {
    pub(crate) store: &'a Store,
}
impl<'a> AssessmentQueries<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_and_workspace_share_summary_contract_without_saved_configuration() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let target = directory.path().join("sample");
        std::fs::write(&target, b"fixture")?;
        let store = Store::open(directory.path().join("state"))?;
        let run = crate::coordinator::Coordinator::new(store.clone()).create_run(
            &target,
            "fixture",
            Budget::default(),
            serde_json::json!({"internal_setting":"excluded"}),
        )?;
        let status = AssessmentQueries::new(&store).status(&run.id, 5)?;
        let workspace = WorkspaceQueries::default().read(&store, Some(&run.id), None, 5)?;
        assert_eq!(status.run.id, workspace.runs[0].value.as_ref().unwrap().id);
        assert_eq!(
            status.agents[0].id,
            workspace.agents[0].value.as_ref().unwrap().id
        );
        assert!(serde_json::to_value(status)?["run"].get("config").is_none());
        assert_eq!(ReportQueries::new(&store).snapshot(&run.id)?.run.id, run.id);
        Ok(())
    }
}
