//! Shared public JSON-RPC contract. Transport framing belongs to the transport libraries.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod resources;
pub use resources::*;
mod responses;
pub use responses::*;
mod methods;
pub mod params;
pub use methods::*;

pub const PROTOCOL: u32 = 3;
pub const MAX_RPC_BYTES: usize = 48 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerIdentity {
    #[schemars(range(min = 3, max = 3))]
    pub protocol: u32,
    #[schemars(length(min = 1, max = 256))]
    pub installation_id: String,
    #[schemars(length(min = 1, max = 256))]
    pub boot_id: String,
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub package_sha256: String,
}

impl WorkerIdentity {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.protocol == PROTOCOL, "unsupported worker protocol");
        anyhow::ensure!(
            [&self.installation_id, &self.boot_id]
                .iter()
                .all(|value| !value.is_empty() && value.len() <= 256),
            "invalid worker identity"
        );
        anyhow::ensure!(
            self.package_sha256.len() == 64
                && self
                    .package_sha256
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "invalid worker package identity"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentLease {
    pub epoch: u64,
    pub run_id: String,
    pub owner_id: String,
    pub boot_id: String,
    pub token: String,
    pub expires_at: f64,
}

impl EnvironmentLease {
    pub fn validate(
        &self,
        run: &str,
        owner: &str,
        identity: &WorkerIdentity,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.run_id == run
                && self.owner_id == owner
                && self.boot_id == identity.boot_id
                && self.epoch > 0
                && self.token.len() == 32
                && self.token.bytes().all(|c| c.is_ascii_hexdigit())
                && self.expires_at.is_finite()
                && self.expires_at > 0.0,
            "worker returned an invalid environment lease"
        );
        Ok(())
    }
}

impl EnvironmentLease {
    pub fn same_fence(&self, other: &Self) -> bool {
        self.epoch == other.epoch
            && self.run_id == other.run_id
            && self.owner_id == other.owner_id
            && self.token == other.token
            && self.boot_id == other.boot_id
    }
}

/// Limits advertised by the worker, independent of controller Agent concurrency.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerCapacity {
    pub max_active_resources: usize,
    pub exclusive_run_lease: bool,
}

/// Explicit pre-effect rejection: this is the only execution error safe to queue/retry.
#[derive(Debug)]
pub struct CapacityBusy;
impl std::fmt::Display for CapacityBusy {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("worker execution capacity is occupied; no new effect was admitted")
    }
}
impl std::error::Error for CapacityBusy {}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkerErrorKind {
    InvalidRequest,
    SessionLost,
    Conflict,
    ResourceBusy,
    BindingChanged,
    LeaseLost,
    OutcomeUnknown,
    Internal,
    #[default]
    Unknown,
}

/// Authentication and request identity are separate from operation parameters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(rename = "_binding", default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<WorkerIdentity>,
    #[serde(rename = "_lease", default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<EnvironmentLease>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkerErrorData {
    pub kind: WorkerErrorKind,
}
