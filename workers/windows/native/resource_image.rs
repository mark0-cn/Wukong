//! Stage admitted logical files before any target instruction. The same held
//! source readers and sealed destination handles support programs and data.
use crate::{
    resource_input::{self, AdmittedInput, Purpose, Reference},
    resource_journal::Owner,
    security::{self, AclPolicy},
};
use anyhow::{Result, ensure};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

pub(crate) fn validate(
    program: &Reference,
    files: &[Reference],
    arguments: &[String],
) -> Result<()> {
    validate_paths(program, files)?;
    for file in files {
        ensure!(
            arguments.contains(&file.path),
            "native argument file is not a direct argv entry"
        );
    }
    for argument in arguments {
        if argument.starts_with("files/") && argument != &program.path {
            ensure!(
                files.iter().any(|file| &file.path == argument),
                "native argv file has no admitted image member"
            );
        }
    }
    Ok(())
}
pub(crate) fn validate_paths(program: &Reference, files: &[Reference]) -> Result<()> {
    ensure!(
        files.len() <= 64,
        "native process has too many argument files"
    );
    let mut names = BTreeSet::new();
    for file in std::iter::once(program).chain(files) {
        let name = file
            .path
            .strip_prefix("files/")
            .ok_or_else(|| anyhow::anyhow!("native image requires admitted files/... paths"))?;
        wukong_worker_package::validate_path(name)?;
        ensure!(
            file.path.split('/').count() <= crate::resource_cleanup::MAX_DEPTH,
            "native image exceeds cleanup directory depth"
        );
        let key = file.path.to_ascii_lowercase();
        ensure!(
            names.insert(key),
            "native image has case-aliased or duplicate members"
        );
    }
    for name in &names {
        for ancestor in Path::new(name).ancestors().skip(1) {
            let key = ancestor.to_string_lossy().replace('\\', "/");
            ensure!(
                !names.contains(&key),
                "native image file overlaps a member directory"
            );
        }
    }
    Ok(())
}

pub(crate) fn acquire(
    metadata: &Path,
    owner: &Owner,
    program: &Reference,
    files: &[Reference],
) -> Result<Vec<AdmittedInput>> {
    std::iter::once(program)
        .chain(files)
        .map(|reference| {
            resource_input::open(
                metadata,
                owner,
                reference,
                if reference.executable {
                    Purpose::Execute
                } else {
                    Purpose::Inspect
                },
            )
        })
        .collect()
}

/// All destination parents are sealed while the target is absent. Retain the
/// returned handles through Job drain, denying writes and delete sharing.
pub(crate) fn stage(
    scratch: &Path,
    sources: Vec<AdmittedInput>,
    executable_policy: &AclPolicy,
    working: Option<&security::ResourceDirectory>,
    allow_execution: bool,
) -> Result<Vec<fs::File>> {
    let private = AclPolicy::private_vm();
    stage_with_policy(
        scratch,
        sources,
        executable_policy,
        &private,
        working.map(|working| working.path()),
        allow_execution,
    )
}
fn stage_with_policy(
    scratch: &Path,
    sources: Vec<AdmittedInput>,
    executable_policy: &AclPolicy,
    private: &AclPolicy,
    working: Option<&Path>,
    allow_execution: bool,
) -> Result<Vec<fs::File>> {
    let mut held = Vec::with_capacity(sources.len());
    for mut source in sources {
        let reference = source.reference().clone();
        let relative = Path::new(&reference.path);
        let mut parents: Vec<PathBuf> = relative
            .parent()
            .into_iter()
            .flat_map(Path::ancestors)
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_owned)
            .collect();
        parents.reverse();
        for parent in parents {
            let path = scratch.join(parent);
            if !path.exists() {
                executable_policy.create_directory(&path)?;
            }
            let guarded = match working {
                Some(working) => path.canonicalize()? == working,
                None => false,
            };
            if !guarded {
                executable_policy.verify(&path, true)?;
            }
        }
        let destination = scratch.join(relative);
        let mut file = private.create_file(&destination)?;
        ensure!(
            std::io::copy(&mut source, &mut file)? == reference.size,
            "native staged file length differs"
        );
        file.flush()?;
        file.sync_all()?;
        drop(file);
        let policy = if allow_execution && reference.executable {
            executable_policy.clone()
        } else {
            executable_policy.read_only_data()
        };
        policy.apply(&destination, false)?;
        security::no_links(&destination)?;
        held.push(
            fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(destination)?,
        );
    }
    Ok(held)
}

pub(crate) fn working_directory(
    scratch: &Path,
    cwd: &str,
    app: &str,
    logon: &str,
    policy: &AclPolicy,
) -> Result<security::ResourceDirectory> {
    crate::resource_launch::logical(cwd)?;
    let relative = Path::new(cwd);
    let mut parents: Vec<_> = relative
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .filter(|path| !path.as_os_str().is_empty())
        .collect();
    parents.reverse();
    for parent in parents {
        let path = scratch.join(parent);
        if !path.exists() {
            policy.create_directory(&path)?;
        }
        policy.verify(&path, true)?;
    }
    security::ResourceDirectory::create(&scratch.join(relative), app, logon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    fn reference(path: &str, executable: bool) -> Reference {
        Reference {
            path: path.into(),
            id: "a".repeat(64),
            sha256: hex::encode(Sha256::digest(b"pinned fixture")),
            size: 14,
            executable,
        }
    }
    #[test]
    fn argument_members_reject_aliases_ancestors_and_unreferenced_files() -> Result<()> {
        let program = reference("files/bin/target.exe", true);
        let data = reference("files/data/input.bin", false);
        validate(
            &program,
            std::slice::from_ref(&data),
            std::slice::from_ref(&data.path),
        )?;
        for path in [
            "files/BIN/target.exe",
            "files/bin",
            "files/bin/target.exe/child",
            "files/data/../input",
            "files/data/NUL",
            "metadata/secret",
        ] {
            let file = reference(path, false);
            assert!(
                validate(&program, std::slice::from_ref(&file), &[path.into()]).is_err(),
                "{path}"
            );
        }
        assert!(validate(&program, &[data], &[]).is_err());
        assert!(validate(&program, &[], &["files/missing.bin".into()]).is_err());
        Ok(())
    }
    #[test]
    fn staged_image_retains_paths_readonly_data_and_no_write_delete_sharing() -> Result<()> {
        check_staged_access(true)
    }
    #[test]
    fn dump_staging_denies_execution_even_for_executable_image_members() -> Result<()> {
        check_staged_access(false)
    }
    fn check_staged_access(allow_execution: bool) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Security::{
                Authorization::SE_FILE_OBJECT, CreateRestrictedToken, DISABLE_MAX_PRIVILEGE,
                TOKEN_DUPLICATE, TOKEN_QUERY,
            },
            Storage::FileSystem::{
                DELETE, FILE_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, WRITE_DAC, WRITE_OWNER,
            },
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        };
        let temp = tempfile::tempdir()?;
        let private = AclPolicy::current_user()?;
        let mut token = std::ptr::null_mut();
        security::check(unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE,
                &mut token,
            )
        })?;
        let original = security::Handle(token);
        let mut token = std::ptr::null_mut();
        // Administrator test runners can have SeTakeOwnershipPrivilege enabled.
        // Model ordinary sandbox DACL access, not that privilege's bypass.
        security::check(unsafe {
            CreateRestrictedToken(
                original.0,
                DISABLE_MAX_PRIVILEGE,
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                &mut token,
            )
        })?;
        let token = security::Handle(token);
        let logon = security::token_logon_sid(&token)?;
        const APP: &str = "S-1-15-2-101-102-103-104-105-106-107";
        let image_policy = private.clone().with_resource_code(APP, &logon)?;
        private.apply(temp.path(), true)?;
        let scratch = temp.path().join("image");
        private.create_directory(&scratch)?;
        let references = [
            reference("files/bin/target.exe", true),
            reference("files/data/input.bin", false),
        ];
        let mut sources = Vec::new();
        for (index, reference) in references.iter().enumerate() {
            let path = temp.path().join(format!("source-{index}"));
            fs::write(&path, b"pinned fixture")?;
            sources.push(AdmittedInput::new(
                fs::OpenOptions::new()
                    .read(true)
                    .share_mode(FILE_SHARE_READ)
                    .open(path)?,
                reference.clone(),
            ));
        }
        let handles = stage_with_policy(
            &scratch,
            sources,
            &image_policy,
            &private,
            None,
            allow_execution,
        )?;
        assert_eq!(handles.len(), 2);
        for reference in &references {
            let path = scratch.join(&reference.path);
            assert_eq!(fs::read(&path)?, b"pinned fixture");
            assert_eq!(
                fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .unwrap_err()
                    .raw_os_error(),
                Some(32)
            );
            assert_eq!(fs::remove_file(&path).unwrap_err().raw_os_error(), Some(32));
            let expected = if allow_execution && reference.executable {
                image_policy.clone()
            } else {
                image_policy.read_only_data()
            };
            expected.verify(&path, false)?;
            let file = fs::File::open(&path)?;
            let descriptor =
                security::SecurityDescriptor::from_object(file.as_raw_handle(), SE_FILE_OBJECT)?;
            for sid in [APP, &logon] {
                assert!(security::test_authz::allows(
                    &descriptor,
                    &token,
                    sid,
                    FILE_GENERIC_READ
                )?);
                assert_eq!(
                    security::test_authz::allows(&descriptor, &token, sid, FILE_EXECUTE)?,
                    allow_execution && reference.executable
                );
                for desired in [FILE_GENERIC_WRITE, DELETE, WRITE_DAC, WRITE_OWNER] {
                    assert!(
                        !security::test_authz::allows(&descriptor, &token, sid, desired)?,
                        "{sid} unexpectedly received {desired:#x} for {}",
                        reference.path
                    );
                }
            }
            for other in [
                "S-1-15-2-201-202-203-204-205-206-207",
                "S-1-5-5-201-202",
                "S-1-5-32-545", // All ordinary users must not read target data.
                "S-1-15-2-1",   // Nor every AppContainer on the VM.
            ] {
                assert!(!security::test_authz::allows(
                    &descriptor,
                    &token,
                    other,
                    FILE_GENERIC_READ
                )?);
            }
        }
        drop(handles);
        fs::remove_dir_all(&scratch)?;
        Ok(())
    }
}
