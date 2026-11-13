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
    ensure!(
        files.len() <= 64,
        "native process has too many argument files"
    );
    let mut names = BTreeSet::new();
    for (index, file) in std::iter::once(program).chain(files).enumerate() {
        let name = file
            .path
            .strip_prefix("files/")
            .ok_or_else(|| anyhow::anyhow!("native image requires admitted files/... paths"))?;
        wukong_worker_package::validate_path(name)?;
        ensure!(
            index == 0 || arguments.contains(&file.path),
            "native argument file is not a direct argv entry"
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
) -> Result<Vec<fs::File>> {
    let private = AclPolicy::private_vm();
    stage_with_policy(scratch, sources, executable_policy, &private)
}
fn stage_with_policy(
    scratch: &Path,
    sources: Vec<AdmittedInput>,
    executable_policy: &AclPolicy,
    private: &AclPolicy,
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
            executable_policy.verify(&path, true)?;
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
        let policy = if reference.executable {
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
        let temp = tempfile::tempdir()?;
        let private = AclPolicy::current_user()?;
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
        let handles = stage_with_policy(&scratch, sources, &private, &private)?;
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
            let expected = if reference.executable {
                private.clone()
            } else {
                private.read_only_data()
            };
            expected.verify(&path, false)?;
        }
        drop(handles);
        fs::remove_dir_all(&scratch)?;
        Ok(())
    }
}
