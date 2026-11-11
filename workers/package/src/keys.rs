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

/// Create/read one stable signing identity in an existing private controller
/// parent. This function never logs or exports private bytes. Windows uses
/// current-user DPAPI and a protected user/SYSTEM ACL; Unix uses owner-only
/// files. Existing unsafe directories are rejected, never silently repaired.
pub fn load_controller_signer(directory: &Path) -> Result<Signer> {
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
