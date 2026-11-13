//! Native VM installation/service implementation. Controller signing lives in
//! the independent package library and never in a model-callable service API.
#[cfg(windows)]
pub mod accounts;
#[cfg(windows)]
mod authority;
#[cfg(windows)]
pub mod broker;
#[cfg(any(windows, test))]
mod broker_wire;
#[cfg(windows)]
mod debugger_mcp;
#[cfg(windows)]
pub mod deployment;
#[cfg(windows)]
mod environment;
#[cfg(windows)]
pub mod firewall;
#[cfg(windows)]
mod inputs;
#[cfg(windows)]
pub mod installer;
#[cfg(windows)]
mod lease_proxy;
pub mod pe_imports;
#[cfg(windows)]
mod persona;
#[cfg(windows)]
pub mod pipe_transport;
#[cfg(windows)]
mod process_access;
#[cfg(windows)]
pub mod process_capture;
#[cfg(windows)]
pub mod processes;
#[cfg(windows)]
pub mod recovery;
#[cfg(windows)]
mod resource_cleanup;
#[cfg(windows)]
pub mod resource_host;
#[cfg(windows)]
mod resource_image;
#[cfg(windows)]
pub mod resource_input;
#[cfg(windows)]
pub mod resource_journal;
#[cfg(windows)]
mod resource_launch;
#[cfg(windows)]
pub mod resource_process;
#[cfg(windows)]
pub mod resource_recovery;
#[cfg(windows)]
mod resource_runtime;
#[cfg(windows)]
pub mod security;
#[cfg(windows)]
pub mod service;
#[cfg(windows)]
mod service_access;
#[cfg(windows)]
pub mod setup_ui;
#[cfg(windows)]
pub mod staging;
#[cfg(windows)]
mod token_policy;
