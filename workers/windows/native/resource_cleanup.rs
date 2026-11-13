//! Owned scratch cleanup after independently draining its resource Job.
//! cap-std owns handle-relative traversal/removal, Windows SDK owns ACLs.
use crate::security::{self, AclPolicy};
use anyhow::{Result, ensure};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use std::{
    fs,
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt as _},
        io::AsRawHandle,
    },
    path::Path,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE},
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        GetFileInformationByHandle, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
    },
};

pub(crate) fn remove_reclaimed(path: &Path) -> Result<()> {
    security::require_system()?;
    let private = AclPolicy::private_vm();
    private.verify(path, true)?;
    let _privileges = crate::persona::Persona::cleanup_privileges()?;
    remove_with_policy(path, &private)
}
fn remove_with_policy(path: &Path, policy: &AclPolicy) -> Result<()> {
    policy.verify(path, true)?;
    let root = fs::OpenOptions::new()
        .access_mode(access())
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    ensure!(
        root.metadata()?.is_dir()
            && root.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "cleanup root is not an owned directory"
    );
    let directory = Dir::from_std_file(root);
    let mut budget = Budget {
        remaining: 100_000,
        deadline: Instant::now() + Duration::from_secs(30),
    };
    contents(&directory, policy, &mut budget, 0)?;
    drop(directory);
    fs::remove_dir(path)?;
    Ok(())
}
fn access() -> u32 {
    GENERIC_READ | GENERIC_WRITE | READ_CONTROL | WRITE_DAC | WRITE_OWNER
}
struct Budget {
    remaining: usize,
    deadline: Instant,
}
fn contents(directory: &Dir, policy: &AclPolicy, budget: &mut Budget, depth: usize) -> Result<()> {
    ensure!(depth <= 128, "owned cleanup exceeds directory depth bound");
    for entry in directory.entries()? {
        ensure!(
            budget.remaining > 0 && Instant::now() < budget.deadline,
            "owned cleanup exceeds entry/deadline bound"
        );
        budget.remaining -= 1;
        let entry = entry?;
        let mut options = OpenOptions::new();
        options
            .access_mode(access())
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .maybe_dir(true)
            .follow(FollowSymlinks::No);
        let held = match entry.open_with(&options) {
            Ok(file) => file.into_std(),
            Err(open_error) => {
                // Links are never opened through their destination. Removing
                // an entry relative to its held parent cannot change outside
                // ACLs; a nonempty/inaccessible directory still fails closed.
                if entry.remove_file().is_err() {
                    entry.remove_dir().map_err(|remove_error| anyhow::anyhow!("owned cleanup open failed: {open_error}; entry removal failed: {remove_error}"))?;
                }
                continue;
            }
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        security::check(unsafe { GetFileInformationByHandle(held.as_raw_handle(), &mut info) })?;
        let is_directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
            // Never modify a regular file's ACL/attributes if another hard
            // link can expose those changes outside this resource directory.
            if is_directory || info.nNumberOfLinks == 1 {
                policy.apply_handle(&held)?;
                if !is_directory && held.metadata()?.permissions().readonly() {
                    let mut permissions = held.metadata()?.permissions();
                    #[allow(
                        clippy::permissions_set_readonly_false,
                        reason = "Windows-only held file: clears READONLY without changing its ACL"
                    )]
                    permissions.set_readonly(false);
                    held.set_permissions(permissions)?;
                }
            }
            if is_directory {
                contents(
                    &Dir::from_std_file(held.try_clone()?),
                    policy,
                    budget,
                    depth + 1,
                )?;
            }
        }
        drop(held);
        if is_directory {
            entry.remove_dir()?;
        } else {
            entry.remove_file()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capability_cleanup_unlinks_hardlinks_and_junctions_without_modifying_outside_identity()
    -> Result<()> {
        let root = tempfile::tempdir()?;
        let policy = AclPolicy::current_user()?;
        policy.apply(root.path(), true)?;
        let outside = root.path().join("outside");
        policy.create_directory(&outside)?;
        let sentinel = outside.join("keep.bin");
        fs::write(&sentinel, b"outside-original")?;
        policy.apply(&sentinel, false)?;
        let scratch = root.path().join("scratch");
        policy.create_directory(&scratch)?;
        fs::hard_link(&sentinel, scratch.join("alias.bin"))?;
        junction::create(&outside, scratch.join("junction"))?;
        policy.create_directory(&scratch.join("ordinary"))?;
        fs::write(scratch.join("ordinary/data"), b"owned")?;
        remove_with_policy(&scratch, &policy)?;
        assert!(!scratch.exists());
        assert_eq!(fs::read(&sentinel)?, b"outside-original");
        policy.verify(&sentinel, false)?;
        policy.verify(&outside, true)?;
        Ok(())
    }
}
