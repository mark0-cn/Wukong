//! Context tool input schemas.
use super::*;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct ThreatModelGetArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct ThreatModelSaveArgs {
    #[schemars(length(min = 1))]
    pub title: String,
    #[schemars(length(min = 1))]
    pub body: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct ThreatModelAmendArgs {
    #[schemars(length(min = 1))]
    pub id: String,
    #[schemars(length(min = 1))]
    pub amendment: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct NoteSaveArgs {
    #[schemars(length(min = 1))]
    pub title: String,
    #[schemars(length(min = 1))]
    pub body: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct NotesListArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(in crate::application::tools) enum ContextKind {
    Note,
    Todo,
    ThreatModel,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct ContextSearchArgs {
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
pub(in crate::application::tools) struct ContextReadArgs {
    pub kind: ContextKind,
    pub id: String,
    #[schemars(range(min = 0))]
    pub offset: Option<usize>,
    #[schemars(range(min = 1, max = 12000))]
    pub max_chars: Option<usize>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct TodoSaveArgs {
    #[schemars(length(min = 1))]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct TodoUpdateArgs {
    #[schemars(length(min = 1))]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TodoStatus>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct TodosListArgs {}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::application::tools) struct CoverageRecordArgs {
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
pub(in crate::application::tools) struct CoverageUpdateArgs {
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
pub(in crate::application::tools) struct CoverageListArgs {}
