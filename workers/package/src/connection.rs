//! Public handoff from offline Windows setup to the controller.
use crate::SshPublicKey;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

/// Bound on a public `connection.json` document.
pub const MAX_RECEIPT_BYTES: usize = 16 * 1024;
/// Bound on guest address hints, matching the exporter.
pub const MAX_CANDIDATE_ADDRESSES: usize = 8;
/// Current installation receipt version.
pub const RECEIPT_VERSION: u64 = 2;

/// Public worker connection receipt emitted by the offline Windows setup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionReceipt {
    pub schema: u32,
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
    pub candidate_addresses: Vec<String>,
    pub production_isolation_verified: bool,
}

impl ConnectionReceipt {
    /// Parse a bounded receipt and validate its common public fields.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_RECEIPT_BYTES,
            "the public connection receipt exceeds 16 KiB"
        );
        let receipt: Self =
            serde_json::from_slice(bytes).context("invalid public Windows connection receipt")?;
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1 && self.installed && self.generation >= 1,
            "the connection receipt does not describe a completed worker installation"
        );
        ensure!(
            self.receipt_version == RECEIPT_VERSION,
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
        ensure!(
            !self.environment_id.is_empty()
                && self.environment_id.len() <= 64
                && self
                    .environment_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
            "the connection receipt environment identity is invalid"
        );
        ensure!(
            crate::paths::hash(&self.manifest_sha256),
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
