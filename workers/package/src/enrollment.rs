//! Public deployment instructions. Existing installation trust always wins over
//! keys delivered in a new envelope; initial trust requires the VM owner.
use crate::{PublicKey, SignatureRecord, Signer, VerifiedManifest, paths};
use anyhow::{Context, Result, ensure};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, net::IpAddr, path::Path};

const DOMAIN: &[u8] = b"Wukong/Windows-worker-enrollment/v1\0";
const MAX_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshPublicKey {
    pub algorithm: String,
    pub key: String,
}
impl SshPublicKey {
    fn blob(&self) -> Result<Vec<u8>> {
        ensure!(
            self.algorithm == "ssh-ed25519" && self.key.len() == 68,
            "only canonical Ed25519 SSH public keys are supported"
        );
        let bytes = STANDARD
            .decode(&self.key)
            .context("invalid SSH public key base64")?;
        ensure!(
            bytes.len() == 51
                && bytes[..4] == [0, 0, 0, 11]
                && &bytes[4..15] == b"ssh-ed25519"
                && bytes[15..19] == [0, 0, 0, 32]
                && STANDARD.encode(&bytes) == self.key,
            "invalid SSH public key structure"
        );
        Ok(bytes)
    }
    pub fn fingerprint(&self) -> Result<String> {
        Ok(format!(
            "SHA256:{}",
            STANDARD_NO_PAD.encode(Sha256::digest(self.blob()?))
        ))
    }
    pub fn authorized_key(&self) -> Result<String> {
        self.blob()?;
        Ok(format!("{} {}", self.algorithm, self.key))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    pub schema: u32,
    pub generation: u64,
    pub installation_id: String,
    pub environment_id: String,
    pub controller_public_key: String,
    pub payload_manifest_sha256: String,
    pub management_peer: IpAddr,
    pub ssh_port: u16,
    pub ssh_public_key: SshPublicKey,
}
impl Enrollment {
    fn validate(&self) -> Result<()> {
        ensure!(self.schema == 1, "unsupported worker enrollment format");
        ensure!(self.generation > 0, "missing enrollment generation");
        ensure!(
            self.installation_id.len() == 32
                && self
                    .installation_id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid installation identity"
        );
        ensure!(
            !self.environment_id.is_empty()
                && self.environment_id.len() <= 64
                && self
                    .environment_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
            "invalid enrollment environment ID"
        );
        PublicKey::from_hex(&self.controller_public_key)?;
        ensure!(
            paths::hash(&self.payload_manifest_sha256),
            "invalid enrollment payload identity"
        );
        ensure!(
            self.management_peer == self.management_peer.to_canonical()
                && self.ssh_port > 0
                && !self.management_peer.is_unspecified()
                && !self.management_peer.is_loopback()
                && !self.management_peer.is_multicast()
                && !matches!(self.management_peer, IpAddr::V4(ip) if ip.is_broadcast()),
            "invalid SSH management address or port"
        );
        self.ssh_public_key.blob()?;
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= MAX_BYTES, "worker enrollment exceeds 16 KiB");
        Ok(bytes)
    }
    pub fn sign(&self, signer: &Signer) -> Result<(Vec<u8>, SignatureRecord)> {
        ensure!(
            self.controller_public_key == signer.public_key().to_hex(),
            "enrollment controller differs from signer"
        );
        let bytes = self.canonical_bytes()?;
        let message = [DOMAIN, bytes.as_slice()].concat();
        Ok((bytes, signer.sign_bytes(&message)))
    }
}

pub struct VerifiedEnrollment(Enrollment);
impl VerifiedEnrollment {
    /// `trusted` must come from protected installation state or explicit initial
    /// owner enrollment, never automatically from the incoming envelope.
    pub fn verify(bytes: &[u8], signature: &SignatureRecord, trusted: &PublicKey) -> Result<Self> {
        ensure!(bytes.len() <= MAX_BYTES, "worker enrollment exceeds 16 KiB");
        trusted.verify_bytes(&[DOMAIN, bytes].concat(), signature)?;
        let enrollment: Enrollment = serde_json::from_slice(bytes)?;
        ensure!(
            enrollment.controller_public_key == trusted.to_hex(),
            "enrollment controller is not trusted"
        );
        ensure!(
            enrollment.canonical_bytes()? == bytes,
            "worker enrollment is not canonical"
        );
        Ok(Self(enrollment))
    }
    pub fn enrollment(&self) -> &Enrollment {
        &self.0
    }
    /// `previous` is the protected highest accepted record, including an update
    /// whose activation failed. Repeated identical requests reconcile receipts;
    /// they must not call this entry to reactivate an old authority/SSH key.
    pub fn authorize_after(&self, previous: &Self) -> Result<()> {
        ensure!(
            self.0.controller_public_key == previous.0.controller_public_key
                && self.0.installation_id == previous.0.installation_id
                && self.0.environment_id == previous.0.environment_id,
            "enrollment cannot replace installation authority or identity"
        );
        ensure!(
            self.0.generation > previous.0.generation,
            "stale or replayed enrollment generation"
        );
        Ok(())
    }
    pub fn bind_payload(&self, payload: &VerifiedManifest) -> Result<()> {
        ensure!(
            payload.sha256() == self.0.payload_manifest_sha256
                && payload.signer_fingerprint()
                    == PublicKey::from_hex(&self.0.controller_public_key)?.fingerprint(),
            "enrollment and offline payload differ"
        );
        Ok(())
    }
}

/// Controller-only publication of public enrollment records. This never writes
/// private signing/SSH keys, replaces existing output, or establishes first trust.
pub fn write_enrollment(
    output: &Path,
    enrollment: &Enrollment,
    signer: &Signer,
    payload: &VerifiedManifest,
) -> Result<VerifiedEnrollment> {
    let (bytes, signature) = enrollment.sign(signer)?;
    let verified = VerifiedEnrollment::verify(&bytes, &signature, &signer.public_key())?;
    verified.bind_payload(payload)?;
    ensure!(output.is_absolute(), "enrollment output must be absolute");
    crate::files::package_root(
        output
            .parent()
            .context("missing enrollment output parent")?,
    )?;
    fs::create_dir(output)?;
    let signature = serde_json::to_vec(&signature)?;
    for (name, bytes) in [
        ("enrollment.json", bytes.as_slice()),
        ("enrollment.sig", signature.as_slice()),
    ] {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output.join(name))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    Ok(verified)
}
