//! Analysis tool input schemas.
use super::*;

pub(in crate::application::tools) type StaticAddress = crate::adapters::binary::AddressRef;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::application::tools) enum TargetRef {
    Primary,
    Evidence { evidence_id: String },
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct BinaryInspectArgs {
    pub target_ref: Option<TargetRef>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct BinaryStringsArgs {
    pub target_ref: Option<TargetRef>,
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
    pub target_ref: Option<TargetRef>,
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
    pub target_ref: Option<TargetRef>,
    pub address: StaticAddress,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct XrefsQueryArgs {
    /// Optional same-run binary evidence for a baseline/secondary target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<TargetRef>,
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
    /// Interpreter for the self_check_v1 protocol.
    pub interpreter: ArtifactInterpreter,
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
    #[schemars(range(min = 1, max = 5000))]
    pub limit: Option<u64>,
    pub target_ref: Option<TargetRef>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct DifferentialTestArgs {
    pub target_launch_spec: String,
    pub reference_launch_spec: String,
    #[schemars(length(min = 1, max = 8))]
    pub input_evidence_ids: Vec<String>,
    pub comparison: ComparisonRule,
    /// Process exits admitted as observations, usually [0]. Others are inconclusive.
    #[schemars(length(min = 1, max = 8))]
    pub expected_exit_codes: Vec<i64>,
}
choices!(ComparisonRule {
    ExactOutput,
    ExitStatus,
    StdoutJson
});

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(in crate::application::tools) enum ArtifactInterpreter {
    Python3,
    Bash,
}
impl ArtifactInterpreter {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Python3 => "python3",
            Self::Bash => "bash",
        }
    }
}
