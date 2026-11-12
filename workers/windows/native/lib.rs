//! Native VM installation/service implementation. Controller signing lives in
//! the independent package library and never in a model-callable service API.
#[cfg(windows)]
pub mod accounts;
#[cfg(windows)]
mod authority;
#[cfg(windows)]
pub mod broker;
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
#[cfg(windows)]
mod persona;
#[cfg(windows)]
pub mod pipe_transport;
#[cfg(windows)]
mod process_access;
#[cfg(windows)]
pub mod processes;
#[cfg(windows)]
pub mod recovery;
#[cfg(windows)]
pub mod resource_host;
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
