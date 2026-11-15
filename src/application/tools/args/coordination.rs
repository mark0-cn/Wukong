//! Coordination tool input schemas.
use super::*;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct CreateAgentArgs {
    #[schemars(length(min = 1))]
    pub name: String,
    /// Analyst performs the work; verifier independently checks another agent's claim.
    pub role: ChildRole,
    #[schemars(length(min = 1))]
    pub task: String,
    #[schemars(length(min = 1))]
    pub scope: String,
    /// Registered environment to bind to this child; omitted inherits the parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional capabilities, such as recon, reverse, hunter, fuzz, crash, exploit or reporter.
    /// These supplement the assigned role and never change its permissions or lifecycle.
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Defaults to false: use the child's brief and evidence references. Explicit opt-in only.
    pub inherit_context: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_for_predecessors: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_policy: Option<DependencyPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<TaskContract>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct ViewAgentGraphArgs {
    /// Full agent ID (or unambiguous prefix) for that agent's detailed contract/result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// Summary page size, default 8 and maximum 16.
    #[schemars(range(min = 1, max = 16))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct SendMessageToAgentArgs {
    #[schemars(length(min = 1))]
    pub agent_id: String,
    #[schemars(length(min = 1))]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<InformationKind>,
    #[schemars(range(min = 1, max = 9223372036854775807_u64))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct FollowUpAgentArgs {
    #[schemars(length(min = 1))]
    pub agent_id: String,
    #[schemars(length(min = 1))]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ids: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct MessageStatusArgs {
    #[schemars(range(min = 1, max = 9223372036854775807_u64))]
    pub message_id: u64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct WaitForAgentsArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_ids: Option<Vec<String>>,
    #[schemars(range(min = 1, max = 3600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<DependencyPolicy>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct StopAgentArgs {
    #[schemars(length(min = 1))]
    pub agent_id: String,
    #[schemars(length(min = 1))]
    pub reason: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct AgentFinishArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct FinishScanArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LoadSkillArgs {
    #[schemars(length(min = 1))]
    pub name: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct ThinkArgs {
    #[schemars(length(min = 1))]
    pub content: String,
}
