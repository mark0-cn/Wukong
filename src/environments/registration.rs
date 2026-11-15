//! Operator enrollment of public Windows connection receipts. Private key
//! contents are never imported; saving prepares only a dedicated public host pin.
use super::{EnvironmentConfig, validate_id};
use crate::runtime::ssh::{ManagedSsh, validate_host, validate_no_links};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use wukong_worker_package::SshPublicKey;
#[cfg(test)]
mod tests;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: u32,
    installed: bool,
    installation_id: String,
    environment_id: String,
    generation: u64,
    manifest_sha256: String,
    ssh_user: String,
    ssh_port: u16,
    host_key: SshPublicKey,
    host_fingerprint: String,
    #[serde(rename = "production_isolation_verified")]
    _production_isolation_verified: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsConnection {
    pub connection_json: String,
    pub name: String,
    pub host: String,
    pub identity_file: PathBuf,
    pub confirmed_fingerprint: String,
    pub make_default: bool,
}

fn pin_path(home: &Path, installation: &str, key: &SshPublicKey) -> Result<PathBuf> {
    let record = format!("wukong-worker-{installation} {}\n", key.authorized_key()?);
    Ok(home.join(format!(
        "ssh-host-{}.known_hosts",
        hex::encode(Sha256::digest(record.as_bytes()))
    )))
}

pub fn prepare(
    home: &Path,
    input: &WindowsConnection,
    existing: &BTreeMap<String, EnvironmentConfig>,
) -> Result<(String, Value)> {
    ensure!(
        input.connection_json.len() <= 16384,
        "public connection receipt exceeds 16 KiB"
    );
    let receipt: Receipt = serde_json::from_str(&input.connection_json)
        .context("invalid public Windows connection receipt")?;
    ensure!(
        receipt.schema == 1 && receipt.installed && receipt.generation > 0,
        "connection receipt does not describe an installed worker"
    );
    validate_id(&receipt.environment_id)?;
    ensure!(
        !existing.contains_key(&receipt.environment_id),
        "environment ID already exists; edit its settings instead of replacing it"
    );
    ensure!(
        receipt.manifest_sha256.len() == 64
            && receipt
                .manifest_sha256
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "invalid connection payload identity"
    );
    let fingerprint = receipt.host_key.fingerprint()?;
    ensure!(
        receipt.host_fingerprint == fingerprint
            && input.confirmed_fingerprint.trim() == fingerprint,
        "independently confirmed SSH fingerprint does not match the public host key"
    );
    let host = input.host.trim();
    validate_host(host)?;
    validate_no_links(&input.identity_file)?;
    ensure!(
        fs::metadata(&input.identity_file)?.is_file(),
        "select an existing local SSH private key file"
    );
    let ssh = ManagedSsh {
        installation_id: receipt.installation_id.clone(),
        user: receipt.ssh_user,
        port: receipt.ssh_port,
        identity_file: input.identity_file.clone(),
        known_hosts_file: pin_path(home, &receipt.installation_id, &receipt.host_key)?,
        host_key: receipt.host_key,
    };
    ssh.validate()?;
    let value = json!({"name":input.name.trim(),"platform":"windows","installation_id":receipt.installation_id,
        "runtime":{"backend":"managed_ssh","ssh_host":host,"managed_ssh":ssh}});
    let environment: EnvironmentConfig = serde_json::from_value(value.clone())?;
    environment.validate()?;
    Ok((receipt.environment_id, value))
}

/// Called under the controller maintenance lock, after configuration validation.
/// Only generated public-pin names are writable; arbitrary user SSH files stay
/// external. Existing mismatched contents are never replaced.
pub(crate) fn prepare_pin_files(
    home: &Path,
    environments: &BTreeMap<String, EnvironmentConfig>,
) -> Result<Vec<PathBuf>> {
    let mut pins = Vec::new();
    for environment in environments.values() {
        let Some(ssh) = &environment.runtime.managed_ssh() else {
            continue;
        };
        let path = pin_path(home, &ssh.installation_id, &ssh.host_key)?;
        if ssh.known_hosts_file != path {
            continue;
        }
        ssh.validate()?;
        validate_no_links(&path)?;
        let expected = ssh.known_hosts_record()?;
        match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(expected.as_bytes())?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&path)?;
                ensure!(metadata.is_file(), "public SSH pin is not a regular file");
                let mut bytes = Vec::new();
                fs::File::open(&path)?.take(4097).read_to_end(&mut bytes)?;
                ensure!(
                    bytes == expected.as_bytes(),
                    "existing public SSH pin differs; it was not replaced"
                );
            }
            Err(error) => return Err(error.into()),
        }
        pins.push(path);
    }
    pins.sort();
    pins.dedup();
    Ok(pins)
}
