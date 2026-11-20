//! Cross-process admission: a pending switch blocks new GUI and CLI assessments.
//!
//! The maintenance layer is also process-reentrant. Onboarding holds one
//! exclusive guard across environment activation, and activation saves the
//! configuration through `ConfigDocument::save_in`, which asks for the same
//! exclusive admission. OS file locks belong to an individual handle, so a
//! second `try_lock` from this process would fail exactly like a competing
//! process even though the outermost operation already owns the root.
//!
//! Reentrancy invariants:
//! - A per-process registry, keyed by the canonical lock root, is the single
//!   source of truth for locks this process holds. The registry entry owns the
//!   OS [`FileLock`]; the returned guards only carry a reference count.
//! - Exclusive admission admits every nested request: other processes are
//!   already excluded, and the exclusive owner's in-process work must be able
//!   to continue. Shared admission admits only nested shared requests; a nested
//!   exclusive request is still refused (no upgrade), because the shared owner
//!   represents another active task or assessment in this process.
//! - The OS lock is released once, when the last guard for a canonical root
//!   drops. Nested guards never unlock on their own.
//! - The registry mutex is held across the non-blocking `try_lock` attempt and
//!   registration so another thread cannot interleave an acquisition for the
//!   same root between the miss and the insert; `try_lock` never blocks, so no
//!   blocking call runs under the mutex.
//! - A poisoned registry mutex is recovered instead of panicking: the map only
//!   stores counted handles, so the data remains valid and a second panic while
//!   unwinding would be strictly worse.
pub use crate::adapters::files::locks::{FileLock, LockMode};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, OnceLock, PoisonError},
};

/// A held maintenance admission for one canonical lock root.
///
/// Nested acquisitions share the outermost OS lock through the registry: only
/// the last guard for a root releases it.
#[must_use = "keep the guard alive for the complete protected operation"]
pub struct MaintenanceLock {
    root: PathBuf,
}

impl Drop for MaintenanceLock {
    fn drop(&mut self) {
        let mut held = registry_locked();
        let release = {
            let Some(entry) = held.get_mut(&self.root) else {
                // Guards are only created after registration and Drop runs once.
                debug_assert!(false, "maintenance guard dropped without a registry entry");
                return;
            };
            entry.guards = entry.guards.saturating_sub(1);
            entry.guards == 0
        };
        if release {
            // Removing the entry drops the registry-owned FileLock, which is the
            // only place the outermost owner releases the OS lock.
            held.remove(&self.root);
        }
    }
}

/// One lock root currently held by this process.
struct Held {
    /// Mode of the outermost acquisition. A nested request never changes it.
    mode: LockMode,
    /// Live [`MaintenanceLock`] guards for this root.
    guards: usize,
    /// OS admission held for the complete outermost scope. Dropped with the
    /// registry entry once `guards` reaches zero. Field is never read.
    _os: FileLock,
}

fn registry() -> &'static Mutex<BTreeMap<PathBuf, Held>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<PathBuf, Held>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Recover a poisoned registry instead of panicking; see the module invariants.
fn registry_locked() -> MutexGuard<'static, BTreeMap<PathBuf, Held>> {
    registry().lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn lock(root: &Path, mode: LockMode) -> Result<MaintenanceLock> {
    acquire(root, mode, false)
}
pub(crate) fn cleanup_lock(root: &Path) -> Result<MaintenanceLock> {
    acquire(root, LockMode::Exclusive, true)
}
fn acquire(root: &Path, mode: LockMode, cleanup: bool) -> Result<MaintenanceLock> {
    std::fs::create_dir_all(root)
        .with_context(|| format!("create maintenance directory {}", root.display()))?;
    // Key by the canonical root so relative/absolute spellings of the same
    // directory (`recovery::root()` and `home.join("updates")`) share one entry.
    let root = std::fs::canonicalize(root)
        .with_context(|| format!("resolve maintenance directory {}", root.display()))?;
    let mut held = registry_locked();
    if let Some(entry) = held.get_mut(&root) {
        match (entry.mode, mode) {
            // The exclusive owner admits any nested request: the OS lock already
            // excludes other processes and this process must make progress.
            (LockMode::Exclusive, _) | (LockMode::Shared, LockMode::Shared) => {
                entry.guards += 1;
                return Ok(MaintenanceLock { root });
            }
            // Refuse an upgrade while this process still holds shared admission:
            // another task or assessment depends on it, so this mirrors what a
            // competing exclusive process would observe.
            (LockMode::Shared, LockMode::Exclusive) => {
                bail!("another application task or update is active")
            }
        }
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("maintenance.lock"))
        .with_context(|| format!("open maintenance lock in {}", root.display()))?;
    // `FileLock` acquisition is a non-blocking `try_lock`; keeping the registry
    // mutex here makes the miss-check and registration atomic without blocking.
    let os = mode.lock(file).context(match mode {
        LockMode::Exclusive => "another application task or update is active",
        LockMode::Shared => "application maintenance is in progress",
    })?;
    // Only the first acquisition re-checks the cleanup marker. Nested scopes run
    // inside a cleanup that is allowed to own the marker it is maintaining.
    ensure!(
        cleanup || !root.join("cleanup-in-progress.json").exists(),
        "uninstall cleanup is in progress; finish or retry the uninstaller"
    );
    held.insert(
        root.clone(),
        Held {
            mode,
            guards: 1,
            _os: os,
        },
    );
    Ok(MaintenanceLock { root })
}
pub fn assessment_guard() -> Result<MaintenanceLock> {
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
    use std::time::Duration;

    /// A competing-process stand-in. OS file locks belong to an individual
    /// handle, so a second open handle in this process observes exactly the
    /// exclusion a second process would, without depending on process spawns.
    fn competing_try_lock(root: &Path, mode: LockMode) -> Result<()> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("maintenance.lock"))?;
        mode.lock(file).map(drop).map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => anyhow::anyhow!("would block"),
            std::fs::TryLockError::Error(error) => error.into(),
        })
    }

    fn root(directory: &tempfile::TempDir) -> PathBuf {
        directory.path().join("updates")
    }

    #[test]
    fn nested_exclusive_acquisitions_share_one_os_lock() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = root(&directory);
        let outer = lock(&root, LockMode::Exclusive)?;
        let inner = lock(&root, LockMode::Exclusive)?;
        // Nested guards still exclude a competing process while any is alive.
        assert!(competing_try_lock(&root, LockMode::Shared).is_err());
        assert!(competing_try_lock(&root, LockMode::Exclusive).is_err());
        drop(inner);
        assert!(
            competing_try_lock(&root, LockMode::Shared).is_err(),
            "an inner guard must not release the outer OS lock"
        );
        drop(outer);
        assert!(
            competing_try_lock(&root, LockMode::Exclusive).is_ok(),
            "the last guard must release the OS lock"
        );
        Ok(())
    }

    #[test]
    fn shared_guards_nest_but_cannot_upgrade_to_exclusive() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = root(&directory);
        let first = lock(&root, LockMode::Shared)?;
        let second = lock(&root, LockMode::Shared)?;
        assert!(competing_try_lock(&root, LockMode::Exclusive).is_err());
        assert!(
            lock(&root, LockMode::Exclusive).is_err(),
            "a shared owner must not upgrade to exclusive admission"
        );
        drop(first);
        drop(second);
        drop(lock(&root, LockMode::Exclusive)?);
        assert!(lock(&root, LockMode::Shared).is_ok());
        Ok(())
    }

    #[test]
    fn exclusive_admission_admits_nested_shared_requests() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = root(&directory);
        let outer = lock(&root, LockMode::Exclusive)?;
        let shared = lock(&root, LockMode::Shared)?;
        let exclusive = lock(&root, LockMode::Exclusive)?;
        drop(exclusive);
        drop(shared);
        assert!(competing_try_lock(&root, LockMode::Shared).is_err());
        drop(outer);
        assert!(competing_try_lock(&root, LockMode::Shared).is_ok());
        Ok(())
    }

    #[test]
    fn separate_roots_do_not_interfere() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        let exclusive = lock(&first, LockMode::Exclusive)?;
        let shared = lock(&second, LockMode::Shared)?;
        assert!(competing_try_lock(&first, LockMode::Shared).is_err());
        assert!(competing_try_lock(&second, LockMode::Exclusive).is_err());
        drop(shared);
        drop(exclusive);
        assert!(competing_try_lock(&first, LockMode::Exclusive).is_ok());
        assert!(competing_try_lock(&second, LockMode::Exclusive).is_ok());
        Ok(())
    }

    #[test]
    fn competing_process_admission_still_excludes_and_releases() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = root(&directory);
        std::fs::create_dir_all(&root)?;
        let other = {
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(root.join("maintenance.lock"))?;
            FileLock::try_lock(file).expect("a fresh root is free")
        };
        assert!(lock(&root, LockMode::Shared).is_err());
        assert!(lock(&root, LockMode::Exclusive).is_err());
        drop(other);
        assert!(lock(&root, LockMode::Shared).is_ok());
        Ok(())
    }

    #[test]
    fn cleanup_marker_gates_only_the_first_acquisition() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = root(&directory);
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join("cleanup-in-progress.json"), b"{}")?;
        assert!(lock(&root, LockMode::Shared).is_err());
        assert!(lock(&root, LockMode::Exclusive).is_err());
        let cleanup = cleanup_lock(&root)?;
        // Work performed by the cleanup itself must not trip over the marker it
        // writes, so nested acquisitions are admitted under the cleanup guard.
        drop(lock(&root, LockMode::Exclusive)?);
        drop(lock(&root, LockMode::Shared)?);
        drop(cleanup);
        // With the registry empty, the marker still rejects a new admission.
        assert!(lock(&root, LockMode::Shared).is_err());
        std::fs::remove_file(root.join("cleanup-in-progress.json"))?;
        assert!(lock(&root, LockMode::Shared).is_ok());
        Ok(())
    }

    #[test]
    fn concurrent_threads_share_one_os_lock_and_release_it_once() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = root(&directory);
        let outer = lock(&root, LockMode::Shared)?;
        let threads = (0..4)
            .map(|_| {
                let root = root.clone();
                std::thread::spawn(move || -> Result<()> {
                    for _ in 0..8 {
                        let guard = lock(&root, LockMode::Shared)?;
                        std::thread::sleep(Duration::from_millis(1));
                        drop(guard);
                    }
                    Ok(())
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("maintenance admission thread panicked"))??;
        }
        assert!(competing_try_lock(&root, LockMode::Exclusive).is_err());
        drop(outer);
        assert!(competing_try_lock(&root, LockMode::Exclusive).is_ok());
        Ok(())
    }

    #[test]
    fn tasks_and_maintenance_exclude_each_other() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = root(&directory);
        let task = lock(&root, LockMode::Shared)?;
        assert!(lock(&root, LockMode::Shared).is_ok());
        assert!(lock(&root, LockMode::Exclusive).is_err());
        assert!(competing_try_lock(&root, LockMode::Exclusive).is_err());
        drop(task);
        let update = lock(&root, LockMode::Exclusive)?;
        assert!(competing_try_lock(&root, LockMode::Shared).is_err());
        // In-process nested shared admission is reentrant under the update owner
        // instead of failing against this process's own handle.
        assert!(lock(&root, LockMode::Shared).is_ok());
        drop(update);
        assert!(lock(&root, LockMode::Shared).is_ok());
        Ok(())
    }
}
