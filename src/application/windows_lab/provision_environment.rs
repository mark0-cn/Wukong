//! `lab provision --register`: write the verified managed-SSH environment into
//! the operator configuration and publish its public host pin.
//!
//! The private identity key stays under the controller state directory; only the
//! public host pin is derived here from the independently verified receipt.
use crate::adapters::environments::EnvironmentConfig;
use crate::adapters::environments::registration::{self, WindowsConnection};
use crate::application::windows_lab::MediaIdentity;
use crate::application::windows_lab::SshKey;
use crate::application::windows_lab::provision::{
    PublicConnection, known_hosts_path, validate_connection,
};
use crate::config::AppConfig;
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use toml_edit::Item;

#[derive(Debug)]
pub struct EnvRegistration {
    pub environment_id: String,
    pub environment: EnvironmentConfig,
}

#[derive(Debug)]
pub struct ConfigWrite {
    pub configuration: PathBuf,
    pub backup: PathBuf,
    pub replaced: bool,
    /// Environment entry the rewrite replaced, retained so the caller can
    /// retract its public host pin only after the new mapping is published.
    pub previous: Option<EnvironmentConfig>,
}

/// Build the managed-SSH environment from the verified public receipt. The
/// caller passes the guest IPv4 discovered after boot.
pub fn prepare_environment(
    config: &AppConfig,
    state_dir: &Path,
    media: &MediaIdentity,
    connection: &PublicConnection,
    guest_ip: &str,
    identity_file: &Path,
    force: bool,
) -> Result<EnvRegistration> {
    validate_connection(connection, media)?;
    ensure!(
        identity_file.is_absolute() && identity_file.is_file(),
        "controller-managed SSH identity key is missing: {}",
        identity_file.display()
    );
    crate::adapters::files::safety::safe(identity_file)?;
    let home = state_dir.join("environments");
    fs::create_dir_all(&home)
        .with_context(|| format!("create controller environment directory {}", home.display()))?;
    let mut existing = config.environments.clone();
    if force {
        existing.remove(&media.environment_id);
    }
    let input = WindowsConnection {
        connection_json: serde_json::to_string(connection)?,
        name: format!("Windows worker {}", media.environment_id),
        host: guest_ip.trim().to_owned(),
        identity_file: identity_file.to_path_buf(),
        confirmed_fingerprint: connection.host_fingerprint.clone(),
        make_default: false,
    };
    let (environment_id, value) = registration::prepare(&home, &input, &existing).with_context(|| {
        format!(
            "environment {} cannot be registered; pass --force to replace an existing environment",
            media.environment_id
        )
    })?;
    ensure!(
        environment_id == media.environment_id,
        "registered environment differs from the provisioned media identity"
    );
    let environment: EnvironmentConfig = serde_json::from_value(value)?;
    environment.validate()?;
    // The registration adapter must pin exactly the installer-generated worker
    // host key from the verified receipt, into the generated controller pin
    // file, while the identity file stays the controller-managed client key.
    // Check the outcome here so a future registration change cannot silently
    // pin the enrollment client identity instead.
    let ssh = environment
        .runtime
        .managed_ssh()
        .context("registered environment is not a managed SSH environment")?;
    ensure!(
        SshKey::from_public(&ssh.host_key)? == connection.host_key,
        "registered host pin is not the verified receipt worker host key"
    );
    ensure!(
        ssh.known_hosts_file
            == known_hosts_path(state_dir, &connection.installation_id, &connection.host_key)?,
        "registered known-hosts file is not derived from the verified receipt host key"
    );
    ensure!(
        ssh.identity_file.as_path() == identity_file,
        "registered identity file is not the controller SSH client key"
    );
    Ok(EnvRegistration {
        environment_id,
        environment,
    })
}

/// Publish the public host pin file before any connection is attempted.
pub fn pin_environment(state_dir: &Path, registration: &EnvRegistration) -> Result<Vec<PathBuf>> {
    let home = state_dir.join("environments");
    fs::create_dir_all(&home)?;
    registration::prepare_pin_files(
        &home,
        &BTreeMap::from([(
            registration.environment_id.clone(),
            registration.environment.clone(),
        )]),
    )
}

/// Write the environment table into the operator configuration, preserving
/// comments and unrelated settings. A timestamped backup is written first, so a
/// caller whose later publishing step fails can restore it; an existing
/// different environment is refused without `--force`. The replaced public host
/// pin is retracted only after the caller publishes the new mapping.
pub fn write_configuration(
    configuration: &Path,
    registration: &EnvRegistration,
    force: bool,
) -> Result<ConfigWrite> {
    ensure!(
        configuration.is_absolute() && configuration.is_file(),
        "--register requires an existing absolute configuration file"
    );
    let text = fs::read_to_string(configuration)
        .with_context(|| format!("read configuration {}", configuration.display()))?;
    let mut document: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("parse configuration {}", configuration.display()))?;
    let current: AppConfig =
        toml::from_str(&text).context("parse configuration for registration")?;
    let previous = current
        .environments
        .get(&registration.environment_id)
        .cloned();
    let mut replaced = false;
    if let Some(previous) = &previous {
        let unchanged =
            serde_json::to_value(previous)? == serde_json::to_value(&registration.environment)?;
        if !unchanged {
            ensure!(
                force,
                "environment {} is already registered with different settings; pass --force to replace it",
                registration.environment_id
            );
            replaced = true;
        }
    }
    let environments = document
        .entry("environments")
        .or_insert(Item::Table(toml_edit::Table::new()));
    let table = environments
        .as_table_like_mut()
        .context("configuration environments setting is not a table")?;
    table.insert(
        &registration.environment_id,
        toml_item(&registration.environment)?,
    );
    if current.default_environment.is_none() {
        document["default_environment"] = toml_edit::value(registration.environment_id.clone());
    }
    let updated = document.to_string();
    let parsed: AppConfig =
        toml::from_str(&updated).context("updated configuration is not valid TOML")?;
    parsed
        .validate()
        .context("updated configuration does not validate")?;
    let backup = backup_path(configuration)?;
    let bytes = fs::read(configuration)?;
    let mut staged = tempfile::NamedTempFile::new_in(
        configuration
            .parent()
            .context("configuration has no parent directory")?,
    )?;
    staged.write_all(&bytes)?;
    staged.as_file().sync_all()?;
    staged
        .persist(&backup)
        .map_err(|error| error.error)
        .context("write configuration backup")?;
    write_synced(configuration, updated.as_bytes())?;
    Ok(ConfigWrite {
        configuration: configuration.to_path_buf(),
        backup,
        replaced,
        previous,
    })
}

/// Restore the configuration a failed registration attempt replaced, using the
/// timestamped backup the rewrite wrote first. A failed `--register` must leave
/// the previous mapping in force: the lab configuration hash persisted before
/// this command still matches it, so later commands keep opening.
pub fn restore_configuration(written: &ConfigWrite) -> Result<()> {
    let previous = fs::read(&written.backup)
        .with_context(|| format!("read configuration backup {}", written.backup.display()))?;
    write_synced(&written.configuration, &previous)
}

/// Retract the public host pin of an environment this registration replaced.
/// The caller runs this only after the rewritten configuration and its new pin
/// are published; the pin is skipped when the replacement reuses the same pin
/// file for its own environment.
pub fn retract_replaced_pin(
    state_dir: &Path,
    written: &ConfigWrite,
    published_pins: &[PathBuf],
) -> Result<()> {
    let Some(previous) = written.previous.as_ref().filter(|_| written.replaced) else {
        return Ok(());
    };
    let Some(ssh) = previous.runtime.managed_ssh() else {
        return Ok(());
    };
    if published_pins.contains(&ssh.known_hosts_file) {
        return Ok(());
    }
    let home = state_dir.join("environments");
    if ssh.known_hosts_file.starts_with(&home) && ssh.known_hosts_file.is_file() {
        fs::remove_file(&ssh.known_hosts_file).with_context(|| {
            format!(
                "remove replaced public host pin {}",
                ssh.known_hosts_file.display()
            )
        })?;
    }
    Ok(())
}

/// Known-hosts pins live under the configuration's state directory. The caller
/// passes the controller state directory explicitly so a portable configuration
/// never writes outside its declared state.
/// A ready-to-paste environment block without secrets, using the same durable
/// public pin path as `prepare_environment`.
pub fn environment_snippet(registration: &EnvRegistration, guest_ip: &str) -> Result<String> {
    let environment = &registration.environment;
    let ssh = environment
        .runtime
        .managed_ssh()
        .context("registered environment is not a managed SSH environment")?;
    let block = format!(
        "[environments.{0}]\nname = {1}\nplatform = \"windows\"\ninstallation_id = {2}\n\n[environments.{0}.runtime]\nbackend = \"managed_ssh\"\nssh_host = {3}\nrequest_timeout_secs = {4}\n\n[environments.{0}.runtime.managed_ssh]\ninstallation_id = {2}\nuser = {5}\nport = {6}\nidentity_file = {7}\nknown_hosts_file = {8}\n\n[environments.{0}.runtime.managed_ssh.host_key]\nalgorithm = {9}\nkey = {10}\n",
        registration.environment_id,
        toml_string(&environment.name),
        toml_string(&ssh.installation_id),
        toml_string(guest_ip),
        environment.runtime.request_timeout_secs,
        toml_string(&ssh.user),
        ssh.port,
        toml_string(&ssh.identity_file.to_string_lossy()),
        toml_string(&ssh.known_hosts_file.to_string_lossy()),
        toml_string(&ssh.host_key.algorithm),
        toml_string(&ssh.host_key.key),
    );
    Ok(block)
}

/// Fill the placeholder environment block from a finalized media identity when
/// `--register` was not requested. Unknown values stay explicit placeholders.
pub fn placeholder_block(
    media: &MediaIdentity,
    connection: &PublicConnection,
    state_dir: &Path,
    identity_file: Option<&Path>,
) -> Result<String> {
    let known_hosts =
        known_hosts_path(state_dir, &connection.installation_id, &connection.host_key)?;
    Ok(format!(
        "[environments.{0}]\nname = {1}\nplatform = \"windows\"\ninstallation_id = {2}\n\n[environments.{0}.runtime]\nbackend = \"managed_ssh\"\nssh_host = \"<GUEST_IPV4>\"\nrequest_timeout_secs = 300\n\n[environments.{0}.runtime.managed_ssh]\ninstallation_id = {2}\nuser = {3}\nport = {4}\nidentity_file = {5}\nknown_hosts_file = {6}\n\n[environments.{0}.runtime.managed_ssh.host_key]\nalgorithm = {7}\nkey = {8}\n",
        media.environment_id,
        toml_string(&format!("Windows worker {}", media.environment_id)),
        toml_string(&media.installation_id),
        toml_string(&media.ssh_user),
        media.ssh_port,
        match identity_file {
            Some(path) => toml_string(&path.to_string_lossy()),
            None => "\"<ABSOLUTE_CONTROLLER_SSH_PRIVATE_KEY>\"".to_owned(),
        },
        toml_string(&known_hosts.to_string_lossy()),
        toml_string(&connection.host_key.algorithm),
        toml_string(&connection.host_key.key),
    ))
}

/// Allocate a never-existing timestamped backup beside the configuration.
fn backup_path(configuration: &Path) -> Result<PathBuf> {
    let name = configuration
        .file_name()
        .context("configuration has no file name")?
        .to_string_lossy();
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    for attempt in 0..100 {
        let suffix = if attempt == 0 {
            String::new()
        } else {
            format!("-{attempt}")
        };
        let candidate = configuration.with_file_name(format!("{name}.{timestamp}{suffix}.bak"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("cannot allocate a timestamped configuration backup name")
}

fn toml_item<T: Serialize>(value: &T) -> Result<Item> {
    let serialized = toml::to_string(&BTreeMap::from([("value", value)]))?;
    let document: toml_edit::DocumentMut = serialized.parse()?;
    Ok(document
        .get("value")
        .context("serialized TOML value is missing")?
        .clone())
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("configuration has no parent directory")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(bytes)?;
    staged.as_file().sync_all()?;
    staged
        .persist(path)
        .map_err(|error| error.error)
        .context("publish configuration")?;
    Ok(())
}

#[cfg(test)]
mod tests;
