//! Shared public JSON-RPC contract. Transport framing belongs to the transport libraries.
use serde::{Deserialize, Serialize};

pub const PROTOCOL: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerIdentity {
    pub protocol: u32,
    pub installation_id: String,
    pub boot_id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

pub fn starts_resource(method: &str) -> bool {
    matches!(
        method,
        "job.start"
            | "debug.start"
            | "interact.start"
            | "exec.run"
            | "py.run"
            | "crash.replay"
            | "input.minimize"
            | "poc.verify"
            | "libc.identify"
            | "seccomp.analyze"
            | "cyclic.offset"
            | "emu.run"
    )
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
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
