//! Polls an owned Tokio task without holding a resource lock across external IO.
use anyhow::{Context, Result};
use futures_util::FutureExt;
pub(super) enum Observation<T> {
    Running(tokio::task::JoinHandle<Result<T>>),
    Complete(T),
    Failed(String),
}
impl<T> Observation<T> {
    pub(super) fn poll(&mut self) -> Result<Option<&T>> {
        if let Self::Running(task) = self {
            let Some(result) = task.now_or_never() else {
                return Ok(None);
            };
            *self = match result
                .context("owned resource observer interrupted")
                .and_then(|result| result)
            {
                Ok(value) => Self::Complete(value),
                Err(error) => Self::Failed(format!("{error:#}").chars().take(2048).collect()),
            };
        }
        match self {
            Self::Complete(value) => Ok(Some(value)),
            Self::Failed(error) => anyhow::bail!("{error}"),
            Self::Running(_) => unreachable!(),
        }
    }
}
