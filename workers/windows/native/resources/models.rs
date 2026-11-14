//! Immutable resource identity and lifecycle observations; no storage implementation.
use crate::artifacts::inputs::Owner;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Process,
    Debugger,
    Interactive,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationKind {
    DebugControl,
    InteractiveWrite,
}

/// Persisted intent for native recovery, not evidence that its OS objects exist.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRecord {
    pub id: String,
    pub phase: Phase,
    pub start_effect: String,
    pub owner: Owner,
    pub kind: ResourceKind,
    pub specification: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Preparing,
    Live,
    Closing,
    Closed,
}
impl Phase {
    pub(crate) fn read(value: &str) -> Result<Self> {
        match value {
            "preparing" => Ok(Self::Preparing),
            "live" => Ok(Self::Live),
            "closing" => Ok(Self::Closing),
            "closed" => Ok(Self::Closed),
            _ => anyhow::bail!("invalid native resource phase"),
        }
    }
}
