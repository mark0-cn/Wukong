//! Standalone worker build; the engine implementation is shared with the development helper.
#[cfg(windows)]
mod dbgeng;

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    return dbgeng::serve();
    #[cfg(not(windows))]
    anyhow::bail!("DbgEng requires Windows")
}
