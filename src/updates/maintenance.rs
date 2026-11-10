//! Cross-process admission: a pending switch blocks new GUI and CLI assessments.
use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

pub fn lock(root: &Path, exclusive: bool) -> Result<File> {
    std::fs::create_dir_all(root)
        .with_context(|| format!("create maintenance directory {}", root.display()))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("maintenance.lock"))
        .with_context(|| format!("open maintenance lock in {}", root.display()))?;
    if exclusive {
        file.try_lock()
            .context("another application task or update is active")?;
    } else {
        file.try_lock_shared()
            .context("application maintenance is in progress")?;
    }
    Ok(file)
}
pub fn assessment_guard() -> Result<File> {
    let root = crate::desktop::paths::home()?.join("updates");
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
