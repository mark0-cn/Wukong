//! Desktop application boundary shared by the webview and its native host.
pub mod configuration;
#[cfg(feature = "desktop")]
pub mod host;
pub mod paths;
pub mod presentation;
pub mod service;
#[cfg(feature = "desktop")]
pub mod updates;
pub mod workspace;
