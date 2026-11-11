use crate::{Manifest, manifest::MAX_MANIFEST_BYTES};
use anyhow::{Context, Result, ensure};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    pub fn from_hex(value: &str) -> Result<Self> {
        ensure!(value.len() == 64, "invalid worker signing public key");
        Ok(Self(hex::decode(value)?.try_into().map_err(|_| {
            anyhow::anyhow!("invalid worker signing public key")
        })?))
    }
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
    pub fn fingerprint(&self) -> String {
        hex::encode(Sha256::digest(self.0))
    }
}

/// The private key is never serializable or included in package metadata.
/// Callers must store PKCS#8 bytes only in the controller's protected key store.
pub struct Signer(Ed25519KeyPair);

impl Signer {
    pub fn generate() -> Result<(Self, ring::pkcs8::Document)> {
        let document = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .map_err(|_| anyhow::anyhow!("worker signing key generation failed"))?;
        Ok((Self::from_pkcs8(document.as_ref())?, document))
    }
    pub fn from_pkcs8(bytes: &[u8]) -> Result<Self> {
        Ok(Self(Ed25519KeyPair::from_pkcs8(bytes).map_err(|_| {
            anyhow::anyhow!("invalid worker signing private key")
        })?))
    }
    pub fn public_key(&self) -> PublicKey {
        PublicKey(
            self.0
                .public_key()
                .as_ref()
                .try_into()
                .expect("Ed25519 public key length"),
        )
    }
    pub fn sign(&self, manifest: &Manifest) -> Result<(Vec<u8>, SignatureRecord)> {
        let bytes = manifest.canonical_bytes()?;
        let signature = SignatureRecord {
            schema: 1,
            algorithm: "ed25519".into(),
            key_sha256: self.public_key().fingerprint(),
            signature: hex::encode(self.0.sign(&bytes).as_ref()),
        };
        Ok((bytes, signature))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureRecord {
    pub schema: u32,
    pub algorithm: String,
    pub key_sha256: String,
    pub signature: String,
}

/// Construction requires an independently pinned public key. A key delivered
/// beside the package never establishes trust in that same package.
pub struct VerifiedManifest {
    manifest: Manifest,
    sha256: String,
    signer: String,
    trusted: PublicKey,
}

impl VerifiedManifest {
    pub fn verify(bytes: &[u8], signature: &SignatureRecord, trusted: &PublicKey) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_MANIFEST_BYTES,
            "worker manifest exceeds 4 MiB"
        );
        ensure!(
            signature.schema == 1
                && signature.algorithm == "ed25519"
                && signature.key_sha256 == trusted.fingerprint(),
            "worker package signer is not trusted"
        );
        ensure!(
            signature.signature.len() == 128,
            "invalid worker package signature length"
        );
        let decoded = hex::decode(&signature.signature)
            .context("invalid worker package signature encoding")?;
        UnparsedPublicKey::new(&ED25519, trusted.0)
            .verify(bytes, &decoded)
            .map_err(|_| anyhow::anyhow!("worker package signature verification failed"))?;
        let manifest: Manifest =
            serde_json::from_slice(bytes).context("invalid signed worker manifest")?;
        ensure!(
            manifest.canonical_bytes()? == bytes,
            "worker manifest is not canonical"
        );
        Ok(Self {
            manifest,
            sha256: hex::encode(Sha256::digest(bytes)),
            signer: trusted.fingerprint(),
            trusted: trusted.clone(),
        })
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn signer_fingerprint(&self) -> &str {
        &self.signer
    }
    pub(crate) fn trusted_key(&self) -> &PublicKey {
        &self.trusted
    }
}
