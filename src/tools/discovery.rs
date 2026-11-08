//! Tool visibility is Strix-style: every agent sees the full catalog for its
//! role upfront, and no model turn is spent discovering or loading tools. The
//! only per-role difference is the lifecycle tool: `finish_scan` for the root
//! and `agent_finish` for children.
use super::*;
use std::collections::BTreeSet;

impl ToolService {
    /// Native task/message/wait are supplied separately by SubagentsPlugin.
    pub(crate) fn model_tool_names(&self, agent: &AgentMetadata) -> Result<BTreeSet<String>> {
        Ok(self
            .definitions(agent.role)
            .into_iter()
            .filter(|definition| definition.name.as_str() != "create_agent")
            .map(|definition| definition.name.to_string())
            .collect())
    }
}
