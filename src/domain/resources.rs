//! Controller-owned resource records. JSON is confined to worker payloads.
use super::*;
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentBinding {
    pub id: String,
    pub run_id: String,
    pub environment_id: String,
    pub identity: wukong_worker_protocol::WorkerIdentity,
    #[serde(default)]
    pub prior_identities: Vec<wukong_worker_protocol::WorkerIdentity>,
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
    #[serde(default)]
    pub execution_notification_message_id: Option<i64>,
    #[serde(default)]
    pub execution_status_evidence_id: Option<String>,
    #[serde(default)]
    pub evidence_sync: EvidenceSync,
    #[serde(default)]
    pub cleanup_intent: Option<CleanupIntent>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
impl JobRecord {
    /// Old complete imports remain ready without replaying either notification.
    pub fn evidence_ready(&self) -> bool {
        self.imported_artifacts.is_some() || self.evidence_sync.state == SyncState::Ready
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    #[default]
    Pending,
    Syncing,
    Retry,
    Failed,
    Ready,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EvidenceSync {
    pub state: SyncState,
    pub manifest_loaded: bool,
    pub files: Vec<SyncFile>,
    pub attempts: u32,
    pub retry_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncFile {
    pub worker_id: String,
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub evidence_id: Option<String>,
    pub error: Option<String>,
}

impl EvidenceSync {
    pub fn progress(&self, ready: bool) -> Value {
        json!({"state":if ready { SyncState::Ready } else { self.state },
            "manifest_loaded":self.manifest_loaded,"total":self.files.len(),
            "completed":self.files.iter().filter(|file|file.evidence_id.is_some()).count(),
            "error":self.error,"retry_at":self.retry_at})
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupIntent {
    pub call_id: String,
    pub agent_id: String,
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
    #[serde(default)]
    pub cleanup_intent: Option<CleanupIntent>,
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
