//! Common tool input schemas.
use super::*;

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
pub(in crate::tools) struct Expected {
    pub exit_code: Option<i64>,
    pub signal: Option<i64>,
    pub stdout_contains: Option<String>,
    pub stderr_contains: Option<String>,
}

choices!(ByteOrder { Little, Big });

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(in crate::tools) enum InputMode {
    Argv,
    Stdin,
}

choices!(CaptureEncoding { Hex, Little, Big });

choices!(AnalysisVerdict {
    Supported,
    Refuted,
    Inconclusive
});
