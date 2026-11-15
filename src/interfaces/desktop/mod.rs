//! Desktop application boundary shared by the webview and its native host.
#[cfg(feature = "desktop")]
pub mod cleanup;
pub mod configuration;
#[cfg(feature = "desktop")]
pub mod host;
pub use crate::adapters::files::paths;
#[cfg(feature = "desktop")]
pub(crate) mod cleanup_review;
pub mod preferences;
pub mod presentation;
pub mod service;
mod tool_aliases;
#[cfg(feature = "desktop")]
mod update_diagnostics;
#[cfg(feature = "desktop")]
mod update_health;
#[cfg(feature = "desktop")]
pub mod updates;
pub mod workspace;
