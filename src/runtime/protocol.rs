//! Typed worker boundary. Unknown result fields are retained for raw evidence.
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    pub(crate) fn validate(
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

pub use crate::domain::{JobResult, JobState, JobStatus, SessionHandle, SessionState};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkerErrorKind {
    InvalidRequest,
    SessionLost,
    Conflict,
    BindingChanged,
    LeaseLost,
    OutcomeUnknown,
    Internal,
    #[default]
    Unknown,
}

#[derive(Debug)]
pub struct WorkerError {
    pub kind: WorkerErrorKind,
    pub message: String,
}
impl std::fmt::Display for WorkerError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{}", self.message)
    }
}
impl std::error::Error for WorkerError {}
impl WorkerError {
    pub(crate) fn from_rpc(message: String, data: Option<&str>) -> Self {
        let kind = data
            .and_then(|data| serde_json::from_str::<Value>(data).ok())
            .and_then(|data| serde_json::from_value(data["kind"].clone()).ok())
            .unwrap_or_default();
        Self { kind, message }
    }
}

#[derive(Debug)]
pub struct TransportFailure {
    pub timed_out: bool,
    pub message: String,
}
impl std::fmt::Display for TransportFailure {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{}", self.message)
    }
}
impl std::error::Error for TransportFailure {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_keeps_nested_results_and_treats_lost_as_terminal() {
        let status: JobStatus=serde_json::from_value(serde_json::json!({"job_id":"id","state":"lost","result":{"exit_code":7,"passed":false,"attempts":2}})).unwrap();
        assert!(status.state.terminal());
        assert_eq!(status.result.as_ref().unwrap().exit_code, Some(7));
        assert_eq!(
            serde_json::to_value(status).unwrap()["result"]["attempts"],
            2
        );
        assert_eq!(
            WorkerError::from_rpc(
                "different localized message".into(),
                Some(r#"{"kind":"session_lost"}"#)
            )
            .kind,
            WorkerErrorKind::SessionLost
        );
    }
}
