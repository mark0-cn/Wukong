//! Request, result and durable media metadata shared with VM provisioning.
use serde::{Deserialize, Serialize};
use std::{net::Ipv4Addr, path::PathBuf};
use wukong_worker_package::SshPublicKey;

/// Schema-1 finalized-media receipt written as `media.json`. The field set and
/// order match the Python finalize output the offline provisioning consumer
/// accepts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaRecord {
    pub schema: u32,
    pub version: String,
    pub controller_public_key: String,
    pub manifest_sha256: String,
    pub setup_sha256: String,
    pub enrollment_sha256: String,
    pub installation_id: String,
    pub environment_id: String,
    pub generation: u64,
    pub source_revision: String,
}

/// Parameters for one finalize operation. Paths must be absolute; `output` must
/// not exist yet.
#[derive(Clone, Debug)]
pub struct FinalizeMediaRequest {
    /// Released template ZIP archive or unpacked template directory.
    pub template: PathBuf,
    /// Absolute controller signing key directory (outside template and output).
    pub keys: PathBuf,
    /// New finalized media directory.
    pub output: PathBuf,
    pub installation_id: String,
    pub environment_id: String,
    pub generation: u64,
    pub management_peer: Ipv4Addr,
    pub ssh_port: u16,
    /// Existing controller-managed SSH client public key.
    pub ssh_public_key: Option<SshPublicKey>,
    /// New private identity path for a controller-managed SSH keypair.
    pub generate_ssh_key: Option<PathBuf>,
    /// Media/controller version; defaults to the template version.
    pub controller_version: Option<String>,
}

/// Result of one successful finalize.
#[derive(Debug)]
pub struct FinalizedMedia {
    pub record: MediaRecord,
    pub media_path: PathBuf,
    /// SHA-256 of the template identity (`media-template.json`).
    pub template_sha256: String,
    pub ssh_public_key: SshPublicKey,
    pub ssh_identity_file: Option<PathBuf>,
}
