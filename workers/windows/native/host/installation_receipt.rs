//! Read and refresh public connection receipts from the protected installation.
use crate::{isolation::environment, rpc::pipe_transport};
use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};
use wukong_worker_package::{ConnectionReceipt, MAX_RECEIPT_BYTES};

/// Protected installation base directory; every installation owns one child directory.
pub(super) fn install_base() -> Result<PathBuf> {
    Ok(environment::program_data()?.join("WukongWorkers"))
}
/// Public receipt path of one installation. The installation ID is revalidated so
/// no caller can turn this into an arbitrary path.
pub(super) fn installation_receipt_path(base: &Path, installation_id: &str) -> Result<PathBuf> {
    pipe_transport::service_name(installation_id)?;
    Ok(base.join(installation_id).join("connection.json"))
}
/// List every installation below `base` that has a complete receipt. A directory
/// without one is a failed or foreign entry, not an existing installation.
pub(super) fn installed_receipts(base: &Path) -> Result<Vec<ConnectionReceipt>> {
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut receipts = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if pipe_transport::service_name(&name).is_err() || !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("connection.json");
        // A receipt that cannot be read or validated is not treated as installed, so
        // the media path may still repair an interrupted installation below `base`.
        let Ok(bytes) = crate::artifacts::io::read_bytes(&path, MAX_RECEIPT_BYTES as u64) else {
            continue;
        };
        let Ok(receipt) = installed_receipt(&bytes, &name) else {
            continue;
        };
        receipts.push(receipt);
    }
    receipts.sort_by(|left, right| left.installation_id.cmp(&right.installation_id));
    Ok(receipts)
}
fn installed_receipt(bytes: &[u8], installation_id: &str) -> Result<ConnectionReceipt> {
    let receipt = ConnectionReceipt::parse(bytes)?;
    ensure!(
        receipt.installation_id == installation_id,
        "installed receipt belongs to another installation"
    );
    Ok(receipt)
}

pub(super) fn refreshed_receipt(
    receipt: &ConnectionReceipt,
    candidates: Vec<String>,
) -> Result<ConnectionReceipt> {
    let mut refreshed = receipt.clone();
    refreshed.candidate_addresses = candidates;
    refreshed.validate()?;
    Ok(refreshed)
}

#[cfg(test)]
#[path = "installer_tests.rs"]
mod tests;
