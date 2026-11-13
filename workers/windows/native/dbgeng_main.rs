//! Standalone legacy helper, pending replacement by the external WinDbg MCP backend.
#[cfg(windows)]
mod dbgeng;

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    return dbgeng::serve();
    #[cfg(not(windows))]
    anyhow::bail!("DbgEng requires Windows")
}
