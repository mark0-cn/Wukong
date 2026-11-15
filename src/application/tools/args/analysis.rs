//! Analysis tool input schemas.
use super::*;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub(in crate::application::tools) enum StaticAddress {
    /// Prefer copying the complete module-relative AddressRef from binary_inspect.
    Reference(crate::adapters::binary::AddressRef),
    /// Legacy byte offset relative to image_base, never a VA, file offset or page number.
    Offset(u64),
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct BinaryInspectArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct BinaryStringsArgs {
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
pub(in crate::application::tools) struct FunctionsSearchArgs {
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
pub(in crate::application::tools) struct FunctionDecompileArgs {
    /// Optional same-run binary evidence for a baseline/secondary target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_evidence_id: Option<String>,
    pub address: StaticAddress,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct XrefsQueryArgs {
    /// Optional same-run binary evidence for a baseline/secondary target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_evidence_id: Option<String>,
    pub address: StaticAddress,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    #[schemars(range(min = 1, max = 5000))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct AnalysisVerifyArgs {
    pub producer_id: String,
    pub claim: String,
    pub scope: String,
    pub expected: String,
    pub observed: String,
    pub verdict: AnalysisVerdict,
    pub evidence_ids: Vec<String>,
    pub counterexamples: Vec<String>,
    /// Individually tested claims; supported applies only to these checks.
    pub checks: Vec<AnalysisCheck>,
    /// Untested claims, boundary cases and environment limitations (empty if none).
    pub limitations: Vec<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct ArtifactValidateArgs {
    pub artifact_evidence_id: String,
    /// Verifier-owned baseline.json imported from the real --verify job's artifacts.
    pub baseline_evidence_id: String,
    pub baseline_job_id: String,
    /// python3 or bash; other interpreters can implement the same stdin JSON protocol.
    pub interpreter: String,
    /// Original arguments after the script name; stage file arguments explicitly.
    #[serde(default)]
    pub arguments: Vec<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct AnalysisCheck {
    pub claim: String,
    pub expected: String,
    pub observed: String,
    pub verdict: AnalysisVerdict,
    pub evidence_ids: Vec<String>,
    /// The exact report/script evidence being checked, if any.
    pub artifact_evidence_id: Option<String>,
    /// Required for a reproduction check; links the verifier's executed argv.
    pub reproduction_job_id: Option<String>,
    /// Expected process exit status for reproduction checks (including negative controls).
    pub expected_exit_code: Option<i64>,
    pub kind: AnalysisCheckKind,
}

#[derive(Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(in crate::application::tools) enum AnalysisCheckKind {
    Behavior,
    Algorithm,
    Boundary,
    Reproduction,
    /// Independently inspect that real --verify and --validation-io share the same evaluator.
    ValidatorIntegration,
    Static,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct FunctionInstructionsArgs {
    pub address: StaticAddress,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
    pub target_evidence_id: Option<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct DifferentialTestArgs {
    /// In-run worker executable paths; reference may be a staged Python script.
    pub target: String,
    pub reference: String,
    pub reference_python: bool,
    /// At most 64 base64 stdin vectors, each at most 64 KiB decoded.
    pub inputs_base64: Vec<String>,
    pub case_timeout_seconds: u64,
}
