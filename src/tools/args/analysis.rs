//! Analysis tool input schemas.
use super::*;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct BinaryInspectArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct BinaryStringsArgs {
    #[schemars(range(min = 1, max = 65536))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,
    #[schemars(range(min = 1, max = 65536))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[schemars(range(min = 1, max = 5000))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct FunctionsSearchArgs {
    /// Optional same-run binary evidence for a baseline/secondary target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_evidence_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[schemars(range(min = 1, max = 5000))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct FunctionDecompileArgs {
    /// Optional same-run binary evidence for a baseline/secondary target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_evidence_id: Option<String>,
    pub address: u64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct XrefsQueryArgs {
    /// Optional same-run binary evidence for a baseline/secondary target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_evidence_id: Option<String>,
    pub address: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    #[schemars(range(min = 1, max = 5000))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct AnalysisVerifyArgs {
    pub producer_id: String,
    pub claim: String,
    pub scope: String,
    pub expected: String,
    pub observed: String,
    pub verdict: AnalysisVerdict,
    pub evidence_ids: Vec<String>,
    pub counterexamples: Vec<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct FunctionInstructionsArgs {
    pub address: u64,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
    pub target_evidence_id: Option<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools) struct DifferentialTestArgs {
    /// In-run worker executable paths; reference may be a staged Python script.
    pub target: String,
    pub reference: String,
    pub reference_python: bool,
    /// At most 64 base64 stdin vectors, each at most 64 KiB decoded.
    pub inputs_base64: Vec<String>,
    pub case_timeout_seconds: u64,
}
