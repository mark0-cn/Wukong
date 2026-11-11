mod claims;
pub(crate) use claims::flag_claims;
mod rules;
pub use rules::{CompletionFacts, CompletionRules, DependencyRules, MessageRules, TaskGraph};
mod progress;
mod resources;
use chrono::{DateTime, Utc};
pub use progress::*;
pub use resources::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, path::PathBuf, str::FromStr};
use uuid::Uuid;

pub fn id(prefix: &str) -> String {
    let value = Uuid::new_v4().simple().to_string();
    if prefix == "agent" {
        value
    } else {
        format!("{prefix}_{value}")
    }
}
pub fn now() -> DateTime<Utc> {
    Utc::now()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Queued,
    Running,
    Waiting,
    BudgetPaused,
    Completed,
    Stopped,
    Crashed,
    Failed,
}
impl Status {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Stopped | Self::Crashed | Self::Failed
        )
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::BudgetPaused => "budget_paused",
            Self::Completed => "completed",
            Self::Stopped => "stopped",
            Self::Crashed => "crashed",
            Self::Failed => "failed",
        }
    }
}
impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Root,
    Analyst,
    Verifier,
}
impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Analyst => "analyst",
            Self::Verifier => "verifier",
        }
    }
}
impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl FromStr for Role {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "root" => Self::Root,
            "analyst" => Self::Analyst,
            "verifier" => Self::Verifier,
            _ => anyhow::bail!("unknown role {s}"),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Budget {
    pub max_agents: usize,
    pub max_depth: u32,
    pub max_concurrent_agents: usize,
    pub max_turns_per_agent: usize,
    pub max_total_tokens: u64,
    pub max_usd: Option<f64>,
    pub max_wall_seconds: u64,
    /// Optional per-role share of the run token budget for worker agents;
    /// missing roles use [`Budget::role_share`] defaults. The root agent is
    /// governed by the run budget alone. Shares are not enforced to sum to 1.
    pub role_token_shares: std::collections::HashMap<Role, f64>,
}
impl Budget {
    pub fn role_share(&self, role: Role) -> f64 {
        self.role_token_shares
            .get(&role)
            .copied()
            .unwrap_or(match role {
                Role::Root => 0.15,
                Role::Analyst => 0.60,
                Role::Verifier => 0.30,
            })
    }

    /// Per-agent token quota derived from the run budget. A 10k floor keeps
    /// small-budget runs governed by the run-level budget alone instead of
    /// degenerating into role quotas that kill every agent on its first call.
    pub fn role_token_cap(&self, role: Role) -> u64 {
        ((self.role_share(role) * self.max_total_tokens as f64) as u64).max(10_000)
    }
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            max_agents: 24,
            max_depth: 4,
            max_concurrent_agents: 4,
            max_turns_per_agent: 40,
            max_total_tokens: 1_600_000,
            max_usd: None,
            max_wall_seconds: 3600,
            role_token_shares: std::collections::HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub target_path: PathBuf,
    pub target_sha256: String,
    pub task: String,
    pub status: Status,
    pub root_id: String,
    pub config: Value,
    pub budget: Budget,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveSkill {
    pub name: String,
    pub sha256: String,
    pub content: String,
    pub evidence_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadata {
    pub id: String,
    pub run_id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub role: Role,
    pub task: String,
    pub scope: String,
    pub contract: TaskContract,
    #[serde(default)]
    pub task_result: Option<TaskResult>,
    pub skills: Vec<String>,
    /// Immutable snapshots explicitly activated with load_skill.
    #[serde(default)]
    pub active_skills: Vec<ActiveSkill>,
    pub status: Status,
    pub depth: u32,
    /// Follow-up activation; separates tool IDs and terminal notices from prior work.
    #[serde(default)]
    pub generation: u64,
    pub inherit_context: bool,
    pub wait_for: Vec<String>,
    pub wait_until: Option<DateTime<Utc>>,
    pub wait_policy: DependencyPolicy,
    /// Declared predecessor agents this child must wait for before the
    /// runner starts it (scheduler-level gating; distinct from the parked
    /// waiter `wait_for` used by wait_for_agents).
    #[serde(default)]
    pub wait_for_predecessors: Vec<String>,
    pub dependency_policy: DependencyPolicy,
    pub predecessors_released: bool,
    pub turns: usize,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub token_cap: Option<u64>,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Optional history is loaded only at explicit inheritance/recovery boundaries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConversation {
    pub inherited_context: Option<Value>,
    pub checkpoint: Option<Value>,
}

/// Complete metadata and history for recovery, inheritance and export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSnapshot {
    #[serde(flatten)]
    pub metadata: AgentMetadata,
    pub inherited_context: Option<Value>,
    pub checkpoint: Option<Value>,
}

impl AgentSnapshot {
    pub fn from_parts(metadata: AgentMetadata, conversation: AgentConversation) -> Self {
        Self {
            metadata,
            inherited_context: conversation.inherited_context,
            checkpoint: conversation.checkpoint,
        }
    }
    pub fn into_conversation(self) -> AgentConversation {
        AgentConversation {
            inherited_context: self.inherited_context,
            checkpoint: self.checkpoint,
        }
    }
}
impl std::ops::Deref for AgentSnapshot {
    type Target = AgentMetadata;
    fn deref(&self) -> &Self::Target {
        &self.metadata
    }
}
impl std::ops::DerefMut for AgentSnapshot {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.metadata
    }
}

#[derive(
    schemars::JsonSchema, Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq,
)]
#[serde(rename_all = "snake_case")]
pub enum DependencyPolicy {
    #[default]
    AllSuccess,
    AllTerminal,
    AnySuccess,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DependencyState {
    Pending,
    Satisfied,
    Blocked { reason: String },
}

#[derive(schemars::JsonSchema, Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct TaskContract {
    pub objective: String,
    pub input_evidence_ids: Vec<String>,
    pub constraints: Vec<String>,
    /// Names of required evidence-backed artifacts, such as a field table.
    pub deliverables: Vec<String>,
    /// Exact labels the result must explicitly account for on success.
    pub completion_conditions: Vec<String>,
}

#[derive(
    schemars::JsonSchema, Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq,
)]
#[serde(rename_all = "snake_case")]
pub enum TaskOutcome {
    #[default]
    Success,
    Inconclusive,
    Blocked,
    Cancelled,
}

#[derive(schemars::JsonSchema, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskArtifact {
    pub name: String,
    pub evidence_id: String,
    /// Structured summary of the linked original artifact; never a replacement for original evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<StructuredArtifact>,
}

#[derive(schemars::JsonSchema, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredArtifact {
    /// formats, algorithms, state_machine or binary_diff.
    pub kind: String,
    /// Each row includes evidence_id plus the fields required by its kind.
    pub rows: Vec<serde_json::Value>,
}
impl StructuredArtifact {
    pub fn validate(&self) -> anyhow::Result<()> {
        let required: &[&str] = match self.kind.as_str() {
            "formats" => &["offset", "width", "encoding", "meaning", "confidence"],
            "algorithms" => &["input", "expected", "observed", "arithmetic"],
            "state_machine" => &["from", "action", "to", "expected", "observed"],
            "binary_diff" => &["baseline_sha256", "target_sha256", "before", "after"],
            _ => anyhow::bail!("unknown structured artifact kind"),
        };
        anyhow::ensure!(
            !self.rows.is_empty() && self.rows.len() <= 256,
            "invalid structured row count"
        );
        for row in &self.rows {
            anyhow::ensure!(
                serde_json::to_vec(row)?.len() <= 16384,
                "structured row too large"
            );
            for key in required.iter().copied().chain(["evidence_id"]) {
                anyhow::ensure!(
                    row.get(key).is_some_and(
                        |v| !v.is_null() && v.as_str().is_none_or(|s| !s.trim().is_empty())
                    ),
                    "missing structured field {key}"
                );
            }
            anyhow::ensure!(
                row["evidence_id"].as_str().is_some(),
                "evidence_id must be a string"
            );
            if self.kind == "binary_diff" {
                for key in ["baseline_sha256", "target_sha256"] {
                    anyhow::ensure!(
                        row[key].as_str().is_some_and(
                            |s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
                        ),
                        "invalid target digest"
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(schemars::JsonSchema, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskResult {
    pub outcome: TaskOutcome,
    pub answer: String,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<TaskArtifact>,
    #[serde(default)]
    pub completed_conditions: Vec<String>,
    #[serde(default)]
    pub unknowns: Vec<String>,
    #[serde(default)]
    pub next_actions: Vec<String>,
}

impl TaskResult {
    pub fn evidence_links(&self) -> Vec<String> {
        self.evidence_ids
            .iter()
            .cloned()
            .chain(
                self.artifacts
                    .iter()
                    .map(|artifact| artifact.evidence_id.clone()),
            )
            .chain(
                self.artifacts
                    .iter()
                    .filter_map(|a| a.structured.as_ref())
                    .flat_map(|s| &s.rows)
                    .filter_map(|r| r["evidence_id"].as_str().map(str::to_owned)),
            )
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub run_id: String,
    pub sender: String,
    pub recipient: String,
    pub content: String,
    pub acknowledged: bool,
    pub created_at: DateTime<Utc>,
    pub envelope: MessageEnvelope,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    #[default]
    Information,
    Question,
    Reply,
    FollowUp,
    Result,
    Blocked,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageDelivery {
    #[default]
    Queued,
    Included,
    Replied,
    Stale,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageOptions {
    pub kind: MessageKind,
    pub correlation_id: Option<String>,
    pub reply_to: Option<i64>,
    pub recipient_generation: Option<u64>,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageEnvelope {
    #[serde(flatten)]
    pub options: MessageOptions,
    pub task_id: String,
    pub sender_generation: Option<u64>,
    pub delivery: MessageDelivery,
    pub replied_by: Option<i64>,
    pub stale_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageReceipt {
    pub message_id: i64,
    pub delivery: MessageDelivery,
    pub recipient_generation: u64,
    pub correlation_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub run_id: String,
    pub agent_id: Option<String>,
    pub kind: String,
    pub data: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    Prepared,
    Running,
    Completed,
    Failed,
    Uncertain,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub run_id: String,
    pub agent_id: String,
    pub name: String,
    pub arguments: Value,
    pub state: CallState,
    pub side_effecting: bool,
    pub result: Option<Value>,
    pub evidence_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub id: String,
    pub run_id: String,
    pub agent_id: String,
    pub tool_call_id: String,
    pub sha256: String,
    pub relative_path: String,
    pub size: u64,
    pub media_type: String,
    pub source: String,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(schemars::JsonSchema, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FindingStage {
    Suspected,
    Reproduced,
    RootCauseConfirmed,
    ImpactVerified,
    Inconclusive,
    Rejected,
}
#[derive(schemars::JsonSchema, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thought {
    pub id: String,
    pub run_id: String,
    pub agent_id: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub run_id: String,
    pub agent_id: String,
    pub title: String,
    pub description: String,
    pub root_cause: Option<String>,
    pub impact: Option<String>,
    pub severity: Option<Severity>,
    pub severity_rationale: Option<String>,
    pub stage: FindingStage,
    pub location: Option<Value>,
    pub evidence_ids: Vec<String>,
    pub verification_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    pub id: String,
    pub run_id: String,
    pub finding_id: String,
    pub verifier_agent_id: String,
    pub passed: bool,
    pub attempts: u64,
    pub successes: u64,
    pub impact: String,
    pub evidence_ids: Vec<String>,
    pub environment: Value,
    pub conditions: Value,
    pub result: Value,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Coverage {
    pub id: String,
    pub run_id: String,
    pub agent_id: String,
    pub scope: String,
    pub kind: String,
    pub outcome: String,
    pub notes: String,
    pub evidence_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedDocument {
    pub id: String,
    pub run_id: String,
    pub agent_id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub amendments: Vec<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Records whose ownership can be checked without serializing their payload.
pub trait RunScoped {
    fn run_id(&self) -> &str;
}
macro_rules! run_scoped { ($($record:ty),+ $(,)?) => { $(impl RunScoped for $record { fn run_id(&self)->&str { &self.run_id } })+ }; }
run_scoped!(
    AgentMetadata,
    AgentSnapshot,
    JobRecord,
    SessionRecord,
    WorkerFileRecord,
    Finding,
    Verification,
    Coverage,
    Evidence,
    SharedDocument,
    ToolCallRecord,
    Thought,
    PublicReply
);
