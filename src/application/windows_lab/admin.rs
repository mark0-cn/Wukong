//! Administrative channel used only for offline service/driver installation.
use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize, Clone, Debug)]
pub struct AdminFile {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Serialize, Clone, Debug)]
pub struct AdminRequest {
    pub install_id: String,
    pub media_sha256: String,
    pub files: Vec<AdminFile>,
    pub timeout_secs: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct AdminOutcome {
    pub exit_code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    pub guest_media: String,
    pub guest_script: String,
    pub settled: bool,
}
#[async_trait]
pub trait LabAdmin: Send + Sync {
    async fn install(&self, request: AdminRequest) -> Result<AdminOutcome>;
    async fn finish(&self) {}
}
