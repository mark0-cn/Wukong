//! Durable controller-side state for one generated Windows worker installation.
//!
//! The generation record written by [`crate::application::worker_media`] is
//! immutable. The desktop workflow therefore keeps its mutable part — the chosen
//! environment name, the imported public receipt and the verified address — in a
//! separate sidecar JSON file. Sidecars never contain private key material.
use super::receipt::ConnectionReceipt;
use crate::adapters::files::json::{read_json, write_json};
use crate::application::worker_media::GenerationRecord;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use wukong_worker_package::SshPublicKey;

/// Schema of one installation state sidecar.
pub const STATE_SCHEMA: u32 = 1;
/// Directory below the worker-media root that holds the sidecars.
pub const STATE_DIR: &str = "state";

/// Lifecycle of one generated installation, from the GUI's point of view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallationStatus {
    /// Media was generated but no receipt has been imported yet.
    #[default]
    Generated,
    /// A receipt was imported but the managed SSH connection is not verified yet.
    InstalledPending,
    /// A registered environment for this installation passed the doctor check.
    Available,
}

/// Mutable state of one installation id.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallationState {
    pub schema: u32,
    pub installation_id: String,
    pub environment_id: String,
    pub environment_name: String,
    pub status: InstallationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ConnectionReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_host_key: Option<SshPublicKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_address: Option<String>,
    pub updated_at: String,
}

impl InstallationState {
    fn validate(&self, installation_id: &str) -> Result<()> {
        ensure!(
            self.schema == STATE_SCHEMA,
            "unsupported worker media installation state"
        );
        ensure!(
            self.installation_id == installation_id,
            "worker media installation state belongs to another installation"
        );
        ensure!(
            !self.environment_name.trim().is_empty() && self.environment_name.len() <= 256,
            "worker media installation state has an invalid environment name"
        );
        chrono::DateTime::parse_from_rfc3339(&self.updated_at)
            .context("worker media installation state timestamp is invalid")?;
        if let Some(receipt) = &self.receipt {
            ensure!(
                receipt.installation_id == self.installation_id,
                "worker media installation state receipt belongs to another installation"
            );
        }
        Ok(())
    }
}

/// Sidecar directory below the worker-media root.
pub fn state_directory(worker_media_root: &Path) -> PathBuf {
    worker_media_root.join(STATE_DIR)
}

/// Sidecar path for one installation id.
pub fn state_path(worker_media_root: &Path, installation_id: &str) -> PathBuf {
    state_directory(worker_media_root).join(format!("{installation_id}.json"))
}

/// Read the sidecar of one installation if it exists.
pub fn read(worker_media_root: &Path, installation_id: &str) -> Result<Option<InstallationState>> {
    let path = state_path(worker_media_root, installation_id);
    if !path.try_exists()? {
        return Ok(None);
    }
    let state: InstallationState = read_json(&path).with_context(|| {
        format!(
            "read the worker media installation state {}",
            path.display()
        )
    })?;
    state.validate(installation_id)?;
    Ok(Some(state))
}

fn write(worker_media_root: &Path, state: &InstallationState) -> Result<()> {
    state.validate(&state.installation_id)?;
    let path = state_path(worker_media_root, &state.installation_id);
    write_json(&path, state).with_context(|| {
        format!(
            "write the worker media installation state {}",
            path.display()
        )
    })
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Record a fresh generation before any receipt is known.
pub fn generated(
    worker_media_root: &Path,
    record: &GenerationRecord,
    environment_name: &str,
) -> Result<InstallationState> {
    let state = InstallationState {
        schema: STATE_SCHEMA,
        installation_id: record.installation_id.clone(),
        environment_id: record.environment_id.clone(),
        environment_name: environment_name.to_owned(),
        status: InstallationStatus::Generated,
        receipt: None,
        confirmed_fingerprint: None,
        pinned_host_key: None,
        last_address: None,
        updated_at: now(),
    };
    write(worker_media_root, &state)?;
    Ok(state)
}

/// Remember an imported receipt as pending trust before probing, so an
/// interrupted or failed verification can be retried without re-importing.
pub fn remember_receipt(
    worker_media_root: &Path,
    record: &GenerationRecord,
    environment_name: &str,
    receipt: &ConnectionReceipt,
) -> Result<InstallationState> {
    let previous = read(worker_media_root, &record.installation_id)?;
    let state = InstallationState {
        schema: STATE_SCHEMA,
        installation_id: record.installation_id.clone(),
        environment_id: record.environment_id.clone(),
        environment_name: environment_name.to_owned(),
        status: InstallationStatus::InstalledPending,
        receipt: Some(receipt.clone()),
        confirmed_fingerprint: Some(receipt.host_fingerprint.clone()),
        pinned_host_key: Some(receipt.host_key.clone()),
        last_address: previous.and_then(|previous| previous.last_address),
        updated_at: now(),
    };
    write(worker_media_root, &state)?;
    Ok(state)
}

/// Mark a verified, registered installation as available on the recorded address.
pub fn mark_available(
    worker_media_root: &Path,
    installation_id: &str,
    address: &str,
) -> Result<InstallationState> {
    let mut state = read(worker_media_root, installation_id)?.with_context(|| {
        format!(
            "no worker media installation state exists for installation {installation_id}; import the receipt first"
        )
    })?;
    state.status = InstallationStatus::Available;
    state.last_address = Some(address.to_owned());
    state.updated_at = now();
    write(worker_media_root, &state)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::net::Ipv4Addr;

    fn record() -> GenerationRecord {
        GenerationRecord {
            schema: 1,
            installation_id: "0123456789abcdef0123456789abcdef".into(),
            environment_id: "windows-01234567".into(),
            app_version: "1.0.0".into(),
            template_sha256: "a".repeat(64),
            payload_manifest_sha256: "b".repeat(64),
            controller_public_key: "c".repeat(64),
            ssh_identity_file: PathBuf::from("C:\\state\\ssh\\management_ed25519"),
            management_peer: Ipv4Addr::new(192, 168, 199, 1),
            network_prefix_len: 24,
            ssh_port: 22_240,
            media_path: PathBuf::from("C:\\state\\media"),
            zip_path: PathBuf::from("C:\\state\\media.zip"),
            created_at: "2026-01-01T00:00:00+00:00".into(),
        }
    }

    fn receipt() -> ConnectionReceipt {
        let mut blob = vec![0, 0, 0, 11];
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&[0, 0, 0, 32]);
        blob.extend_from_slice(&[7; 32]);
        let key = SshPublicKey {
            algorithm: "ssh-ed25519".into(),
            key: base64::engine::general_purpose::STANDARD.encode(blob),
        };
        let fingerprint = key.fingerprint().expect("fixture fingerprint");
        ConnectionReceipt {
            schema: 1,
            receipt_version: 2,
            installed: true,
            installation_id: record().installation_id,
            environment_id: record().environment_id,
            generation: 1,
            manifest_sha256: "b".repeat(64),
            ssh_user: format!("wkgm{}", "0123456789abcdef"),
            ssh_port: 22_240,
            host_key: key,
            host_fingerprint: fingerprint,
            candidate_addresses: vec!["192.168.199.40".into()],
            production_isolation_verified: false,
        }
    }

    #[test]
    fn state_round_trips_and_never_contains_private_material() -> Result<()> {
        let root = tempfile::tempdir()?;
        let record = record();
        let generated = generated(root.path(), &record, "Windows Worker 01234567")?;
        assert_eq!(generated.status, InstallationStatus::Generated);
        let restored = read(root.path(), &record.installation_id)?.expect("state exists");
        assert_eq!(restored.installation_id, generated.installation_id);
        assert_eq!(restored.status, InstallationStatus::Generated);
        let state = remember_receipt(root.path(), &record, "Windows Worker 01234567", &receipt())?;
        assert_eq!(state.status, InstallationStatus::InstalledPending);
        assert_eq!(
            state.confirmed_fingerprint.as_deref(),
            Some(receipt().host_fingerprint.as_str())
        );
        let available = mark_available(root.path(), &record.installation_id, "192.168.199.40")?;
        assert_eq!(available.status, InstallationStatus::Available);
        assert_eq!(available.last_address.as_deref(), Some("192.168.199.40"));
        assert_eq!(
            read(root.path(), &record.installation_id)?.map(|state| state.status),
            Some(InstallationStatus::Available)
        );
        let text = std::fs::read_to_string(state_path(root.path(), &record.installation_id))?;
        assert!(!text.contains("private"));
        assert_eq!(state.schema, STATE_SCHEMA);
        assert!(mark_available(root.path(), "f".repeat(32).as_str(), "x").is_err());
        Ok(())
    }

    #[test]
    fn sidecars_are_scoped_to_their_installation() -> Result<()> {
        let root = tempfile::tempdir()?;
        let record = record();
        generated(root.path(), &record, "Windows Worker 01234567")?;
        assert!(read(root.path(), &"f".repeat(32))?.is_none());
        // A sidecar copy that claims another installation identity is refused.
        let path = state_path(root.path(), &record.installation_id);
        let mut value: serde_json::Value = read_json(&path)?;
        value["installation_id"] = serde_json::json!("f".repeat(32));
        write_json(&path, &value)?;
        assert!(read(root.path(), &record.installation_id).is_err());
        Ok(())
    }
}
