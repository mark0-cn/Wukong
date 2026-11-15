//! Windows identity, process, network and filesystem isolation.
pub(crate) mod environment;
pub mod firewall;
pub(crate) mod persona;
pub(crate) mod process_access;
pub mod process_capture;
pub(crate) mod process_profile;
pub mod processes;
pub(crate) mod resource_cleanup;
pub mod security;
pub(crate) mod token_policy;
