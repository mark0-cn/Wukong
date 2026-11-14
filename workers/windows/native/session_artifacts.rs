//! Immutable views of an append-only MCP transcript. Each small protected
//! receipt binds a prefix; later calls append without copying earlier evidence.
use crate::{
    deployment,
    security::{self, AclPolicy},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

pub(crate) const LIMIT: usize = 8 * 1024 * 1024;
/// Shared bounded evidence writer for external debugger and interactive I/O.
pub(crate) struct Transcript {
    file: File,
    retained: usize,
}
impl Transcript {
    pub(crate) fn new(file: File) -> Self {
        Self { file, retained: 0 }
    }
    pub(crate) fn append(&mut self, value: &serde_json::Value) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        ensure!(
            self.retained + bytes.len() < LIMIT,
            "native session transcript exceeded its resource budget"
        );
        self.file.write_all(&bytes)?;
        self.file.write_all(b"\n")?;
        self.file.sync_all()?;
        self.retained += bytes.len() + 1;
        Ok(())
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Prefix {
    schema: u32,
    resource: String,
    sequence: u64,
    bytes: u64,
    sha256: String,
    request_sha256: String,
}
fn path(root: &Path, id: &str, suffix: &str) -> Result<PathBuf> {
    ensure!(
        id.len() == 32
            && id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid session artifact resource"
    );
    Ok(root
        .join("metadata/resource-results")
        .join(format!("{id}.{suffix}")))
}
fn receipt(root: &Path, id: &str, sequence: u64) -> Result<PathBuf> {
    ensure!(
        (1..=4096).contains(&sequence),
        "session call count exceeds its bound"
    );
    path(root, id, &format!("transcript-{sequence}.json"))
}
pub(crate) fn create(root: &Path, id: &str) -> Result<File> {
    create_with_policy(root, id, &AclPolicy::private_vm())
}
fn create_with_policy(root: &Path, id: &str, policy: &AclPolicy) -> Result<File> {
    let path = path(root, id, "session.jsonl")?;
    drop(policy.create_file(&path)?);
    // Keep the writer exclusive and deny replacement, while permitting the
    // service to read previously published prefixes through separate handles.
    let file = OpenOptions::new()
        .write(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path)?;
    policy.verify(&path, false)?;
    security::no_links(&path)?;
    Ok(file)
}
pub(crate) fn snapshot(root: &Path, id: &str, sequence: u64, request_hash: &str) -> Result<String> {
    snapshot_with_policy(root, id, sequence, request_hash, &AclPolicy::private_vm())
}
fn snapshot_with_policy(
    root: &Path,
    id: &str,
    sequence: u64,
    request_hash: &str,
    policy: &AclPolicy,
) -> Result<String> {
    let destination = receipt(root, id, sequence)?;
    let source = path(root, id, "session.jsonl")?;
    policy.verify(&source, false)?;
    let data = deployment::read_bytes(&source, LIMIT as u64)?;
    let prefix = Prefix {
        schema: 1,
        resource: id.into(),
        sequence,
        bytes: data.len() as u64,
        sha256: hex::encode(Sha256::digest(&data)),
        request_sha256: request_hash.into(),
    };
    deployment::publish_json(&destination, &prefix, policy)?;
    Ok(format!("native-results/{id}/transcript-{sequence}"))
}
pub(crate) fn read(root: &Path, id: &str, sequence: u64, request_hash: &str) -> Result<Vec<u8>> {
    read_with_policy(root, id, sequence, request_hash, &AclPolicy::private_vm())
}
fn read_with_policy(
    root: &Path,
    id: &str,
    sequence: u64,
    request_hash: &str,
    policy: &AclPolicy,
) -> Result<Vec<u8>> {
    let receipt = receipt(root, id, sequence)?;
    policy.verify(&receipt, false)?;
    let prefix: Prefix = serde_json::from_slice(&deployment::read_bytes(&receipt, 2048)?)?;
    ensure!(
        prefix.schema == 1
            && prefix.resource == id
            && prefix.sequence == sequence
            && prefix.bytes <= LIMIT as u64
            && prefix.request_sha256 == request_hash,
        "session transcript prefix belongs to another request or exceeds its bound"
    );
    let source = path(root, id, "session.jsonl")?;
    policy.verify(&source, false)?;
    security::no_links(&source)?;
    let mut bytes = Vec::new();
    File::open(source)?
        .take(prefix.bytes)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == prefix.bytes && hex::encode(Sha256::digest(&bytes)) == prefix.sha256,
        "session transcript prefix changed or was truncated"
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};
    #[test]
    fn published_prefixes_survive_append_and_reject_replacement_or_changed_bytes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let policy = AclPolicy::current_user()?;
        policy.create_directory(&root.join("metadata"))?;
        policy.create_directory(&root.join("metadata/resource-results"))?;
        let id = "a".repeat(32);
        let hash = "b".repeat(64);
        let mut writer = create_with_policy(root, &id, &policy)?;
        writer.write_all(b"first\n")?;
        writer.sync_all()?;
        snapshot_with_policy(root, &id, 1, &hash, &policy)?;
        writer.write_all(b"second\n")?;
        writer.sync_all()?;
        snapshot_with_policy(root, &id, 2, &hash, &policy)?;
        assert_eq!(read_with_policy(root, &id, 1, &hash, &policy)?, b"first\n");
        assert_eq!(
            read_with_policy(root, &id, 2, &hash, &policy)?,
            b"first\nsecond\n"
        );
        assert!(read_with_policy(root, &id, 1, &"c".repeat(64), &policy).is_err());
        assert!(snapshot_with_policy(root, &id, 1, &hash, &policy).is_err());
        assert!(
            OpenOptions::new()
                .write(true)
                .open(path(root, &id, "session.jsonl")?)
                .is_err()
        );
        assert!(std::fs::remove_file(path(root, &id, "session.jsonl")?).is_err());
        writer.seek(SeekFrom::Start(0))?;
        writer.write_all(b"other\n")?;
        writer.sync_all()?;
        assert!(read_with_policy(root, &id, 1, &hash, &policy).is_err());
        assert!(receipt(root, &id, 0).is_err());
        assert!(receipt(root, &id, 4097).is_err());
        assert!(path(root, "../escape", "session.jsonl").is_err());
        Ok(())
    }
}
