//! Inputs for directory bundle tools.
use super::*;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct BundleEmptyArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct BundleStageArgs {
    /// Explicit member to execute; omission uses the inferred entry.
    pub entry: Option<String>,
    /// Member subset including entry (1 through 64); omission selects all.
    pub members: Option<Vec<String>>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Relative working directory; omission uses the bundle root.
    pub cwd: Option<String>,
    pub input_mode: Option<BundleInputMode>,
    /// LaunchSpec-owned process timeout; defaults to 300 seconds.
    #[schemars(range(min = 1, max = 3600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct BundleMemberArgs {
    /// Member path relative to the analyzed directory (forward slashes).
    pub path: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(in crate::application::tools) enum BundleInputMode {
    StdinPipe,
    ArgvFile,
}
