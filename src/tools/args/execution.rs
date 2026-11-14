//! Execution tool input schemas.
use super::*;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct RuntimeDoctorArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct EnvironmentDoctorArgs {
    pub environment_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct StageTargetArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct ArtifactStageArgs {
    pub artifact_id: String,
    #[serde(default)]
    pub executable: bool,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct FilePutArgs {
    #[schemars(length(min = 1))]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<bool>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct FileGetArgs {
    #[schemars(length(min = 1))]
    pub path: String,
    #[schemars(range(min = 1, max = 17179869184_u64))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

pub(in crate::tools) use wukong_worker_protocol::params::{
    AngrSolveArgs, CrashReplayArgs, DebugCloseArgs, DebugCommandArgs, DebugControlArgs,
    DebugInspectArgs, DebugStartArgs, EmulateRunArgs, ExecCommandArgs, FuzzStartArgs,
    FuzzStatusArgs, InputMinimizeArgs, InteractiveCloseArgs, InteractiveSendArgs,
    InteractiveStartArgs, JobArtifactsArgs, JobCancelArgs, JobStartArgs, JobStatusArgs,
    LibcIdentifyArgs, NetworkReplay, PocVerifyArgs, PyRunArgs, SeccompAnalyzeArgs,
};

pub(in crate::tools) use wukong_worker_protocol::params::RuntimeProfileArgs;

pub(in crate::tools) use wukong_worker_protocol::params::CrashSnapshotArgs;
pub(in crate::tools) use wukong_worker_protocol::params::CyclicOffsetArgs;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct MitigationProfileArgs {
    pub target: String,
    pub session_id: Option<String>,
    pub interactive_session_id: Option<String>,
    /// Optional runtime address: report only the actual containing mapping's permissions.
    pub address: Option<u64>,
}
