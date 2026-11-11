//! Cooperative cancellation: callers must finish owned-resource cleanup before settling.
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("preparation cancelled")
    }
}
impl std::error::Error for Cancelled {}
pub fn check(token: Option<&CancellationToken>) -> anyhow::Result<()> {
    if token.is_some_and(CancellationToken::is_cancelled) {
        return Err(Cancelled.into());
    }
    Ok(())
}
