//! Controller-owned resource records. JSON is confined to worker payloads.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentBinding {
    pub id: String,
    pub run_id: String,
    pub environment_id: String,
    pub identity: crate::runtime::protocol::WorkerIdentity,
}
pub use wukong_worker_protocol::{JobResult, JobState, JobStatus, SessionHandle, SessionState};

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
