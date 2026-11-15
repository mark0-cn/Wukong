//! Cross-process admission: a pending switch blocks new GUI and CLI assessments.
use crate::adapters::files::locks::FileLock;
use anyhow::{Context, Result, ensure};
use std::{fs::OpenOptions, path::Path};

pub fn lock(root: &Path, exclusive: bool) -> Result<FileLock> {
    acquire(root, exclusive, false)
}
pub(crate) fn cleanup_lock(root: &Path) -> Result<FileLock> {
    acquire(root, true, true)
}
fn acquire(root: &Path, exclusive: bool, cleanup: bool) -> Result<FileLock> {
    std::fs::create_dir_all(root)
        .with_context(|| format!("create maintenance directory {}", root.display()))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("maintenance.lock"))
        .with_context(|| format!("open maintenance lock in {}", root.display()))?;
    let guard = if exclusive {
        FileLock::try_lock(file).context("another application task or update is active")?
    } else {
        FileLock::try_lock_shared(file).context("application maintenance is in progress")?
    };
    ensure!(
        cleanup || !root.join("cleanup-in-progress.json").exists(),
        "uninstall cleanup is in progress; finish or retry the uninstaller"
    );
    Ok(guard)
}
pub fn assessment_guard() -> Result<FileLock> {
    let root = crate::adapters::files::paths::home()?.join("updates");
    let guard = lock(&root, false)?;
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
        let task = lock(dir.path(), false)?;
        assert!(lock(dir.path(), false).is_ok());
        assert!(lock(dir.path(), true).is_err());
        drop(task);
        let update = lock(dir.path(), true)?;
        assert!(lock(dir.path(), false).is_err());
        drop(update);
        assert!(lock(dir.path(), false).is_ok());
        Ok(())
    }
}
