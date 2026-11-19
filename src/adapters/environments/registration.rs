//! Operator enrollment of public Windows connection receipts. Private key
//! contents are never imported; saving prepares only a dedicated public host pin.
use super::{EnvironmentConfig, Platform};
use crate::adapters::worker::{
    ManagedSshConfig, RuntimeConfig, TransportConfig,
    ssh::{ManagedSsh, validate_host, validate_no_links},
};
use anyhow::{Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use wukong_worker_package::{ConnectionReceipt, SshPublicKey};
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsConnection {
    pub connection_json: String,
    pub name: String,
    pub host: String,
    pub identity_file: PathBuf,
    pub confirmed_fingerprint: String,
    pub make_default: bool,
}

pub(crate) fn pin_path(home: &Path, installation: &str, key: &SshPublicKey) -> Result<PathBuf> {
    ensure!(
        installation.len() == 32
            && installation
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "invalid installation identity for the public host pin"
    );
    let record = format!("wukong-worker-{installation} {}\n", key.authorized_key()?);
    Ok(home.join(format!(
        "ssh-host-{}.known_hosts",
        hex::encode(Sha256::digest(record.as_bytes()))
    )))
}

pub(crate) struct ConnectionTarget<'a> {
    pub name: &'a str,
    pub host: &'a str,
    pub identity_file: &'a Path,
    pub confirmed_fingerprint: &'a str,
}

pub fn prepare(
    home: &Path,
    input: &WindowsConnection,
    existing: &BTreeMap<String, EnvironmentConfig>,
) -> Result<(String, EnvironmentConfig)> {
    let receipt = ConnectionReceipt::parse(input.connection_json.as_bytes())?;
    let environment = prepare_config(
        home,
        &receipt,
        ConnectionTarget {
            name: &input.name,
            host: &input.host,
            identity_file: &input.identity_file,
            confirmed_fingerprint: &input.confirmed_fingerprint,
        },
        existing,
    )?;
    Ok((receipt.environment_id, environment))
}

pub(crate) fn prepare_config(
    home: &Path,
    receipt: &ConnectionReceipt,
    target: ConnectionTarget<'_>,
    existing: &BTreeMap<String, EnvironmentConfig>,
) -> Result<EnvironmentConfig> {
    receipt.confirm(target.confirmed_fingerprint)?;
    ensure!(
        !existing.contains_key(&receipt.environment_id),
        "environment ID already exists; edit its settings instead of replacing it"
    );
    let host = target.host.trim();
    validate_host(host)?;
    validate_no_links(target.identity_file)?;
    ensure!(
        fs::metadata(target.identity_file)?.is_file(),
        "select an existing local SSH private key file"
    );
    let ssh = ManagedSsh {
        installation_id: receipt.installation_id.clone(),
        user: receipt.ssh_user.clone(),
        port: receipt.ssh_port,
        identity_file: target.identity_file.into(),
        known_hosts_file: pin_path(home, &receipt.installation_id, &receipt.host_key)?,
        host_key: receipt.host_key.clone(),
    };
    let environment = EnvironmentConfig {
        name: target.name.trim().into(),
        platform: Platform::Windows,
        installation_id: Some(receipt.installation_id.clone()),
        runtime: RuntimeConfig {
            transport: TransportConfig::ManagedSsh(ManagedSshConfig {
                ssh_host: host.into(),
                ssh_executable: "ssh".into(),
                managed_ssh: ssh,
            }),
            ..Default::default()
        },
    };
    environment.validate()?;
    Ok(environment)
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
