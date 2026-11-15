//! Bounded protected file IO; no installer or deployment dependency.
use crate::isolation::security::{self, AclPolicy};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};
pub fn read_bytes(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    security::no_links(path)?;
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(maximum + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= maximum,
        "native state exceeds size limit"
    );
    Ok(bytes)
}
pub fn write_new_json(path: &Path, value: &impl Serialize, policy: &AclPolicy) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    let mut output = policy.create_file(path)?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    drop(output);
    policy.verify(path, false)
}
pub(crate) fn publish_json(path: &Path, value: &impl Serialize, policy: &AclPolicy) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    policy.publish(path, &bytes)
}
