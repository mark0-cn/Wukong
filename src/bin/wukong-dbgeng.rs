//! Worker-only native debugger helper. The worker owns admission and target process creation.
#[cfg(windows)]
#[path = "../../workers/windows/native/dbgeng.rs"]
mod dbgeng;

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    return dbgeng::serve();
    #[cfg(not(windows))]
    anyhow::bail!("DbgEng requires Windows")
}
