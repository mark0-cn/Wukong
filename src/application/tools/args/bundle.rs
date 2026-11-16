//! Inputs for directory bundle tools.
use super::*;

#[derive(Serialize, Deserialize, JsonSchema)]
pub(in crate::application::tools) struct BundleEmptyArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
pub(in crate::application::tools) struct BundleMemberArgs {
    /// Member path relative to the analyzed directory (forward slashes).
    pub path: String,
}
