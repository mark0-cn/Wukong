//! Jobs systems for the Rig adapter.
use super::*;

/// Terminal, delivered-but-not-yet-injected job results for one agent, each
/// marked injected once its brief enters the prompt.
pub(super) fn collect_job_briefs(store: &Store, agent: &AgentRecord) -> Result<Vec<Value>> {
    crate::tools::JobService::briefs(store, agent)
}
