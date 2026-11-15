//! Shared completion rules. An execution outcome and resource cleanup are independent.
use crate::{JobState, SessionState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeCertainty {
    Known,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupState {
    Pending,
    Unconfirmed,
    Confirmed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CleanupProof {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobs_empty: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles_removed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scratch_removed: Option<bool>,
}
impl CleanupProof {
    pub fn confirmed(&self) -> bool {
        self.jobs_empty == Some(true)
            && self.profiles_removed == Some(true)
            && self.scratch_removed == Some(true)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceObservation {
    pub terminal: bool,
    pub outcome: OutcomeCertainty,
    pub cleanup: CleanupState,
}
impl ResourceObservation {
    pub fn settled(self) -> bool {
        self.terminal && self.cleanup == CleanupState::Confirmed
    }
    pub fn job(state: JobState, pending: Option<bool>, outcome: Option<OutcomeCertainty>) -> Self {
        Self {
            terminal: state.terminal(),
            outcome: outcome.unwrap_or(if state == JobState::Lost {
                OutcomeCertainty::Unknown
            } else {
                OutcomeCertainty::Known
            }),
            // v3 job receipts without the optional flag retain their existing meaning.
            cleanup: if pending == Some(true) {
                CleanupState::Pending
            } else if state.terminal() {
                CleanupState::Confirmed
            } else {
                CleanupState::Unconfirmed
            },
        }
    }
}

/// Decodes only lifecycle fields; original tool evidence stays with the caller.
#[derive(Debug, Deserialize)]
pub struct SessionObservation {
    pub state: SessionState,
    #[serde(default)]
    pub cleanup_pending: Option<bool>,
    #[serde(default)]
    pub cleanup_confirmed: Option<bool>,
    #[serde(default)]
    pub result: Option<CleanupProof>,
}
impl SessionObservation {
    pub fn observation(&self) -> ResourceObservation {
        let terminal = matches!(self.state, SessionState::Closed | SessionState::Lost);
        let confirmed = self.state == SessionState::Closed
            || (self.state == SessionState::Lost
                && (self.cleanup_confirmed == Some(true)
                    || (self.cleanup_pending == Some(false)
                        && self.result.as_ref().is_some_and(CleanupProof::confirmed))));
        ResourceObservation {
            terminal,
            outcome: if matches!(
                self.state,
                SessionState::Lost | SessionState::RecoveryRequired
            ) {
                OutcomeCertainty::Unknown
            } else {
                OutcomeCertainty::Known
            },
            cleanup: if self.cleanup_pending == Some(true) {
                CleanupState::Pending
            } else if confirmed {
                CleanupState::Confirmed
            } else {
                CleanupState::Unconfirmed
            },
        }
    }
}
