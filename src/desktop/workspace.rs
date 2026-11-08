use super::presentation::ChatMessage;
use crate::{domain::*, queries::*, store::Store};
use serde::{Deserialize, Serialize};
#[derive(Clone, Default, Deserialize)]
pub struct WorkspaceRequest {
    pub run: Option<String>,
    pub agent: Option<String>,
    pub event_limit: Option<usize>,
    pub known_revision: Option<String>,
}
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
    crate::store::desktop_queries::read(store, epoch, request)
}
