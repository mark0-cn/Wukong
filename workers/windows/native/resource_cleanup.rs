//! Owned scratch cleanup after independently draining its resource Job.
//! cap-std owns handle-relative traversal/removal, Windows SDK owns ACLs.
use crate::security::{self, AclPolicy};
use anyhow::{Context, Result, ensure};
use backon::{BlockingRetryable, ExponentialBuilder};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use std::{
    ffi::OsStr,
    fs,
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt as _},
        io::AsRawHandle,
    },
    path::Path,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::GENERIC_READ,
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        FILE_WRITE_ATTRIBUTES, GetFileInformationByHandle, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
    },
};

pub(crate) const MAX_DEPTH: usize = 128;
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
    // Older Windows rejects data-write access to READONLY files even in backup
    // mode. Metadata rights suffice; GENERIC_READ preserves backup intent.
    GENERIC_READ | READ_CONTROL | WRITE_DAC | WRITE_OWNER | FILE_WRITE_ATTRIBUTES
}
struct Budget {
    remaining: usize,
    deadline: Instant,
}
fn contents(directory: &Dir, policy: &AclPolicy, budget: &mut Budget, depth: usize) -> Result<()> {
    ensure!(
        depth <= MAX_DEPTH,
        "owned cleanup exceeds directory depth bound"
    );
    for entry in directory.entries()? {
        ensure!(
            budget.remaining > 0 && Instant::now() < budget.deadline,
            "owned cleanup exceeds entry/deadline bound"
        );
        budget.remaining -= 1;
        let entry = entry?;
        let name = entry.file_name();
        let mut options = OpenOptions::new();
        options
            .access_mode(access())
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .maybe_dir(true)
            .follow(FollowSymlinks::No);
        let held = match directory.open_with(&name, &options) {
            Ok(file) => file.into_std(),
            Err(open_error) => {
                // Links are never opened through their destination. Removing
                // an entry relative to its held parent cannot change outside
                // ACLs; a nonempty/inaccessible directory still fails closed.
                if directory.remove_file(&name).is_err() {
                    directory.remove_dir(&name).map_err(|remove_error| anyhow::anyhow!("owned cleanup open failed: {open_error}; entry removal failed: {remove_error}"))?;
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
        remove_entry(directory, &name, is_directory, budget.deadline)?;
    }
    Ok(())
}

/// Windows can still refuse deletion while an image mapping or external read
/// handle is being released. Repeat only removal, after reclamation and Job
/// drain; never repeat execution, ACL changes or tree traversal here.
fn remove_entry(
    directory: &Dir,
    name: &OsStr,
    is_directory: bool,
    deadline: Instant,
) -> Result<()> {
    (|| {
        if is_directory {
            directory.remove_dir(name)
        } else {
            directory.remove_file(name)
        }
    })
    .retry(
        ExponentialBuilder::default()
            .with_min_delay(Duration::from_millis(20))
            .with_max_delay(Duration::from_millis(200))
            .with_max_times(8)
            .with_total_delay(Some(deadline.saturating_duration_since(Instant::now()))),
    )
    .when(|error| matches!(error.raw_os_error(), Some(5 | 32)) && Instant::now() < deadline)
    .sleep(move |delay: Duration| {
        std::thread::sleep(delay.min(deadline.saturating_duration_since(Instant::now())));
    })
    .notify(|error, delay| {
        eprintln!("owned scratch removal pending for {name:?}: {error}; wait {delay:?}");
    })
    .call()
    .with_context(|| format!("remove owned scratch entry {name:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hold_image_mapping(
        path: &Path,
        ready: std::sync::mpsc::SyncSender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) -> Result<()> {
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
            PAGE_READONLY, SEC_IMAGE, UnmapViewOfFile,
        };
        struct View(MEMORY_MAPPED_VIEW_ADDRESS);
        impl Drop for View {
            fn drop(&mut self) {
                unsafe { UnmapViewOfFile(self.0) };
            }
        }
        // Map a copy of this trusted test executable as an image, without
        // executing it. Windows refuses deleting a live image section with 5.
        let file = fs::File::open(path)?;
        let section = security::Handle(unsafe {
            CreateFileMappingW(
                file.as_raw_handle(),
                std::ptr::null(),
                PAGE_READONLY | SEC_IMAGE,
                0,
                0,
                std::ptr::null(),
            )
        });
        ensure!(!section.0.is_null(), "cannot create trusted image mapping");
        let view = View(unsafe { MapViewOfFile(section.0, FILE_MAP_READ, 0, 0, 0) });
        ensure!(!view.0.Value.is_null(), "cannot map trusted image view");
        ready.send(())?;
        release.recv_timeout(Duration::from_secs(10))?;
        drop(view);
        Ok(())
    }

    #[test]
    fn cleanup_waits_for_a_real_image_mapping_to_be_released() -> Result<()> {
        image_mapping_cleanup(false)
    }

    #[test]
    fn cleanup_preserves_failure_while_an_image_mapping_remains() -> Result<()> {
        image_mapping_cleanup(true)
    }

    fn image_mapping_cleanup(keep_mapping: bool) -> Result<()> {
        let root = tempfile::tempdir()?;
        let policy = AclPolicy::current_user()?;
        policy.apply(root.path(), true)?;
        let scratch = root.path().join("image-scratch");
        policy.create_directory(&scratch)?;
        let path = scratch.join("trusted-test.exe");
        fs::copy(std::env::current_exe()?, &path)?;
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        std::thread::scope(|scope| -> Result<()> {
            let holder = scope.spawn(|| hold_image_mapping(&path, ready_tx, release_rx));
            ready_rx.recv_timeout(Duration::from_secs(5))?;
            assert_eq!(fs::remove_file(&path).unwrap_err().raw_os_error(), Some(5));
            if keep_mapping {
                let error = remove_with_policy(&scratch, &policy).unwrap_err();
                assert_eq!(
                    error
                        .downcast_ref::<std::io::Error>()
                        .and_then(std::io::Error::raw_os_error),
                    Some(5)
                );
                assert!(path.is_file());
                release_tx.send(())?;
                holder.join().expect("image holder panicked")?;
                remove_with_policy(&scratch, &policy)?;
            } else {
                scope.spawn(move || {
                    std::thread::sleep(Duration::from_millis(200));
                    release_tx.send(())
                });
                remove_with_policy(&scratch, &policy)?;
                holder.join().expect("image holder panicked")?;
            }
            assert!(!scratch.exists());
            Ok(())
        })
    }

    #[test]
    #[ignore = "dedicated VM System fixture: reclaim only a pinned packaged cleanup-adversary resource"]
    fn reclaim_pinned_cleanup_fixture_as_system() -> Result<()> {
        security::require_system()?;
        let directory = std::env::current_dir()?;
        let private = AclPolicy::private_vm();
        private.verify(&directory, true)?;
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Config {
            installation_id: String,
            resource_id: String,
            manifest_sha256: String,
        }
        let path = directory.join("fixture-reclaim.json");
        private.verify(&path, false)?;
        let config: Config = serde_json::from_slice(&crate::deployment::read_bytes(&path, 4096)?)?;
        crate::processes::resource_job::name(&config.installation_id, &config.resource_id)?;
        let root = crate::environment::program_data()?
            .join("WukongWorkers")
            .join(&config.installation_id);
        let (deployment, actual_root) =
            crate::deployment::load(&root.join("metadata/deployment.json"))?;
        ensure!(
            actual_root.canonicalize()? == root.canonicalize()?,
            "fixture installation root differs"
        );
        let manifest = deployment.verify(&root)?;
        ensure!(
            manifest.sha256() == config.manifest_sha256,
            "fixture signed payload differs"
        );
        // Exclude a live journal writer for the whole assisted fixture cleanup.
        let _writer = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(root.join("metadata/native-resources/writer.lock"))?;
        let path = root
            .join("metadata/resource-requests")
            .join(format!("{}.json", config.resource_id));
        private.verify(&path, false)?;
        let request: crate::resource_process::Request =
            serde_json::from_slice(&crate::deployment::read_bytes(&path, 65536)?)?;
        request.validate()?;
        ensure!(
            request.resource_id == config.resource_id
                && request.owner.identity.installation_id == config.installation_id
                && request.owner.identity.package_sha256 == config.manifest_sha256
                && request.program.path == "files/process/x64.exe"
                && request.program.sha256
                    == manifest.manifest().files["runtime/selftest/x64/wukong-probe.exe"].sha256
                && request.arguments == ["--cleanup-adversary"],
            "assisted cleanup accepts only the known signed readonly/ACL fixture"
        );
        let found = crate::processes::resource_job::drain(
            &config.installation_id,
            &config.resource_id,
            Duration::from_secs(15),
        )?;
        let nonce = crate::resource_process::nonce(&config.installation_id, &config.resource_id)?;
        let path = root
            .join("metadata")
            .join(format!("resource-scratch-{nonce}.json"));
        private.verify(&path, false)?;
        let intent: crate::resource_process::ScratchIntent =
            serde_json::from_slice(&crate::deployment::read_bytes(&path, 8192)?)?;
        ensure!(
            intent.schema == crate::resource_process::SCHEMA
                && intent.resource_id == config.resource_id
                && intent.installation_id == config.installation_id,
            "assisted fixture scratch ownership differs"
        );
        let scratch = root.join("resources").join(nonce);
        deployment
            .code_policy()?
            .verify(&root.join("resources"), true)?;
        security::ResourceDirectory::recover(
            &scratch,
            &intent.appcontainer_sid,
            &intent.logon_sid,
        )?;
        ensure!(!scratch.try_exists()?, "assisted fixture scratch remains");
        crate::deployment::publish_json(
            &directory.join("fixture-reclaimed.json"),
            &serde_json::json!({"scope":"assisted_known_cleanup_fixture","installation_id":config.installation_id,"resource_id":config.resource_id,"manifest_sha256":config.manifest_sha256,"guardian_found":found,"scratch_removed":true,"production_recovery_qualification":false}),
            &private,
        )?;
        Ok(())
    }
    #[test]
    fn cleanup_can_open_readonly_data_without_requesting_data_write_access() -> Result<()> {
        let root = tempfile::tempdir()?;
        let policy = AclPolicy::current_user()?;
        policy.apply(root.path(), true)?;
        let scratch = root.path().join("readonly-scratch");
        policy.create_directory(&scratch)?;
        let data = scratch.join("readonly.bin");
        fs::write(&data, b"readonly-owned-fixture")?;
        let mut permissions = fs::metadata(&data)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&data, permissions)?;
        remove_with_policy(&scratch, &policy)?;
        assert!(!scratch.exists());
        Ok(())
    }
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
