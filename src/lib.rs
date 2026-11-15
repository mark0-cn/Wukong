//! Controller entry points; implementation lives under responsibility modules.
pub mod adapters;
pub mod application;
mod bootstrap;
pub mod config;
pub mod domain;
pub mod i18n;
pub mod interfaces;
pub mod queries;
pub mod storage;

// Compatibility exports for existing library consumers, centralized here.
pub use adapters::{binary, environments, mcp, models as llm, worker as runtime};
pub use application::assessment::runner;
pub use application::maintenance::updates;
pub use application::{agents, coordinator, tools};
pub use interfaces::{cli as interface, desktop, report};
pub use storage as store;
pub mod cleanup {
    pub use crate::application::maintenance::cleanup::*;
    pub use crate::interfaces::cli::cleanup as cli;
}
pub mod workbench {
    pub use crate::application::maintenance::workbench::*;
    pub use crate::interfaces::cli::workbench as cli;
}
