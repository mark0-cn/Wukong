//! Controller-to-worker execution contract for one packaged harness.
//!
//! The lab owns admission, receipts and cleanup records. A worker only uploads
//! the bytes, starts one bounded job and reports the observed terminal state.
use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone, Debug)]
pub struct LabWorkerRequest {
    pub run_id: String,
    pub owner: String,
    pub artifact_name: String,
    pub executable: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdin: Option<Vec<u8>>,
    pub timeout_secs: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct LabWorkerOutcome {
    pub environment_id: String,
    pub installation_id: String,
    pub job_id: String,
    pub state: String,
    pub settled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_path: Option<String>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
}

#[async_trait]
pub trait LabWorker: Send + Sync {
    async fn execute(&self, request: LabWorkerRequest) -> Result<LabWorkerOutcome>;
    async fn doctor(&self) -> Result<Value> {
        anyhow::bail!("worker readiness probe is not configured")
    }
    async fn maintenance(&self) -> Result<Box<dyn LabMaintenance>> {
        Ok(Box::new(()))
    }
    async fn observe(&self, _job_id: &str) -> Result<Option<LabWorkerOutcome>> {
        Ok(None)
    }
}

/// A maintenance scope owns its heartbeat and scoped release, never registry shutdown.
#[async_trait]
pub trait LabMaintenance: Send + Sync {
    async fn finish(self: Box<Self>) -> Result<()>;
}
#[async_trait]
impl LabMaintenance for () {
    async fn finish(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}
