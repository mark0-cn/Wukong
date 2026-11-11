//! Relocatable bulk cache; recovery journals and process locks stay at a stable address.
use super::{maintenance, manifest::Manifest, recovery};
use crate::i18n::Message as UiMessage;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct Location {
    directory: PathBuf,
}
pub fn cache_root() -> Result<PathBuf> {
    let control = recovery::root()?;
    let settings = control.join("cache-location.json");
    if settings.exists() {
        let location: Location = crate::workbench::read_json(&settings)?;
        ensure!(
            location.directory.is_absolute(),
            UiMessage::new("system-the-cache-directory-must-be-an-absolute-path")
        );
        Ok(location.directory)
    } else {
        Ok(control)
    }
}
pub fn cache_lock(exclusive: bool) -> Result<fs::File> {
    let root = recovery::root()?;
    cache_lock_at(&root, exclusive)
}
pub(crate) fn cache_lock_at(root: &Path, exclusive: bool) -> Result<fs::File> {
    fs::create_dir_all(root)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("cache.lock"))?;
    if exclusive {
        file.try_lock().context(UiMessage::new(
            "system-an-update-download-or-recovery-is-using-the",
        ))?;
    } else {
        file.try_lock_shared()
            .context(UiMessage::new("system-the-cache-location-is-changing"))?;
        ensure!(
            !root.join("cleanup-in-progress.json").exists(),
            "uninstall cleanup is in progress"
        );
    }
    Ok(file)
}

pub fn set_cache(destination: &Path) -> Result<PathBuf> {
    let _cache = cache_lock(true)?;
    let _maintenance = maintenance::lock(&recovery::root()?, true)?;
    ensure!(
        recovery::pending()?.is_none(),
        UiMessage::new("system-finish-the-pending-update-or-recovery-before-changing")
    );
    ensure!(
        destination.is_absolute(),
        UiMessage::new("system-choose-an-absolute-path")
    );
    fs::create_dir_all(destination)?;
    let destination = fs::canonicalize(destination)?;
    let source = cache_root()?;
    fs::create_dir_all(&source)?;
    let source = fs::canonicalize(source)?;
    if source == destination {
        return Ok(destination);
    }
    ensure!(
        !source.starts_with(&destination) && !destination.starts_with(&source),
        UiMessage::new("system-the-old-and-new-cache-directories-cannot-contain")
    );
    if let Some(program) = recovery::executable()?.parent() {
        ensure!(
            !destination.starts_with(fs::canonicalize(program)?),
            UiMessage::new("system-the-cache-cannot-be-inside-the-installation-directory")
        );
    }
    ensure!(
        fs::read_dir(&destination)?.next().is_none(),
        UiMessage::new("system-choose-an-empty-directory-existing-files-will-not")
    );
    crate::cleanup::register_cache(&crate::desktop::paths::home()?, &source)?;
    let staging = tempfile::tempdir_in(&destination)?;
    let releases = source.join("releases");
    if releases.exists() {
        copy_releases(&releases, &staging.path().join("releases"))?;
    }
    crate::cleanup::create_owned_directory(&destination.join("probes"))?;
    // The destination was verified empty before Wukong populated it.
    crate::workbench::write_json(
        &destination.join(".wukong-owned.json"),
        &serde_json::json!({"schema_version":1,"application":"io.wukong.desktop"}),
    )?;
    crate::cleanup::register_cache(&crate::desktop::paths::home()?, &destination)?;
    let staged = staging.path().join("releases");
    if staged.exists() {
        fs::rename(staged, destination.join("releases"))?;
    }
    crate::workbench::write_json(
        &recovery::root()?.join("cache-location.json"),
        &Location {
            directory: destination.clone(),
        },
    )?;
    // Old files are preserved. Only the small pointer switches after a complete copy.
    Ok(destination)
}

fn copy_releases(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        ensure!(
            entry.file_type()?.is_dir() && !entry.file_type()?.is_symlink(),
            UiMessage::new("system-the-cache-contains-an-unrecognized-version-directory")
        );
        let version = entry.file_name().to_string_lossy().to_string();
        semver::Version::parse(&version)?;
        let path = entry.path();
        let bytes = fs::read(path.join("latest.json"))?;
        let signature = fs::read_to_string(path.join("latest.json.sig"))?;
        let manifest = Manifest::parse(&bytes, &signature)?;
        ensure!(
            manifest.version == version,
            UiMessage::new("system-cache-version-identity-mismatch")
        );
        let output = destination.join(&version);
        fs::create_dir_all(&output)?;
        for file in ["latest.json", "latest.json.sig"] {
            fs::copy(path.join(file), output.join(file))?;
        }
        let artifact = manifest.artifact()?;
        let filename = artifact.url.rsplit('/').next().unwrap();
        let payload = path.join(filename);
        if payload.exists() {
            ensure!(
                !payload.is_symlink(),
                UiMessage::new("system-cache-files-cannot-be-symbolic-links")
            );
            super::manifest::verify_file(&payload, artifact)?;
            ensure!(
                fs2::available_space(&output)? > artifact.size + 64 * 1024 * 1024,
                UiMessage::new("system-insufficient-space-in-the-new-cache-directory")
            );
            fs::copy(payload, output.join(filename))?;
            super::manifest::verify_file(&output.join(filename), artifact)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_release_cannot_be_migrated() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let input = dir.path().join("source/0.2.0");
        fs::create_dir_all(&input)?;
        fs::write(input.join("latest.json"), b"{}")?;
        fs::write(input.join("latest.json.sig"), b"bad")?;
        let output = dir.path().join("destination");
        assert!(copy_releases(&dir.path().join("source"), &output).is_err());
        assert!(!output.exists());
        assert!(input.join("latest.json").exists());
        Ok(())
    }
}
