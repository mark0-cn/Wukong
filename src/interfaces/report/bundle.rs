//! File manifest and offline integrity checks for an exported report.
use crate::adapters::files::content::{file_identity as hash_file, sha256_bytes as digest};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub target_path: String,
    pub target_sha256: String,
    pub files: Vec<ManifestEntry>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleVerification {
    pub files: usize,
    pub bytes: u64,
    pub target_sha256: String,
}

pub(super) fn safe_relative(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && !path.contains(['\\', ':', '\n', '\r', '\0']),
        "invalid bundle path"
    );
    let path = Path::new(path);
    ensure!(
        !path.is_absolute()
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "bundle path escapes its root"
    );
    Ok(())
}

pub(super) fn checksum_text(manifest: &BundleManifest, manifest_bytes: &[u8]) -> String {
    let mut sums = String::new();
    for file in &manifest.files {
        sums.push_str(&format!("{}  {}\n", file.sha256, file.path));
    }
    sums.push_str(&format!("{}  manifest.json\n", digest(manifest_bytes)));
    sums
}

fn collect_files(root: &Path, current: &Path, files: &mut BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "bundle contains a symlink"
        );
        let canonical = fs::canonicalize(entry.path())?;
        ensure!(canonical.starts_with(root), "bundle file escapes its root");
        if metadata.is_dir() {
            collect_files(root, &canonical, files)?;
        } else {
            ensure!(metadata.is_file(), "bundle contains a non-file entry");
            files.insert(
                canonical
                    .strip_prefix(root)?
                    .to_str()
                    .context("non-UTF8 bundle path")?
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

/// Validate every payload, the checksum index, relative paths, and absence of
/// extra files or symlink escapes. This never starts a runtime or target.
pub fn verify_bundle(output: &Path) -> Result<BundleVerification> {
    let root = fs::canonicalize(output)?;
    ensure!(root.is_dir(), "bundle is not a directory");
    let manifest_bytes = fs::read(root.join("manifest.json"))?;
    let manifest: BundleManifest =
        serde_json::from_slice(&manifest_bytes).context("decode bundle manifest")?;
    ensure!(manifest.schema_version == 1, "unsupported bundle schema");
    let mut expected = BTreeSet::from(["manifest.json".into(), "SHA256SUMS".into()]);
    let mut bytes = 0;
    for entry in &manifest.files {
        safe_relative(&entry.path)?;
        ensure!(
            entry.sha256.len() == 64 && entry.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid manifest digest"
        );
        ensure!(
            expected.insert(entry.path.clone()),
            "duplicate or reserved manifest file"
        );
        let path = fs::canonicalize(root.join(&entry.path))?;
        ensure!(path.starts_with(&root), "bundle path escapes its root");
        let (size, hash) = hash_file(&path)?;
        ensure!(
            size == entry.size && hash == entry.sha256,
            "bundle digest or size mismatch: {}",
            entry.path
        );
        bytes += size;
    }
    safe_relative(&manifest.target_path)?;
    let target = manifest
        .files
        .iter()
        .find(|entry| entry.path == manifest.target_path)
        .context("manifest target is not indexed")?;
    ensure!(
        target.sha256 == manifest.target_sha256,
        "target digest disagrees with manifest"
    );
    ensure!(
        fs::read_to_string(root.join("SHA256SUMS"))? == checksum_text(&manifest, &manifest_bytes),
        "SHA256SUMS disagrees with manifest"
    );
    let mut actual = BTreeSet::new();
    collect_files(&root, &root, &mut actual)?;
    ensure!(
        actual == expected,
        "bundle contains missing or unindexed files"
    );
    Ok(BundleVerification {
        files: expected.len(),
        bytes,
        target_sha256: manifest.target_sha256,
    })
}
