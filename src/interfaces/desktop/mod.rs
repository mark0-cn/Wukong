//! Desktop application boundary shared by the webview and its native host.
#[cfg(feature = "desktop")]
pub mod cleanup;
pub mod configuration;
#[cfg(feature = "desktop")]
pub mod host;
#[cfg(all(feature = "desktop", windows))]
pub mod worker_media;
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
pub mod updates;
pub mod workspace;

#[cfg(feature = "desktop")]
fn context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
}
