//! Copy a verified offline tree into a newly protected directory. Callers own
//! the parent/transaction; this module neither activates code nor registers OS resources.
use crate::security::{AclPolicy, no_links};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ, GetFileInformationByHandle,
};
use wukong_worker_package::{
    Compatibility, FileEntry, PublicKey, SignatureRecord, VerifiedManifest, verify_package,
};

#[cfg(test)]
#[path = "staging_tests.rs"]
pub(crate) mod tests;

/// Hold this exact regular file without admitting writers or delete/rename
/// handles. The source parent still needs the caller's media protections.
fn source_file(path: &Path) -> Result<fs::File> {
    no_links(path)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    crate::security::check(unsafe {
        GetFileInformationByHandle(file.as_raw_handle(), &mut information)
    })?;
    ensure!(
        file.metadata()?.is_file()
            && information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
            && information.nNumberOfLinks == 1,
        "offline source must be a regular, single-link file"
    );
    Ok(file)
}

fn digest(input: &mut fs::File, expected: &FileEntry) -> Result<()> {
    ensure!(
        input.metadata()?.len() == expected.size,
        "offline source size changed"
    );
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let size = input.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        count += size as u64;
        ensure!(count <= expected.size, "offline source grew while staging");
        hash.update(&buffer[..size]);
    }
    ensure!(
        count == expected.size && hex::encode(hash.finalize()) == expected.sha256,
        "offline source changed while staging"
    );
    input.seek(SeekFrom::Start(0))?;
    Ok(())
}

fn copy_member(source: &Path, target: &Path, entry: &FileEntry, policy: &AclPolicy) -> Result<()> {
    let mut input = source_file(source)?;
    // Verify before creating a readable code file. Never publish unknown bytes
    // and only then discover that the source changed after initial admission.
    digest(&mut input, entry)?;
    let mut output = policy.create_file(target)?;
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let size = input.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        count += size as u64;
        ensure!(count <= entry.size, "offline source grew while staging");
        hash.update(&buffer[..size]);
        output.write_all(&buffer[..size])?;
    }
    ensure!(
        count == entry.size && hex::encode(hash.finalize()) == entry.sha256,
        "offline source changed while staging"
    );
    output.sync_all()?;
    drop(output);
    policy.verify(target, false)
}

pub fn stage_payload(
    source: &Path,
    destination: &Path,
    key: &PublicKey,
    expected: &Compatibility<'_>,
    policy: &AclPolicy,
) -> Result<VerifiedManifest> {
    no_links(source)?;
    let verified = verify_package(source, key, expected)?;
    let canonical = verified.manifest().canonical_bytes()?;
    let mut signature_bytes = Vec::new();
    source_file(&source.join("manifest.sig"))?
        .take(1025)
        .read_to_end(&mut signature_bytes)?;
    ensure!(
        signature_bytes.len() <= 1024,
        "offline signature exceeds stage limit"
    );
    let signature: SignatureRecord = serde_json::from_slice(&signature_bytes)?;
    VerifiedManifest::verify(&canonical, &signature, key)?;
    policy.create_directory(destination)?;
    for (name, entry) in &verified.manifest().files {
        let target = destination.join(name);
        let parent = target.parent().context("stage member has no parent")?;
        let mut directories = Vec::new();
        let mut current = parent;
        while current != destination && !current.exists() {
            directories.push(current);
            current = current.parent().context("invalid stage directory")?;
        }
        policy.verify(current, true)?;
        for directory in directories.into_iter().rev() {
            policy.create_directory(directory)?;
        }
        copy_member(&source.join(name), &target, entry, policy)?;
    }
    for name in ["manifest.json", "manifest.sig", "manifest.pub"] {
        let path = source.join(name);
        if name == "manifest.pub" && !path.exists() {
            continue;
        }
        // Write verified in-memory identities, never reopen mutable media as
        // the authority for manifest/public-key bytes after admission.
        let bytes = match name {
            "manifest.json" => canonical.clone(),
            "manifest.sig" => signature_bytes.clone(),
            _ => format!("{}\n", key.to_hex()).into_bytes(),
        };
        let mut output = policy.create_file(&destination.join(name))?;
        output.write_all(&bytes)?;
        output.sync_all()?;
    }
    verified.verify_tree(destination)?;
    Ok(verified)
}
