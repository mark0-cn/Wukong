//! Discover a directory as a bundle without requiring the user to assign roles.
use crate::{adapters::binary, adapters::files::content::sha256_file, domain::bundle::*};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub const MAX_MEMBERS: usize = 256;
pub const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_DEPTH: usize = 16;

pub fn load(root: &Path) -> Result<BundleManifest> {
    let root = fs::canonicalize(root).context("bundle directory must exist")?;
    ensure!(root.is_dir(), "bundle target is not a directory");
    let mut paths = Vec::new();
    collect(&root, 0, &mut paths)?;
    ensure!(!paths.is_empty(), "bundle directory has no regular files");
    paths.sort();
    let mut total = 0u64;
    let mut members = Vec::new();
    for path in paths.iter().take(MAX_MEMBERS) {
        let relative = path
            .strip_prefix(&root)?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::metadata(path)?.len();
        total = total.saturating_add(bytes);
        ensure!(total <= MAX_TOTAL_BYTES, "bundle exceeds the size bound");
        let sha256 = sha256_file(path)?;
        let mut format = "unknown".to_string();
        let mut kind = "data".to_string();
        let mut entry = None;
        let mut imports = Vec::new();
        let mut libraries = Vec::new();
        if looks_like_binary(path)
            && let Ok(profile) = binary::inspect(path)
        {
            format = profile.format.clone();
            kind = match profile.kind.to_ascii_lowercase() {
                value if value.contains("executable") => "executable".into(),
                value if value.contains("dynamic") => "library".into(),
                _ => "data".into(),
            };
            entry = Some(profile.entry.value);
            libraries = profile.libraries.clone();
            let mut seen = std::collections::BTreeSet::new();
            for import in &profile.imports {
                if seen.insert(import.library.to_ascii_lowercase()) {
                    imports.push(import.library.clone());
                }
            }
        }
        members.push(BundleMember {
            path: relative,
            sha256,
            size: bytes,
            format,
            kind,
            entry,
            imports,
            libraries,
        });
    }
    let entry = select_entry(&members).context("bundle has no entry candidate")?;
    let mut identities = members
        .iter()
        .map(|member| format!("{}\0{}", member.path, member.sha256))
        .collect::<Vec<_>>();
    identities.sort();
    let bundle_sha256 = hex::encode(Sha256::digest(identities.join("\n").as_bytes()));
    let mut edges = Vec::new();
    let by_name: BTreeMap<String, String> = members
        .iter()
        .map(|member| {
            (
                member
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&member.path)
                    .to_ascii_lowercase(),
                member.path.clone(),
            )
        })
        .collect();
    for member in &members {
        for library in member.libraries.iter().chain(member.imports.iter()) {
            let name = library
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or(library)
                .to_ascii_lowercase();
            if let Some(target) = by_name.get(&name)
                && target != &member.path
                && !edges
                    .iter()
                    .any(|edge: &BundleEdge| edge.from == member.path && edge.to == *target)
            {
                edges.push(BundleEdge {
                    from: member.path.clone(),
                    to: target.clone(),
                    library: library.clone(),
                });
            }
        }
    }
    let entry_sha256 = members
        .iter()
        .find(|member| member.path == entry)
        .map(|member| member.sha256.clone())
        .context("entry member is missing")?;
    Ok(BundleManifest {
        schema: BUNDLE_SCHEMA,
        root,
        bundle_sha256,
        entry,
        entry_sha256,
        members,
        edges,
    })
}

fn collect(directory: &Path, depth: usize, out: &mut Vec<PathBuf>) -> Result<()> {
    ensure!(depth <= MAX_DEPTH, "bundle directory nesting is too deep");
    ensure!(out.len() <= MAX_MEMBERS, "bundle has too many files");
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect(&path, depth + 1, out)?;
        } else if file_type.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn looks_like_binary(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "exe" | "dll" | "sys" | "ocx" | "cpl" | "scr" | "drv"
    ) || fs::read(path)
        .map(|bytes| bytes.starts_with(b"MZ"))
        .unwrap_or(false)
}

fn select_entry(members: &[BundleMember]) -> Option<String> {
    members
        .iter()
        .find(|member| {
            member.kind == "executable" && member.path.to_ascii_lowercase().ends_with(".exe")
        })
        .or_else(|| members.iter().find(|member| member.kind == "executable"))
        .or_else(|| members.first())
        .map(|member| member.path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_bundle_hashes_members_and_infers_an_entry() -> Result<()> {
        let directory = tempfile::tempdir()?;
        fs::create_dir_all(directory.path().join("bin"))?;
        fs::write(directory.path().join("bin/app.bin"), b"not-a-pe")?;
        fs::write(directory.path().join("data.txt"), b"sidecar")?;
        let manifest = load(directory.path())?;
        assert_eq!(manifest.schema, BUNDLE_SCHEMA);
        assert_eq!(manifest.members.len(), 2);
        assert_eq!(manifest.entry, "bin/app.bin");
        assert_eq!(manifest.entry_sha256, manifest.members[0].sha256);
        assert_eq!(manifest.bundle_sha256.len(), 64);
        let second = load(directory.path())?;
        assert_eq!(manifest.bundle_sha256, second.bundle_sha256);
        Ok(())
    }
}
