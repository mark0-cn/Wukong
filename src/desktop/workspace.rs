use super::presentation::{self, ChatMessage};
pub use crate::queries::WorkspaceRequest;
use crate::queries::workspace::WorkspaceSnapshot;
use crate::{domain::*, queries::*, store::Store};
use serde::Serialize;
#[derive(Serialize)]
pub struct WorkspaceData {
    pub selected: Option<String>,
    pub runs: Vec<RunSummary>,
    pub agents: Vec<AgentSummary>,
    pub findings: Vec<Finding>,
    pub notes: Vec<SharedDocument>,
    pub transcript: Vec<ChatMessage>,
    pub active_tools: Vec<super::presentation::ToolDisplay>,
    pub has_earlier: bool,
}
#[derive(Serialize)]
pub struct WorkspaceResponse {
    pub revision: String,
    pub data: Option<WorkspaceData>,
}
pub fn read(
    store: &Store,
    epoch: u64,
    request: &WorkspaceRequest,
) -> anyhow::Result<WorkspaceResponse> {
    let snapshot = crate::store::desktop_queries::read(store, epoch, request)?;
    Ok(WorkspaceResponse {
        revision: snapshot.revision,
        data: snapshot.data.map(present),
    })
}

fn present(snapshot: WorkspaceSnapshot) -> WorkspaceData {
    let active = snapshot
        .calls
        .iter()
        .filter_map(|call| {
            let agent = snapshot
                .agents
                .iter()
                .find(|agent| agent.id == call.agent_id);
            let display = presentation::with_job(
                presentation::tool_display(call, agent, &snapshot.target_name),
                call,
                agent,
                &snapshot.jobs,
            );
            (matches!(display.state.as_str(), "queued" | "running" | "uncertain")
                && display.job.as_ref().is_none_or(|job| !job.stale))
            .then_some(display)
        })
        .collect();

    let transcript = presentation::transcript(
        &snapshot.events,
        &snapshot.calls,
        &snapshot.agents,
        &snapshot.operator_messages,
        &presentation::TranscriptState {
            target_name: &snapshot.target_name,
            jobs: &snapshot.jobs,
            replies: &snapshot.replies,
            controller_live: snapshot.controller_live,
        },
    );
    WorkspaceData {
        selected: snapshot.selected,
        runs: snapshot.runs,
        agents: snapshot.agents,
        findings: snapshot.findings,
        notes: snapshot.notes,
        transcript,
        active_tools: active,
        has_earlier: snapshot.has_earlier,
    }
}
#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
