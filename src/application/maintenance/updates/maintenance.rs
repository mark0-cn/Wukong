//! Cross-process admission: a pending switch blocks new GUI and CLI assessments.
pub use crate::adapters::files::locks::{FileLock, LockMode};
use anyhow::{Context, Result, ensure};
use std::{fs::OpenOptions, path::Path};

pub fn lock(root: &Path, mode: LockMode) -> Result<FileLock> {
    acquire(root, mode, false)
}
pub(crate) fn cleanup_lock(root: &Path) -> Result<FileLock> {
    acquire(root, LockMode::Exclusive, true)
}
fn acquire(root: &Path, mode: LockMode, cleanup: bool) -> Result<FileLock> {
    std::fs::create_dir_all(root)
        .with_context(|| format!("create maintenance directory {}", root.display()))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("maintenance.lock"))
        .with_context(|| format!("open maintenance lock in {}", root.display()))?;
    let guard = mode.lock(file).context(match mode {
        LockMode::Exclusive => "another application task or update is active",
        LockMode::Shared => "application maintenance is in progress",
    })?;
    ensure!(
        cleanup || !root.join("cleanup-in-progress.json").exists(),
        "uninstall cleanup is in progress; finish or retry the uninstaller"
    );
    Ok(guard)
}
pub fn assessment_guard() -> Result<FileLock> {
    let root = crate::adapters::files::paths::home()?.join("updates");
    let guard = lock(&root, LockMode::Shared)?;
    ensure!(
        !root.join("pending.json").exists(),
        "an update needs confirmation or recovery; open the update center first"
    );
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tasks_and_maintenance_exclude_each_other() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let task = lock(dir.path(), LockMode::Shared)?;
        assert!(lock(dir.path(), LockMode::Shared).is_ok());
        assert!(lock(dir.path(), LockMode::Exclusive).is_err());
        drop(task);
        let update = lock(dir.path(), LockMode::Exclusive)?;
        assert!(lock(dir.path(), LockMode::Shared).is_err());
        drop(update);
        assert!(lock(dir.path(), LockMode::Shared).is_ok());
        Ok(())
    }
}
