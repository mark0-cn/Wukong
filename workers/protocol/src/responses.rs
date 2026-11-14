//! Structured transport observations. Third-party evidence remains separately extensible.
use crate::{FileReceipt, FileReference, RegisteredMethods, WorkerCapacity, WorkerIdentity};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PingResult {
    #[schemars(range(min = 3, max = 3))]
    pub protocol: u32,
    pub identity: WorkerIdentity,
    pub daemon_id: String,
    pub daemon_pid: u32,
    pub state_dir: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DoctorResult {
    #[schemars(range(min = 3, max = 3))]
    pub protocol: u32,
    pub identity: WorkerIdentity,
    pub os: String,
    pub isolation: String,
    pub exclusive_run_lease: bool,
    pub methods: RegisteredMethods,
    pub capacity: Option<WorkerCapacity>,
    pub capabilities: BTreeMap<String, CapabilityValue>,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum CapabilityValue {
    Available(bool),
    Formats(Vec<String>),
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct LeaseReleased {
    pub released: bool,
    pub epoch: u64,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct FileContents {
    #[serde(flatten)]
    pub file: FileReference,
    pub data_base64: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileChunk {
    pub path: String,
    pub offset: u64,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub sha256: Option<String>,
    pub data_base64: String,
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub chunk_sha256: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TransferStarted {
    pub transfer_id: String,
    #[schemars(range(min = 1, max = 1048576))]
    pub chunk_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferStatus {
    pub transfer_id: String,
    pub offset: u64,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<FileReceipt>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ShutdownResult {
    pub state: ShutdownState,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownState {
    Shutdown,
}
