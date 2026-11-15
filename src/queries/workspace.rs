//! A single bounded storage snapshot, independent of desktop presentation.
use super::models::*;
use crate::domain::*;
use serde::Deserialize;
#[derive(Clone, Default, Deserialize)]
pub struct WorkspaceRequest {
    pub run: Option<String>,
    pub agent: Option<String>,
    pub event_limit: Option<usize>,
    pub known_revision: Option<String>,
}
pub(crate) struct WorkspaceSnapshotReply {
    pub revision: String,
    pub data: Option<WorkspaceSnapshot>,
}
#[derive(Default)]
pub(crate) struct WorkspaceSnapshot {
    pub selected: Option<String>,
    pub runs: Vec<RunSummary>,
    pub agents: Vec<AgentSummary>,
    pub findings: Vec<Finding>,
    pub notes: Vec<SharedDocument>,
    pub events: Vec<Event>,
    pub calls: Vec<ToolCallRecord>,
    pub jobs: Vec<JobRecord>,
    pub operator_messages: Vec<Message>,
    pub replies: Vec<PublicReply>,
    pub target_name: String,
    pub controller_live: bool,
    pub has_earlier: bool,
}
