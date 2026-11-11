//! Copy a verified offline tree into a newly protected directory. Callers own
//! the parent/transaction; this module neither activates code nor registers OS resources.
use crate::security::{AclPolicy, no_links};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};
use wukong_worker_package::{Compatibility, PublicKey, VerifiedManifest, verify_package};

#[cfg(test)]
#[path = "staging_tests.rs"]
pub(crate) mod tests;

pub fn stage_payload(
    source: &Path,
    destination: &Path,
    key: &PublicKey,
    expected: &Compatibility<'_>,
    policy: &AclPolicy,
) -> Result<VerifiedManifest> {
    no_links(source)?;
    let verified = verify_package(source, key, expected)?;
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
        no_links(&source.join(name))?;
        let mut input = fs::File::open(source.join(name))?;
        let mut output = policy.create_file(&target)?;
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
        policy.verify(&target, false)?;
    }
    for name in ["manifest.json", "manifest.sig", "manifest.pub"] {
        let path = source.join(name);
        if name == "manifest.pub" && !path.exists() {
            continue;
        }
        no_links(&path)?;
        let maximum = if name == "manifest.json" {
            4 * 1024 * 1024
        } else {
            1024
        };
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(maximum + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= maximum,
            "offline metadata exceeds stage limit"
        );
        let mut output = policy.create_file(&destination.join(name))?;
        output.write_all(&bytes)?;
        output.sync_all()?;
    }
    verified.verify_tree(destination)?;
    Ok(verified)
}
