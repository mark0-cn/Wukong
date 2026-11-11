use crate::{
    Compatibility, PublicKey, SignatureRecord, VerifiedManifest, manifest::MAX_MANIFEST_BYTES,
    validate_path,
};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

fn reject_link(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "package links are forbidden"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "package reparse points are forbidden"
        );
    }
    Ok(metadata)
}

fn bounded_read(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    ensure!(
        reject_link(path)?.is_file(),
        "package metadata must be a regular file"
    );
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= maximum,
        "package metadata exceeds its size limit"
    );
    Ok(bytes)
}

fn package_root(root: &Path) -> Result<PathBuf> {
    ensure!(
        reject_link(root)?.is_dir(),
        "package root must be a directory"
    );
    for parent in root.ancestors().filter(|path| !path.as_os_str().is_empty()) {
        reject_link(parent)?;
    }
    Ok(root.canonicalize()?)
}

/// Verify a complete unpacked package against existing independent trust.
/// No key delivered inside the package can replace `trusted`.
pub fn verify_package(
    root: &Path,
    trusted: &PublicKey,
    compatibility: &Compatibility<'_>,
) -> Result<VerifiedManifest> {
    package_root(root)?;
    let bytes = bounded_read(&root.join("manifest.json"), MAX_MANIFEST_BYTES)?;
    let signature: SignatureRecord =
        serde_json::from_slice(&bounded_read(&root.join("manifest.sig"), 1024)?)?;
    let verified = VerifiedManifest::verify(&bytes, &signature, trusted)?;
    verified.manifest().check_compatibility(compatibility)?;
    verified.verify_tree(root)?;
    Ok(verified)
}

impl VerifiedManifest {
    /// The caller must keep this root private from untrusted writers during
    /// verification and copying. These portable checks reject existing links;
    /// native extraction must also create the root with a protected ACL.
    pub fn verify_tree(&self, root: &Path) -> Result<()> {
        // Reject links in ancestors as well as in archive members. Resolve only
        // after this check; resolving first would hide a junction/symlink.
        let canonical_root = package_root(root)?;
        let manifest = bounded_read(&root.join("manifest.json"), MAX_MANIFEST_BYTES)?;
        let signature: SignatureRecord =
            serde_json::from_slice(&bounded_read(&root.join("manifest.sig"), 1024)?)?;
        let verified = Self::verify(&manifest, &signature, self.trusted_key())?;
        ensure!(
            verified.sha256() == self.sha256(),
            "package manifest changed after signature verification"
        );
        let mut pending = vec![(canonical_root, String::new())];
        let mut observed = BTreeSet::new();
        let mut directories = BTreeSet::new();
        for path in self.manifest().files.keys() {
            let mut current = path.as_str();
            while let Some((parent, _)) = current.rsplit_once('/') {
                directories.insert(parent.to_owned());
                current = parent;
            }
        }
        while let Some((directory, relative)) = pending.pop() {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let filename = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("invalid package member encoding"))?;
                let name = if relative.is_empty() {
                    filename
                } else {
                    format!("{relative}/{filename}")
                };
                validate_path(&name)?;
                let path: PathBuf = entry.path();
                let metadata = reject_link(&path)?;
                if metadata.is_dir() {
                    ensure!(
                        directories.contains(&name),
                        "unexpected package directory: {name}"
                    );
                    pending.push((path, name));
                    continue;
                }
                ensure!(metadata.is_file(), "package members must be regular files");
                if relative.is_empty()
                    && ["manifest.json", "manifest.sig", "manifest.pub"].contains(&name.as_str())
                {
                    if name == "manifest.pub" {
                        let public = bounded_read(&path, 128)?;
                        let public =
                            crate::PublicKey::from_hex(std::str::from_utf8(&public)?.trim())?;
                        ensure!(
                            public.fingerprint() == self.signer_fingerprint(),
                            "delivered public key differs from pinned trust"
                        );
                    }
                    continue;
                }
                let expected = self
                    .manifest()
                    .files
                    .get(&name)
                    .with_context(|| format!("unexpected package member: {name}"))?;
                ensure!(
                    metadata.len() == expected.size,
                    "package member size mismatch: {name}"
                );
                let mut file = fs::File::open(&path)?;
                let mut digest = Sha256::new();
                let mut size = 0u64;
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let count = file.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    size += count as u64;
                    ensure!(
                        size <= expected.size,
                        "package member grew during verification"
                    );
                    digest.update(&buffer[..count]);
                }
                ensure!(
                    size == expected.size && hex::encode(digest.finalize()) == expected.sha256,
                    "package member hash mismatch: {name}"
                );
                observed.insert(name);
            }
        }
        ensure!(
            observed.len() == self.manifest().files.len(),
            "offline package is missing required files"
        );
        Ok(())
    }
}
