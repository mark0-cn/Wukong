//! Launch preparation shares the worker LaunchSpec used by jobs and sessions.
use super::*;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::application::tools) enum LaunchSource {
    Primary,
    Bundle {
        /// Explicit entry member; omission uses the inferred entry.
        entry: Option<String>,
        /// Subset including entry; omission uses all members (at most 64).
        #[schemars(length(min = 1, max = 64))]
        members: Option<Vec<String>>,
    },
    Staged {
        target: String,
        #[serde(default)]
        dependencies: Vec<LaunchMember>,
    },
    LinuxRuntime {
        target: String,
        libc: String,
        loader: String,
    },
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LaunchMember {
    pub path: String,
    /// Relative layout inside the immutable launch directory.
    pub name: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct LaunchPrepareArgs {
    pub source: LaunchSource,
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    pub input_mode: LaunchInputMode,
    #[schemars(range(min = 1, max = 3600))]
    pub timeout_secs: u64,
}
choices!(LaunchInputMode {
    StdinPipe,
    ArgvFile
});
