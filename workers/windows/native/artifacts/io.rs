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

/// Replace a diagnostic snapshot in an already protected directory. Existing
/// create-new publication remains unchanged for execution authority records.
pub(crate) fn replace_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let policy = AclPolicy::private_vm();
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("diagnostic parent missing"))?;
    policy.verify(parent, true)?;
    if path.exists() {
        policy.verify(path, false)?;
    }
    let temporary = parent.join(format!(
        "diagnostic-{}.tmp",
        hex::encode(crate::host::accounts::random_bytes::<16>()?)
    ));
    let result = (|| -> Result<()> {
        let mut output = policy.create_file(&temporary)?;
        output.write_all(bytes)?;
        output.sync_all()?;
        drop(output);
        let from = security::wide(temporary.as_os_str())?;
        let to = security::wide(path.as_os_str())?;
        security::check(unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        })?;
        policy.verify(path, false)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(temporary);
    }
    result
}
