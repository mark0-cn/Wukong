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
    NotReachable,
    Superseded
});
choices!(Direction { To, From, Both });

choices!(AnalysisVerdict {
    Supported,
    Refuted,
    Inconclusive
});
