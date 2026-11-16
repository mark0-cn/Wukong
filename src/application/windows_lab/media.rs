//! Offline installation media: the verified package directory plus the
//! hash-pinned `install.ps1` the administrative channel executes in the guest.
use super::package::PackageManifest;
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub const INSTALL_SCRIPT: &str = "install.ps1";

#[derive(Debug, Clone)]
pub struct MediaFile {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Media {
    pub root: PathBuf,
    pub sha256: String,
    pub package_sha256: String,
    pub script_sha256: String,
    pub files: Vec<MediaFile>,
}

pub fn prepare(store_root: &Path, package_sha256: &str) -> Result<Media> {
    ensure!(
        package_sha256.len() == 64 && package_sha256.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid package identity"
    );
    let package = store_root.join("packages").join(package_sha256);
    let manifest: PackageManifest = serde_json::from_slice(
        &fs::read(package.join("manifest.json"))
            .with_context(|| format!("read package manifest {}", package.display()))?,
    )?;
    let script = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.name == INSTALL_SCRIPT)
        .with_context(|| format!("offline media requires a packaged {INSTALL_SCRIPT}"))?;
    let script_bytes = fs::read(package.join(INSTALL_SCRIPT))?;
    ensure!(
        script_bytes.len() as u64 == script.size
            && hex::encode(Sha256::digest(&script_bytes)) == script.sha256,
        "install script failed verification"
    );
    let mut files = vec![
        MediaFile {
            name: "manifest.json".into(),
            path: package.join("manifest.json"),
        },
        MediaFile {
            name: "SHA256SUMS".into(),
            path: package.join("SHA256SUMS"),
        },
    ];
    for artifact in &manifest.artifacts {
        let path = package.join(&artifact.name);
        let bytes = fs::read(&path)?;
        ensure!(
            bytes.len() as u64 == artifact.size
                && hex::encode(Sha256::digest(&bytes)) == artifact.sha256,
            "package member {} failed verification",
            artifact.name
        );
        files.push(MediaFile {
            name: artifact.name.clone(),
            path,
        });
    }
    Ok(Media {
        root: package,
        sha256: package_sha256.to_owned(),
        package_sha256: package_sha256.to_owned(),
        script_sha256: script.sha256.clone(),
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::windows_lab::package::{self, Dependency, DependencySource};

    async fn package_with(dir: &Path, names: &[(&str, &[u8])]) -> package::Package {
        let mut dependencies = Vec::new();
        for (name, bytes) in names {
            let path = dir.join(name);
            fs::write(&path, bytes).unwrap();
            dependencies.push(Dependency {
                name: (*name).into(),
                sha256: hex::encode(Sha256::digest(bytes)),
                source: DependencySource::Path { path },
            });
        }
        package::materialize(&dir.join("packages"), &dependencies)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn media_lists_verified_package_members() {
        let dir = tempfile::tempdir().unwrap();
        let package = package_with(
            dir.path(),
            &[("install.ps1", b"Write-Output install"), ("app.exe", b"MZ")],
        )
        .await;
        let media = prepare(dir.path(), &package.id).unwrap();
        assert_eq!(media.sha256, package.id);
        assert!(media.files.iter().any(|file| file.name == "install.ps1"));
        assert!(media.files.iter().any(|file| file.name == "app.exe"));
        assert_eq!(
            media.script_sha256,
            hex::encode(Sha256::digest(b"Write-Output install"))
        );
    }

    #[tokio::test]
    async fn media_requires_an_install_script() {
        let dir = tempfile::tempdir().unwrap();
        let package = package_with(dir.path(), &[("app.exe", b"MZ")]).await;
        assert!(prepare(dir.path(), &package.id).is_err());
    }
}
