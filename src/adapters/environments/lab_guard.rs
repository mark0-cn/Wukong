//! Cross-process admission shared by assessment workers and VM maintenance.
use crate::adapters::files::locks::FileLock;
use anyhow::{Context, Result};
use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
};

pub fn path(vmx: &Path) -> Result<PathBuf> {
    let canonical = if vmx.exists() {
        std::fs::canonicalize(vmx)?
    } else {
        std::path::absolute(vmx)?
    };
    Ok(canonical.with_extension("wukong-environment.lock"))
}
pub fn acquire(path: &Path, exclusive: bool) -> Result<FileLock> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    if exclusive { FileLock::try_lock(file) } else { FileLock::try_lock_shared(file) }
        .context("VM maintenance conflicts with an active assessment connection; finish and close its resources first")
}

use sha2::Digest;
/// Canonical physical identity, independent of the registered display alias.
pub fn physical_identity(vmx: &Path) -> Result<String> {
    let path = if vmx.exists() {
        std::fs::canonicalize(vmx)?
    } else {
        std::path::absolute(vmx)?
    };
    let spelling = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let spelling = spelling.to_lowercase();
    Ok(hex::encode(sha2::Sha256::digest(spelling.as_bytes())))
}
