//! Registered builtin handlers. The catalog owns routing and recovery metadata.
use super::*;
use futures::future::BoxFuture;

mod coordination;
pub(super) use coordination::*;
mod context;
pub(super) use context::*;
mod findings;
pub(super) use findings::*;
mod analysis;
pub(super) use analysis::*;
mod execution;
pub(super) use execution::*;
