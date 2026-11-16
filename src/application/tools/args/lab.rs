//! Inputs for the controller-side Windows lab tools.
use super::*;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LabStatusArgs {
    /// Registered windows_lab VM id.
    pub vm: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LabPrepareArgs {
    pub vm: String,
    pub purpose: String,
    pub spec: AgentExperimentSpec,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LabRunArgs {
    pub vm: String,
    pub lab_id: String,
    pub generation: u64,
    /// Artifact name inside the verified package, e.g. harness.exe.
    pub artifact: String,
    /// Optional bounded stdin content evidence id (at most 1 MiB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_evidence_id: Option<String>,
    #[schemars(range(min = 1, max = 600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LabInstallArgs {
    pub vm: String,
    pub lab_id: String,
    pub generation: u64,
    #[schemars(range(min = 1, max = 600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LabRestoreArgs {
    pub vm: String,
    pub lab_id: String,
    pub generation: u64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LabPruneArgs {
    pub vm: String,
}

/// Evidence references are resolved inside the admitted assessment; hashes and
/// controller paths are computed by the service, never supplied by the model.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct AgentExperimentSpec {
    pub target_evidence_id: Option<String>,
    pub launch: crate::application::windows_lab::LaunchKind,
    pub input_channel: crate::application::windows_lab::InputChannel,
    pub account: crate::application::windows_lab::AccountContext,
    #[serde(default)]
    pub annotations: crate::application::windows_lab::ExperimentAnnotations,
    #[serde(default)]
    pub dependencies: Vec<LabDependency>,
    pub install: Option<crate::application::windows_lab::install::InstallPlan>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LabDependency {
    pub name: String,
    pub evidence_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LabOperationArgs {
    pub operation_id: String,
}
