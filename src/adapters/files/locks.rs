//! File locks whose lifetime belongs to a scope, not inherited descriptors.
use std::fs::{File, TryLockError};

/// Requested sharing for an advisory lock. Named so a call site cannot pass a
/// bare `true`/`false` whose meaning lives only in the callee's parameter list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    Shared,
    Exclusive,
}

impl LockMode {
    /// Take the requested lock on an already-open file.
    pub fn lock(self, file: File) -> Result<FileLock, TryLockError> {
        match self {
            Self::Shared => FileLock::try_lock_shared(file),
            Self::Exclusive => FileLock::try_lock(file),
        }
    }
}

#[must_use = "keep the guard alive for the complete protected operation"]
pub struct FileLock {
    file: File,
}

impl FileLock {
    pub fn try_lock(file: File) -> Result<Self, TryLockError> {
        file.try_lock()?;
        Ok(Self { file })
    }

    pub fn try_lock_shared(file: File) -> Result<Self, TryLockError> {
        file.try_lock_shared()?;
        Ok(Self { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // On Unix, close alone leaves the lock held by a concurrent fork until
        // exec closes its inherited descriptor. End this owner's scope now.
        let _ = self.file.unlock();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn inherited_descriptors_do_not_extend_or_release_another_owners_lock() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("scope.lock");
        let open = || {
            File::options()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
        };
        for shared in [false, true] {
            let file = open()?;
            let inherited = file.try_clone()?;
            let owner = if shared {
                FileLock::try_lock_shared(file)?
            } else {
                FileLock::try_lock(file)?
            };
            assert!(matches!(
                FileLock::try_lock(open()?),
                Err(TryLockError::WouldBlock)
            ));
            drop(owner);
            let successor = FileLock::try_lock(open()?)?;
            drop(inherited);
            assert!(matches!(
                FileLock::try_lock(open()?),
                Err(TryLockError::WouldBlock)
            ));
            drop(successor);
            assert!(FileLock::try_lock(open()?).is_ok());
        }
        Ok(())
    }
}
