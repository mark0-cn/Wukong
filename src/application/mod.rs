//! Business use cases and integration with agent execution.
pub(crate) mod agent_runtime;
pub mod agents;
pub mod assessment;
pub(crate) mod context;
pub mod coordinator;
pub(crate) mod effects;
pub mod maintenance;
pub(crate) mod model_catalog;
pub(crate) mod model_recovery;
pub(crate) mod progress;
pub(crate) mod prompts;
pub mod queries;
pub mod tools;
pub(crate) mod usage;
pub mod windows_lab;
// Preserve the original assessment service entry points.
pub use assessment::{AssessmentService, PreparedAssessment};
