//! Operator-only controller signing keys. Never call from a worker RPC handler.
use crate::{Signer, files};
use anyhow::{Context, Result, ensure};
use ring::rand::{SecureRandom, SystemRandom};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};
use zeroize::Zeroizing;

#[cfg(windows)]
#[path = "keys_windows.rs"]
mod platform;

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

    pub fn directory(path: &Path) -> Result<()> {
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        permissions(path, true)
    }
    pub fn permissions(path: &Path, directory: bool) -> Result<()> {
        let metadata = files::reject_link(path)?;
        ensure!(
            metadata.is_dir() == directory && (directory || metadata.is_file()),
            "invalid controller key entry"
        );
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o777 == if directory { 0o700 } else { 0o600 },
            "controller key permissions are not private"
        );
        Ok(())
    }
    pub fn create(path: &Path) -> Result<fs::File> {
        Ok(fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?)
    }
    pub fn protect(bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        Ok(Zeroizing::new(bytes.to_vec()))
    }
    pub fn unprotect(bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        Ok(Zeroizing::new(bytes.to_vec()))
    }
}

/// Reject relative, aliased or non-existent-parent controller key locations.
fn validate_directory(directory: &Path) -> Result<()> {
    ensure!(
        directory.is_absolute(),
        "controller key directory must be absolute"
    );
    ensure!(
        !directory.components().any(|component| matches!(
            component,
            std::path::Component::CurDir | std::path::Component::ParentDir
        )),
        "controller key directory must not contain relative aliases"
    );
    files::package_root(
        directory
            .parent()
            .context("missing controller key parent")?,
    )?;
    Ok(())
}

/// Create one new protected controller identity in a directory that must not
/// exist yet. This never overwrites an existing key directory and removes only
/// its own partially created files on failure. Windows uses current-user DPAPI
/// and a protected user/SYSTEM ACL; Unix uses owner-only files.
pub fn create_controller_signer(directory: &Path) -> Result<Signer> {
    validate_directory(directory)?;
    ensure!(
        !directory.try_exists()?,
        "controller key directory already exists"
    );
    platform::directory(directory)?;
    struct Cleanup<'a> {
        directory: &'a Path,
        remove_file: bool,
        armed: bool,
    }
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            if !self.armed {
                return;
            }
            if self.remove_file {
                let _ = fs::remove_file(self.directory.join("signing-key.bin"));
            }
            // Only removes an empty directory; a concurrent winner is kept.
            let _ = fs::remove_dir(self.directory);
        }
    }
    let mut cleanup = Cleanup {
        directory,
        remove_file: false,
        armed: true,
    };
    let path = directory.join("signing-key.bin");
    let (signer, document) = Signer::generate()?;
    let stored = platform::protect(document.as_ref())?;
    let mut output = {
        let file = platform::create(&path)?;
        cleanup.remove_file = true;
        file
    };
    output.write_all(&stored)?;
    output.sync_all()?;
    drop(output);
    platform::permissions(&path, false)?;
    cleanup.armed = false;
    Ok(signer)
}

/// Create/read one stable signing identity in an existing private controller
/// parent. This function never logs or exports private bytes. Windows uses
/// current-user DPAPI and a protected user/SYSTEM ACL; Unix uses owner-only
/// files. Existing unsafe directories are rejected, never silently repaired.
pub fn load_controller_signer(directory: &Path) -> Result<Signer> {
    validate_directory(directory)?;
    platform::directory(directory)?;
    files::package_root(directory)?;
    let path = directory.join("signing-key.bin");
    if !path.try_exists()? {
        let (_, document) = Signer::generate()?;
        let stored = platform::protect(document.as_ref())?;
        let mut random = [0u8; 16];
        SystemRandom::new()
            .fill(&mut random)
            .map_err(|_| anyhow::anyhow!("key publication random source failed"))?;
        let temporary = directory.join(format!(".key-{}", hex::encode(random)));
        let mut output = platform::create(&temporary)?;
        // Only this newly created file is removed, including on failure.
        struct Cleanup<'a>(&'a Path);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let _ = fs::remove_file(self.0);
            }
        }
        let cleanup = Cleanup(&temporary);
        output.write_all(&stored)?;
        output.sync_all()?;
        drop(output);
        match fs::hard_link(&temporary, &path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        drop(cleanup);
    }
    platform::permissions(&path, false)?;
    let mut stored = Zeroizing::new(Vec::new());
    fs::File::open(&path)?
        .take(64 * 1024 + 1)
        .read_to_end(&mut stored)?;
    ensure!(
        !stored.is_empty() && stored.len() <= 64 * 1024,
        "invalid controller signing key size"
    );
    let private = platform::unprotect(&stored)?;
    Signer::from_pkcs8(&private)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_publishes_one_protected_identity_without_overwriting() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let keys = temporary.path().join("controller");
        let signer = create_controller_signer(&keys)?;
        let stored = keys.join("signing-key.bin");
        assert!(files::reject_link(&stored)?.is_file());
        assert_eq!(fs::read_dir(&keys)?.count(), 1);
        let loaded = load_controller_signer(&keys)?;
        assert_eq!(signer.public_key().to_hex(), loaded.public_key().to_hex());
        assert!(
            create_controller_signer(&keys).is_err(),
            "creation must never overwrite an existing identity"
        );
        assert!(create_controller_signer(Path::new("relative-controller")).is_err());
        #[cfg(windows)]
        assert!(
            Signer::from_pkcs8(&fs::read(&stored)?).is_err(),
            "the created key file must be DPAPI protected"
        );
        Ok(())
    }

    #[test]
    fn concurrent_creation_keeps_the_winner_and_never_deletes_it() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let keys = temporary.path().join("controller");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let keys = keys.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    create_controller_signer(&keys).map(|signer| signer.public_key().to_hex())
                })
            })
            .collect();
        let created: Vec<String> = threads
            .into_iter()
            .filter_map(|thread| thread.join().expect("key creation thread panicked").ok())
            .collect();
        assert_eq!(created.len(), 1, "exactly one creator may win: {created:?}");
        let loaded = load_controller_signer(&keys)?;
        assert_eq!(loaded.public_key().to_hex(), created[0]);
        Ok(())
    }
}
