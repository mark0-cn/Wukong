//! Native VM installation/service implementation. Controller signing lives in
//! the independent package library and never in a model-callable service API.
#[cfg(windows)]
pub mod accounts;
#[cfg(windows)]
mod authority;
#[cfg(windows)]
pub mod deployment;
#[cfg(windows)]
pub mod firewall;
#[cfg(windows)]
pub mod installer;
#[cfg(windows)]
pub mod pipe_transport;
#[cfg(windows)]
pub mod processes;
#[cfg(windows)]
pub mod recovery;
#[cfg(windows)]
pub mod security;
#[cfg(windows)]
pub mod service;
#[cfg(windows)]
pub mod setup_ui;
#[cfg(windows)]
pub mod staging;
