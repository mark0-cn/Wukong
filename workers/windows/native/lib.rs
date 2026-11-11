//! Native VM installation/service implementation. Controller signing lives in
//! the independent package library and never in a model-callable service API.
#[cfg(windows)]
pub mod security;
#[cfg(windows)]
pub mod staging;
