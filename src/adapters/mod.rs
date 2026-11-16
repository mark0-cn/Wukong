//! Concrete host, worker, provider and protocol adapters.
pub mod binary;
pub(crate) mod credentials;
pub mod environments;
pub mod files;
pub mod mcp;
pub mod models;
pub(crate) mod process_io;
pub mod windows_lab;
pub mod windows_lab_admin;
pub mod windows_lab_worker;
pub mod worker;
