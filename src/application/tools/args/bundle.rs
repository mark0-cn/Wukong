//! Inputs for directory bundle tools.
use super::*;

#[derive(Serialize, Deserialize, JsonSchema)]
pub(in crate::application::tools) struct BundleEmptyArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
pub(in crate::application::tools) struct BundleStageArgs {
    /// LaunchSpec-owned process timeout; defaults to 300 seconds.
    #[schemars(range(min = 1, max = 3600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub(in crate::application::tools) struct BundleMemberArgs {
    /// Member path relative to the analyzed directory (forward slashes).
    pub path: String,
}
