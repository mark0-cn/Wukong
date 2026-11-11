//! Controller-owned resource records. JSON is confined to worker payloads.
use super::*;
use std::collections::BTreeMap;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Lost,
}
impl JobState {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut | Self::Lost
        )
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatus {
    pub job_id: String,
    pub state: JobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<JobResult>,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Running,
    Exited,
    Closed,
    Lost,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionHandle {
    pub session_id: String,
    pub state: SessionState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub worker_resource_id: String,
    pub run_id: String,
    pub agent_id: String,
    pub worker_id: String,
    pub owner_generation: u64,
    pub tool_name: String,
    pub finding_id: Option<String>,
    pub call_id: String,
    pub worker_request_id: String,
    pub parameters: Value,
    pub target_sha256: String,
    pub status: JobStatus,
    pub status_evidence_id: Option<String>,
    pub imported_artifacts: Option<Value>,
    pub notification_message_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub worker_resource_id: String,
    pub run_id: String,
    pub agent_id: String,
    pub worker_id: String,
    pub state: SessionState,
    pub worker: Option<Value>,
    pub transcript_evidence_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerFileRecord {
    pub id: String,
    pub run_id: String,
    pub agent_id: String,
    pub worker_id: String,
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub evidence_id: Option<String>,
    pub created_at: DateTime<Utc>,
}
