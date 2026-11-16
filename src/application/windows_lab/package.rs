//! Controller-side dependency locking and offline package assembly.
//!
//! URLs and paths are operator input; every byte is verified against the
//! declared SHA-256 before it enters an immutable package directory. The
//! package ID is the hash of the canonical manifest, so equal inputs produce
//! one reusable directory.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

pub const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub name: String,
    pub sha256: String,
    pub source: DependencySource,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DependencySource {
    Url { url: String },
    Path { path: PathBuf },
}

impl Dependency {
    fn validate(&self) -> Result<()> {
        ensure!(
            valid_name(&self.name) && !reserved_name(&self.name),
            "dependency name must be 1–128 safe filename characters"
        );
        ensure!(
            lowercase_digest(&self.sha256),
            "dependency {} needs a 64-character lowercase SHA-256",
            self.name
        );
        match &self.source {
            DependencySource::Url { url } => {
                ensure!(
                    !url.trim().is_empty()
                        && url.len() <= 4096
                        && !url.chars().any(char::is_whitespace)
                        && (url.starts_with("https://") || url.starts_with("http://")),
                    "dependency {} has an unsupported URL",
                    self.name
                );
            }
            DependencySource::Path { path } => ensure!(
                path.is_absolute() && path.is_file(),
                "dependency {} path must be an absolute existing file",
                self.name
            ),
        }
        Ok(())
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// Artifact names select one packaged file; they never contain a path.
pub fn valid_artifact_name(name: &str) -> bool {
    valid_name(name)
}

fn lowercase_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PackageArtifact {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PackageManifest {
    pub schema: u32,
    pub artifacts: Vec<PackageArtifact>,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug)]
pub struct Package {
    pub id: String,
    pub root: PathBuf,
    pub artifacts: Vec<PackageArtifact>,
}

/// Validate every declared dependency and reject case-insensitive name clashes.
pub fn validate_dependencies(dependencies: &[Dependency]) -> Result<()> {
    ensure!(
        dependencies.len() <= 256,
        "package supports at most 256 dependencies"
    );
    let mut names = std::collections::BTreeSet::new();
    for dependency in dependencies {
        dependency.validate()?;
        ensure!(
            names.insert(dependency.name.to_ascii_lowercase()),
            "duplicate dependency name {}",
            dependency.name
        );
    }
    Ok(())
}

/// Fetch/copy every dependency, verify it, and publish one immutable package.
pub async fn materialize(root: &Path, dependencies: &[Dependency]) -> Result<Package> {
    ensure!(
        !dependencies.is_empty(),
        "offline package needs at least one dependency"
    );
    validate_dependencies(dependencies)?;
    fs::create_dir_all(root).with_context(|| format!("create package root {}", root.display()))?;
    let staging = root.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir_all(&staging)?;
    let result = assemble(root, &staging, dependencies).await;
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

async fn assemble(root: &Path, staging: &Path, dependencies: &[Dependency]) -> Result<Package> {
    let mut artifacts = Vec::with_capacity(dependencies.len());
    let mut total = 0_u64;
    for dependency in dependencies {
        let bytes = read_source(&dependency.source, &dependency.name).await?;
        let size = bytes.len() as u64;
        ensure!(
            size <= MAX_ARTIFACT_BYTES,
            "dependency {} exceeds the per-artifact bound",
            dependency.name
        );
        total = total
            .checked_add(size)
            .context("dependency total overflow")?;
        ensure!(
            total <= MAX_PACKAGE_BYTES,
            "offline package exceeds the total size bound"
        );
        let hash = hex::encode(Sha256::digest(&bytes));
        ensure!(
            hash == dependency.sha256,
            "dependency {} hash mismatch: expected {}, got {}",
            dependency.name,
            dependency.sha256,
            hash
        );
        let path = staging.join(&dependency.name);
        let mut file = fs::File::create(&path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        artifacts.push(PackageArtifact {
            name: dependency.name.clone(),
            sha256: hash,
            size,
        });
    }
    artifacts.sort_by(|a, b| a.name.cmp(&b.name));
    let manifest = PackageManifest {
        schema: 1,
        artifacts: artifacts.clone(),
    };
    let canonical = serde_json::to_vec(&manifest)?;
    let id = hex::encode(Sha256::digest(&canonical));
    fs::write(staging.join("manifest.json"), &canonical)?;
    let sums = artifacts
        .iter()
        .map(|artifact| format!("{}  {}\n", artifact.sha256, artifact.name))
        .collect::<String>();
    fs::write(staging.join("SHA256SUMS"), sums)?;
    let final_root = root.join(&id);
    if final_root.exists() {
        fs::remove_dir_all(staging)?;
        verify_existing(&final_root, &manifest, &id)?;
    } else {
        fs::rename(staging, &final_root)
            .with_context(|| format!("publish package {}", final_root.display()))?;
        verify_existing(&final_root, &manifest, &id)?;
    }
    Ok(Package {
        id,
        root: final_root,
        artifacts,
    })
}

async fn read_source(source: &DependencySource, name: &str) -> Result<Vec<u8>> {
    match source {
        DependencySource::Path { path } => {
            let metadata = fs::metadata(path)?;
            ensure!(
                metadata.len() <= MAX_ARTIFACT_BYTES,
                "dependency {name} exceeds the per-artifact bound"
            );
            read_bounded(path, MAX_ARTIFACT_BYTES)
                .with_context(|| format!("read dependency {name}"))
        }
        DependencySource::Url { url } => {
            let client = reqwest::Client::builder()
                .timeout(FETCH_TIMEOUT)
                .user_agent("wukong-windows-lab")
                .build()?;
            let response = client
                .get(url)
                .send()
                .await
                .with_context(|| format!("download dependency {name}"))?;
            let response = response
                .error_for_status()
                .with_context(|| format!("download dependency {name}"))?;
            if let Some(length) = response.content_length() {
                ensure!(
                    length <= MAX_ARTIFACT_BYTES,
                    "dependency {name} exceeds the per-artifact bound"
                );
            }
            let bytes = response
                .bytes()
                .await
                .with_context(|| format!("read dependency {name}"))?;
            ensure!(
                bytes.len() as u64 <= MAX_ARTIFACT_BYTES,
                "dependency {name} exceeds the per-artifact bound"
            );
            Ok(bytes.to_vec())
        }
    }
}

/// Re-open by immutable identity; a rewritten manifest cannot bless changed bytes.
pub fn open_verified(root: &Path, id: &str) -> Result<PackageManifest> {
    ensure!(lowercase_digest(id), "invalid package identity");
    crate::adapters::files::safety::safe(root)?;
    let manifest_path = root.join("manifest.json");
    ensure!(
        fs::symlink_metadata(root)?.is_dir()
            && !fs::symlink_metadata(root)?.file_type().is_symlink(),
        "package directory must not be a link"
    );
    ensure!(
        fs::symlink_metadata(&manifest_path)?.is_file()
            && fs::metadata(&manifest_path)?.len() <= 1024 * 1024,
        "invalid package manifest file"
    );
    let manifest: PackageManifest =
        serde_json::from_slice(&read_bounded(&manifest_path, 1024 * 1024)?)?;
    ensure!(
        manifest.schema == 1 && manifest.artifacts.len() <= 256,
        "unsupported package manifest"
    );
    ensure!(
        hex::encode(Sha256::digest(serde_json::to_vec(&manifest)?)) == id,
        "package manifest identity mismatch"
    );
    let mut names = std::collections::BTreeSet::new();
    let mut total = 0u64;
    for artifact in &manifest.artifacts {
        ensure!(
            valid_name(&artifact.name)
                && !reserved_name(&artifact.name)
                && lowercase_digest(&artifact.sha256)
                && names.insert(artifact.name.to_ascii_lowercase()),
            "invalid package member"
        );
        ensure!(
            artifact.size <= MAX_ARTIFACT_BYTES,
            "package member too large"
        );
        total = total
            .checked_add(artifact.size)
            .context("package size overflow")?;
        ensure!(total <= MAX_PACKAGE_BYTES, "package exceeds size budget");
        let path = root.join(&artifact.name);
        crate::adapters::files::safety::safe(&path)?;
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() == artifact.size,
            "package member metadata changed"
        );
        let bytes = read_bounded(&path, MAX_ARTIFACT_BYTES)?;
        ensure!(
            bytes.len() as u64 == artifact.size
                && hex::encode(Sha256::digest(&bytes)) == artifact.sha256,
            "package member failed verification"
        );
    }
    let expected_sums = manifest
        .artifacts
        .iter()
        .map(|a| format!("{}  {}\n", a.sha256, a.name))
        .collect::<String>();
    ensure!(
        read_bounded(&root.join("SHA256SUMS"), 1024 * 1024)? == expected_sums.as_bytes(),
        "package checksum index differs from the bound manifest"
    );
    Ok(manifest)
}

fn verify_existing(root: &Path, manifest: &PackageManifest, id: &str) -> Result<()> {
    let stored = open_verified(root, id)?;
    ensure!(stored == *manifest, "existing package manifest differs");
    ensure!(
        hex::encode(Sha256::digest(serde_json::to_vec(&stored)?)) == id,
        "existing package identity mismatch"
    );
    for artifact in &manifest.artifacts {
        let path = root.join(&artifact.name);
        let bytes = fs::read(&path)
            .with_context(|| format!("read packaged artifact {}", path.display()))?;
        ensure!(
            bytes.len() as u64 == artifact.size
                && hex::encode(Sha256::digest(&bytes)) == artifact.sha256,
            "existing package artifact {} failed verification",
            artifact.name
        );
    }
    Ok(())
}

fn reserved_name(name: &str) -> bool {
    [
        "manifest.json",
        "sha256sums",
        "wukong-receipt.ps1",
        "receipt.json",
        "receipt.tmp",
    ]
    .contains(&name.to_ascii_lowercase().as_str())
}

pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    crate::adapters::files::safety::safe(path)?;
    let file = fs::File::open(path)?;
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() <= limit,
        "file exceeds package size budget"
    );
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "file grew beyond package size budget"
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(dir: &Path, name: &str, bytes: &[u8]) -> (Dependency, PathBuf) {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        (
            Dependency {
                name: name.into(),
                sha256: hex::encode(Sha256::digest(bytes)),
                source: DependencySource::Path { path: path.clone() },
            },
            path,
        )
    }

    #[tokio::test]
    async fn changing_manifest_and_member_cannot_rebind_an_existing_package() {
        let dir = tempfile::tempdir().unwrap();
        let (dependency, _) = source(dir.path(), "install.ps1", b"original");
        let package = materialize(&dir.path().join("packages"), &[dependency])
            .await
            .unwrap();
        let mut manifest = open_verified(&package.root, &package.id).unwrap();
        let replacement = b"modified";
        fs::write(package.root.join("install.ps1"), replacement).unwrap();
        manifest.artifacts[0].sha256 = hex::encode(Sha256::digest(replacement));
        fs::write(
            package.root.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(open_verified(&package.root, &package.id).is_err());
        assert!(crate::application::windows_lab::media::prepare(dir.path(), &package.id).is_err());
    }

    #[tokio::test]
    async fn packages_are_verified_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("packages");
        let (first, _) = source(dir.path(), "harness.c", b"int main(void){return 0;}");
        let (second, _) = source(dir.path(), "seed.bin", b"\x00\x01\x02");
        let package = materialize(&root, &[first.clone(), second.clone()])
            .await
            .unwrap();
        assert_eq!(package.artifacts.len(), 2);
        assert!(package.root.join("manifest.json").is_file());
        assert!(package.root.join("SHA256SUMS").is_file());
        assert_eq!(
            fs::read(package.root.join("harness.c")).unwrap(),
            b"int main(void){return 0;}"
        );
        let again = materialize(&root, &[second, first]).await.unwrap();
        assert_eq!(again.id, package.id);
        assert_eq!(again.root, package.root);
    }

    #[tokio::test]
    async fn wrong_hash_is_rejected_before_any_package_is_published() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("packages");
        let (mut dependency, _) = source(dir.path(), "harness.c", b"payload");
        dependency.sha256 = "a".repeat(64);
        let error = materialize(&root, &[dependency]).await.unwrap_err();
        assert!(format!("{error:#}").contains("hash mismatch"));
        assert!(
            !root.exists()
                || fs::read_dir(&root).unwrap().all(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with('.'))
        );
    }

    #[tokio::test]
    async fn unsafe_names_and_duplicates_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("packages");
        let (mut dependency, _) = source(dir.path(), "a.bin", b"a");
        for name in ["..", "a/b", "a b", ".hidden", ""] {
            dependency.name = name.into();
            assert!(materialize(&root, &[dependency.clone()]).await.is_err());
        }
        let (first, _) = source(dir.path(), "A.bin", b"a");
        let (mut second, _) = source(dir.path(), "b.bin", b"b");
        second.name = "a.bin".into();
        assert!(materialize(&root, &[first, second]).await.is_err());
    }

    #[tokio::test]
    async fn url_sources_are_downloaded_and_verified() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("packages");
        let payload = b"remote artifact";
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = socket.read(&mut request).await.unwrap();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            socket.write_all(header.as_bytes()).await.unwrap();
            socket.write_all(payload).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let dependency = Dependency {
            name: "remote.bin".into(),
            sha256: hex::encode(Sha256::digest(payload)),
            source: DependencySource::Url {
                url: format!("http://{address}/remote.bin"),
            },
        };
        let package = materialize(&root, &[dependency]).await.unwrap();
        assert_eq!(fs::read(package.root.join("remote.bin")).unwrap(), payload);
        server.await.unwrap();
    }

    #[test]
    fn dependencies_require_absolute_existing_paths() {
        let dependency = Dependency {
            name: "x".into(),
            sha256: "a".repeat(64),
            source: DependencySource::Path {
                path: PathBuf::from("relative.bin"),
            },
        };
        assert!(dependency.validate().is_err());
    }
}
