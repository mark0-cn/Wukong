//! Typed tool inputs. Rig derives the model schema from these same types.
use crate::domain::{DependencyPolicy, FindingStage, Severity, TaskContract, TaskResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

macro_rules! choices {
    ($name:ident { $($variant:ident),* $(,)? }) => {
        #[derive(Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub(super) enum $name { $($variant),* }
    };
}
choices!(ChildRole { Analyst, Verifier });
choices!(InformationKind {
    Information,
    Question,
    Reply,
    Result,
    Blocked
});
choices!(TodoStatus {
    Pending,
    InProgress,
    Completed,
    Inconclusive
});
choices!(CoverageOutcome {
    Inspected,
    NeedsFollowUp,
    Inconclusive,
    Completed,
    NotReachable
});
choices!(Direction { To, From, Both });

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct Expected {
    pub exit_code: Option<i64>,
    pub signal: Option<i64>,
    pub stdout_contains: Option<String>,
    pub stderr_contains: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateAgentArgs {
    #[schemars(length(min = 1))]
    pub name: String,
    /// Analyst performs the work; verifier independently checks another agent's claim.
    pub role: ChildRole,
    #[schemars(length(min = 1))]
    pub task: String,
    #[schemars(length(min = 1))]
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional capabilities, such as recon, reverse, hunter, fuzz, crash, exploit or reporter.
    /// These supplement the assigned role and never change its permissions or lifecycle.
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherit_context: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_for_predecessors: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_policy: Option<DependencyPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<TaskContract>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ViewAgentGraphArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SendMessageToAgentArgs {
    #[schemars(length(min = 1))]
    pub agent_id: String,
    #[schemars(length(min = 1))]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<InformationKind>,
    #[schemars(range(min = 1, max = 9223372036854775807_u64))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FollowUpAgentArgs {
    #[schemars(length(min = 1))]
    pub agent_id: String,
    #[schemars(length(min = 1))]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ids: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct MessageStatusArgs {
    #[schemars(range(min = 1, max = 9223372036854775807_u64))]
    pub message_id: u64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WaitForAgentsArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_ids: Option<Vec<String>>,
    #[schemars(range(min = 1, max = 3600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<DependencyPolicy>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct StopAgentArgs {
    #[schemars(length(min = 1))]
    pub agent_id: String,
    #[schemars(length(min = 1))]
    pub reason: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentFinishArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FinishScanArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ThreatModelGetArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ThreatModelSaveArgs {
    #[schemars(length(min = 1))]
    pub title: String,
    #[schemars(length(min = 1))]
    pub body: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ThreatModelAmendArgs {
    #[schemars(length(min = 1))]
    pub id: String,
    #[schemars(length(min = 1))]
    pub amendment: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct NoteSaveArgs {
    #[schemars(length(min = 1))]
    pub title: String,
    #[schemars(length(min = 1))]
    pub body: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct NotesListArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolSearchArgs {
    pub query: Option<String>,
    pub group: Option<String>,
    #[schemars(range(min = 0))]
    pub offset: Option<usize>,
    #[schemars(range(min = 1, max = 10))]
    pub limit: Option<usize>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolsLoadArgs {
    #[schemars(length(max = 12))]
    pub names: Vec<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum ContextKind {
    Note,
    Todo,
    ThreatModel,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ContextSearchArgs {
    pub query: Option<String>,
    pub kind: Option<ContextKind>,
    pub scope: Option<String>,
    pub agent_id: Option<String>,
    #[schemars(range(min = 0))]
    pub offset: Option<usize>,
    #[schemars(range(min = 1, max = 10))]
    pub limit: Option<usize>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ContextReadArgs {
    pub kind: ContextKind,
    pub id: String,
    #[schemars(range(min = 0))]
    pub offset: Option<usize>,
    #[schemars(range(min = 1, max = 12000))]
    pub max_chars: Option<usize>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TodoSaveArgs {
    #[schemars(length(min = 1))]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TodoUpdateArgs {
    #[schemars(length(min = 1))]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TodoStatus>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TodosListArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CoverageRecordArgs {
    #[schemars(length(min = 1))]
    pub scope: String,
    #[schemars(length(min = 1))]
    pub kind: String,
    pub outcome: CoverageOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ids: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CoverageUpdateArgs {
    #[schemars(length(min = 1))]
    pub id: String,
    pub outcome: CoverageOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ids: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CoverageListArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FindingFileArgs {
    #[schemars(length(min = 1))]
    pub title: String,
    #[schemars(length(min = 1))]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_cause: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity_rationale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<FindingStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Map<String, Value>>,
    pub evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_id: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FindingUpdateArgs {
    #[schemars(length(min = 1))]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_cause: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity_rationale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<FindingStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_id: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FindingsListArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FindingGetArgs {
    #[schemars(length(min = 1))]
    pub id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct VerificationRecordArgs {
    #[schemars(length(min = 1))]
    pub finding_id: String,
    #[schemars(length(min = 1))]
    pub job_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Map<String, Value>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceReadArgs {
    #[schemars(length(min = 1))]
    pub evidence_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[schemars(range(min = 1, max = 1048576))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct LoadSkillArgs {
    #[schemars(length(min = 1))]
    pub name: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ThinkArgs {
    #[schemars(length(min = 1))]
    pub content: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BinaryInspectArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BinaryStringsArgs {
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
pub(super) struct FunctionsSearchArgs {
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
pub(super) struct FunctionDecompileArgs {
    pub address: u64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct XrefsQueryArgs {
    pub address: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    #[schemars(range(min = 1, max = 5000))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeDoctorArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct StageTargetArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FilePutArgs {
    #[schemars(length(min = 1))]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<bool>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileGetArgs {
    #[schemars(length(min = 1))]
    pub path: String,
    #[schemars(range(min = 1, max = 33554432))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct JobStartArgs {
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Map<String, Value>>,
    #[schemars(range(min = 1, max = 86400))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[schemars(range(min = 1024, max = 8388608))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_limit: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct JobStatusArgs {
    #[schemars(length(min = 1))]
    pub job_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct JobCancelArgs {
    #[schemars(length(min = 1))]
    pub job_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct JobArtifactsArgs {
    #[schemars(length(min = 1))]
    pub job_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FuzzStartArgs {
    #[schemars(length(min = 1))]
    pub target: String,
    #[schemars(length(min = 1))]
    pub seeds: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Map<String, Value>>,
    #[schemars(range(min = 1, max = 86400))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[schemars(range(min = 10, max = 60000))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_timeout_ms: Option<u64>,
    #[schemars(range(min = 16, max = 65536))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FuzzStatusArgs {
    #[schemars(length(min = 1))]
    pub job_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct DebugStartArgs {
    #[schemars(length(min = 1))]
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct DebugCommandArgs {
    #[schemars(length(min = 1))]
    pub session_id: String,
    #[schemars(length(min = 1))]
    pub command: String,
    #[schemars(range(min = 1, max = 60))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct DebugCloseArgs {
    #[schemars(length(min = 1))]
    pub session_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct InteractiveStartArgs {
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Map<String, Value>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct InteractiveSendArgs {
    #[schemars(length(min = 1))]
    pub session_id: String,
    pub data_base64: String,
    #[schemars(range(min = 1, max = 300))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct InteractiveCloseArgs {
    #[schemars(length(min = 1))]
    pub session_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AngrSolveArgs {
    #[schemars(length(min = 1))]
    pub target: String,
    pub find: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avoid: Option<Vec<u64>>,
    #[schemars(range(min = 1, max = 4096))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_length: Option<u64>,
    #[schemars(range(min = 1, max = 10000000))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u64>,
    #[schemars(range(min = 10, max = 3600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub printable_only: Option<bool>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CrashReplayArgs {
    #[schemars(length(min = 1))]
    pub target: String,
    #[schemars(length(min = 1))]
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<Expected>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Map<String, Value>>,
    #[schemars(range(min = 1, max = 60))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[schemars(range(min = 1, max = 16))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trials: Option<u64>,
    #[schemars(range(min = 1, max = 4096))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u64>,
    #[schemars(range(min = 1, max = 3600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimize_timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct InputMinimizeArgs {
    #[schemars(length(min = 1))]
    pub target: String,
    #[schemars(length(min = 1))]
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<Expected>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Map<String, Value>>,
    #[schemars(range(min = 1, max = 60))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[schemars(range(min = 1, max = 16))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trials: Option<u64>,
    #[schemars(range(min = 1, max = 4096))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u64>,
    #[schemars(range(min = 1, max = 3600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimize_timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PocVerifyArgs {
    #[schemars(length(min = 1))]
    pub finding_id: String,
    #[schemars(length(min = 1))]
    pub target: String,
    #[schemars(length(min = 1))]
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<Expected>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Map<String, Value>>,
    #[schemars(range(min = 1, max = 60))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[schemars(range(min = 1, max = 16))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trials: Option<u64>,
    #[schemars(range(min = 1, max = 4096))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u64>,
    #[schemars(range(min = 1, max = 3600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimize_timeout_secs: Option<u64>,
}
