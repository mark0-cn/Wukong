//! Public Windows connection receipt v2 parsing and installation cross-checks.
//!
//! The receipt is written by the offline Windows setup on the guest and exported
//! by the operator. It is public evidence only: the host key is pinned only after
//! this controller recomputes its fingerprint and the operator independently
//! confirms it. Every check here is pure so the validation matrix is testable
//! without a VM.
use super::state::InstallationState;
use crate::adapters::environments::validate_id;
use crate::application::worker_media::GenerationRecord;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::Path};
use wukong_worker_package::SshPublicKey;

/// Bound on a public `connection.json` document.
pub const MAX_RECEIPT_BYTES: usize = 16 * 1024;
/// Bound on guest address hints, matching the exporter.
pub const MAX_CANDIDATE_ADDRESSES: usize = 8;
/// Highest receipt schema revision this controller understands.
pub const SUPPORTED_RECEIPT_VERSION: u64 = 2;

/// Public worker connection receipt emitted by the offline Windows setup.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionReceipt {
    pub schema: u32,
    #[serde(default = "default_receipt_version")]
    pub receipt_version: u64,
    pub installed: bool,
    pub installation_id: String,
    pub environment_id: String,
    pub generation: u64,
    pub manifest_sha256: String,
    pub ssh_user: String,
    pub ssh_port: u16,
    pub host_key: SshPublicKey,
    pub host_fingerprint: String,
    #[serde(default)]
    pub candidate_addresses: Vec<String>,
    pub production_isolation_verified: bool,
}

fn default_receipt_version() -> u64 {
    1
}

/// Read a receipt supplied either as the JSON document itself or as a file path.
pub fn read_input(input: &str) -> Result<Vec<u8>> {
    let trimmed = input.trim();
    ensure!(
        !trimmed.is_empty(),
        "the connection receipt is empty; choose the connection.json exported by the Windows setup"
    );
    if trimmed.starts_with('{') {
        ensure!(
            trimmed.len() <= MAX_RECEIPT_BYTES,
            "the public connection receipt exceeds 16 KiB"
        );
        return Ok(trimmed.as_bytes().to_vec());
    }
    let path = std::path::absolute(Path::new(trimmed))
        .with_context(|| format!("resolve the connection receipt path {trimmed}"))?;
    let bytes = read_bounded(&path)?;
    Ok(bytes)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    ensure!(
        path.is_file(),
        "the connection receipt file is missing: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    fs::File::open(path)
        .with_context(|| format!("open the connection receipt {}", path.display()))?
        .take(MAX_RECEIPT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read the connection receipt {}", path.display()))?;
    ensure!(
        bytes.len() <= MAX_RECEIPT_BYTES,
        "the public connection receipt exceeds 16 KiB"
    );
    Ok(bytes)
}

/// Parse and validate one public receipt document.
pub fn parse(bytes: &[u8]) -> Result<ConnectionReceipt> {
    ensure!(
        bytes.len() <= MAX_RECEIPT_BYTES,
        "the public connection receipt exceeds 16 KiB"
    );
    let receipt: ConnectionReceipt =
        serde_json::from_slice(bytes).context("invalid public Windows connection receipt")?;
    receipt.validate()?;
    Ok(receipt)
}

impl ConnectionReceipt {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1 && self.installed && self.generation >= 1,
            "the connection receipt does not describe a completed worker installation"
        );
        ensure!(
            (1..=SUPPORTED_RECEIPT_VERSION).contains(&self.receipt_version),
            "unsupported connection receipt version; export the install result from the matching Windows setup"
        );
        ensure!(
            self.installation_id.len() == 32
                && self
                    .installation_id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "the connection receipt installation identity is invalid"
        );
        validate_id(&self.environment_id)?;
        ensure!(
            self.manifest_sha256.len() == 64
                && self
                    .manifest_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "the connection receipt payload identity is invalid"
        );
        ensure!(
            self.ssh_user == format!("wkgm{}", &self.installation_id[..16]),
            "the connection receipt management account differs from its installation identity"
        );
        ensure!(
            self.ssh_port > 0,
            "the connection receipt has no management SSH port"
        );
        let fingerprint = self
            .host_key
            .fingerprint()
            .context("the connection receipt worker host key is invalid")?;
        ensure!(
            self.host_fingerprint == fingerprint,
            "the connection receipt host fingerprint does not match its host key"
        );
        ensure!(
            self.candidate_addresses.len() <= MAX_CANDIDATE_ADDRESSES
                && self
                    .candidate_addresses
                    .iter()
                    .all(|candidate| candidate.parse::<std::net::Ipv4Addr>().is_ok()),
            "the connection receipt address candidates are invalid; export a refreshed install result"
        );
        Ok(())
    }

    /// The operator must confirm the fingerprint independently before any pin.
    pub fn confirm(&self, confirmed_fingerprint: &str) -> Result<()> {
        ensure!(
            confirmed_fingerprint.trim() == self.host_fingerprint,
            "the independently confirmed SSH fingerprint does not match the receipt host key"
        );
        Ok(())
    }
}

/// Match the receipt to the generation this controller produced.
///
/// A receipt for another installation or another payload must never be imported:
/// it would describe a worker whose media this controller cannot vouch for.
pub fn match_generation(
    receipt: &ConnectionReceipt,
    records: &[GenerationRecord],
) -> Result<GenerationRecord> {
    let record = records
        .iter()
        .find(|record| record.installation_id == receipt.installation_id)
        .with_context(|| {
            format!(
                "not generated by this controller: no generation record for installation {}",
                receipt.installation_id
            )
        })?;
    ensure!(
        record.payload_manifest_sha256 == receipt.manifest_sha256,
        "not this installation task: the receipt payload differs from the generated media"
    );
    ensure!(
        record.environment_id == receipt.environment_id && record.ssh_port == receipt.ssh_port,
        "not this installation task: the receipt enrollment differs from the generated media"
    );
    Ok(record.clone())
}

/// Refuse to replace a host key already pinned for this installation.
pub fn require_pinned_key(
    receipt: &ConnectionReceipt,
    pinned: Option<&SshPublicKey>,
) -> Result<()> {
    if let Some(pinned) = pinned {
        // `SshPublicKey` carries no ordering identity; compare its canonical
        // algorithm/key fields, which the fingerprint already validated.
        ensure!(
            pinned.algorithm == receipt.host_key.algorithm && pinned.key == receipt.host_key.key,
            "a different worker host key is already pinned for this installation; the connection was not replaced"
        );
    }
    Ok(())
}

/// Resolve the receipt path from a saved sidecar when retrying.
pub(crate) fn retry_receipt(state: &InstallationState) -> Result<ConnectionReceipt> {
    let receipt = state.receipt.clone().with_context(|| {
        format!(
            "no imported connection receipt is stored for installation {}; import the receipt first",
            state.installation_id
        )
    })?;
    receipt.validate()?;
    require_pinned_key(&receipt, state.pinned_host_key.as_ref())?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::{net::Ipv4Addr, path::PathBuf};

    fn host_key(fill: u8) -> SshPublicKey {
        let mut blob = vec![0, 0, 0, 11];
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&[0, 0, 0, 32]);
        blob.extend_from_slice(&[fill; 32]);
        SshPublicKey {
            algorithm: "ssh-ed25519".into(),
            key: base64::engine::general_purpose::STANDARD.encode(blob),
        }
    }

    fn receipt() -> ConnectionReceipt {
        let key = host_key(7);
        ConnectionReceipt {
            schema: 1,
            receipt_version: 2,
            installed: true,
            installation_id: "0123456789abcdef0123456789abcdef".into(),
            environment_id: "windows-01234567".into(),
            generation: 1,
            manifest_sha256: "b".repeat(64),
            ssh_user: format!("wkgm{}", "0123456789abcdef"),
            ssh_port: 22_240,
            host_fingerprint: key.fingerprint().expect("fixture fingerprint"),
            host_key: key,
            candidate_addresses: vec!["192.168.199.40".into()],
            production_isolation_verified: false,
        }
    }

    fn record(installation_id: &str) -> GenerationRecord {
        GenerationRecord {
            schema: 1,
            installation_id: installation_id.into(),
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

    #[test]
    fn valid_receipts_parse_and_require_an_independent_confirmation() -> Result<()> {
        let receipt = receipt();
        let bytes = serde_json::to_vec(&receipt)?;
        let parsed = parse(&bytes)?;
        assert_eq!(parsed.installation_id, receipt.installation_id);
        receipt.confirm(&receipt.host_fingerprint)?;
        assert!(receipt.confirm("SHA256:other").is_err());
        assert!(parse(b"{}").is_err());
        assert!(parse(&vec![b'x'; MAX_RECEIPT_BYTES + 1]).is_err());
        Ok(())
    }

    #[test]
    fn version_one_receipts_remain_importable_without_address_hints() -> Result<()> {
        let mut value = serde_json::to_value(receipt())?;
        value["receipt_version"] = serde_json::json!(1);
        value
            .as_object_mut()
            .expect("object")
            .remove("candidate_addresses");
        let parsed = parse(&serde_json::to_vec(&value)?)?;
        assert!(parsed.candidate_addresses.is_empty());
        let mut unsupported = serde_json::to_value(receipt())?;
        unsupported["receipt_version"] = serde_json::json!(3);
        assert!(parse(&serde_json::to_vec(&unsupported)?).is_err());
        Ok(())
    }

    #[test]
    fn receipts_are_rejected_for_bad_fingerprints_and_candidates() -> Result<()> {
        let mut value = serde_json::to_value(receipt())?;
        value["host_fingerprint"] = serde_json::json!("SHA256:tampered");
        assert!(parse(&serde_json::to_vec(&value)?).is_err());
        let mut value = serde_json::to_value(receipt())?;
        value["candidate_addresses"] = serde_json::json!(["not-an-address"]);
        assert!(parse(&serde_json::to_vec(&value)?).is_err());
        let mut value = serde_json::to_value(receipt())?;
        value["candidate_addresses"] = serde_json::json!(
            (0..9)
                .map(|last| format!("192.168.199.{last}"))
                .collect::<Vec<_>>()
        );
        assert!(parse(&serde_json::to_vec(&value)?).is_err());
        Ok(())
    }

    #[test]
    fn unknown_installations_and_foreign_payloads_are_refused() -> Result<()> {
        let receipt = receipt();
        let records = vec![record("f".repeat(32).as_str())];
        let error = match_generation(&receipt, &records).unwrap_err();
        assert!(format!("{error:#}").contains("not generated by this controller"));
        assert!(match_generation(&receipt, &[]).is_err());
        let matching = record(&receipt.installation_id);
        assert_eq!(
            match_generation(&receipt, std::slice::from_ref(&matching))?.installation_id,
            receipt.installation_id
        );
        let mut foreign = matching.clone();
        foreign.payload_manifest_sha256 = "d".repeat(64);
        let error = match_generation(&receipt, &[foreign]).unwrap_err();
        assert!(format!("{error:#}").contains("not this installation task"));
        let mut foreign = matching.clone();
        foreign.ssh_port = 22_299;
        let error = match_generation(&receipt, &[foreign]).unwrap_err();
        assert!(format!("{error:#}").contains("not this installation task"));
        let mut foreign = matching;
        foreign.environment_id = "windows-other".into();
        assert!(match_generation(&receipt, &[foreign]).is_err());
        Ok(())
    }

    #[test]
    fn a_previously_pinned_host_key_cannot_be_replaced() -> Result<()> {
        let receipt = receipt();
        require_pinned_key(&receipt, None)?;
        require_pinned_key(&receipt, Some(&receipt.host_key))?;
        let error = require_pinned_key(&receipt, Some(&host_key(9))).unwrap_err();
        assert!(format!("{error:#}").contains("different worker host key"));
        Ok(())
    }
}
