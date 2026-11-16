//! Inputs for the controller-side Windows lab tools.
use super::*;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LabStatusArgs {
    /// Registered windows_lab VM id; omit to list configured lab VMs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vm: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LabPrepareArgs {
    pub vm: String,
    pub purpose: String,
    /// ExperimentSpec JSON, including dependencies and the optional install plan.
    pub spec: Value,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LabRunArgs {
    /// Opaque reference returned by lab_prepare; bound to this run, agent and lease generation.
    pub lab_ref: String,
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
    /// Opaque reference returned by lab_prepare; bound to this run, agent and lease generation.
    pub lab_ref: String,
    #[schemars(range(min = 1, max = 600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LabRestoreArgs {
    /// Opaque reference returned by lab_prepare; bound to this run, agent and lease generation.
    pub lab_ref: String,
}
