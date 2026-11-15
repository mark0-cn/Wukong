//! SQL read projections share storage internals, never desktop presentation.
use super::*;
pub(crate) mod findings;
mod narration;
pub(crate) mod report;
mod tool_snapshot;
pub(crate) mod workspace_delta;
pub(crate) mod workspace_snapshot;
