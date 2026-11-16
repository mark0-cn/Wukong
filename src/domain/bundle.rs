//! A directory of related binaries and data, discovered without user-assigned roles.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const BUNDLE_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleManifest {
    pub schema: u32,
    /// Absolute source directory used for staging; not a security boundary.
    pub root: PathBuf,
    pub bundle_sha256: String,
    /// Auto-selected execution candidate, relative to root (forward slashes).
    pub entry: String,
    pub entry_sha256: String,
    pub members: Vec<BundleMember>,
    /// Import-library edges whose library resolves to another bundle member.
    pub edges: Vec<BundleEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleMember {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    /// pe/elf/macho/unknown.
    pub format: String,
    /// executable/library/data, inferred from the binary, never requested from the user.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<u64>,
    #[serde(default)]
    pub imports: Vec<String>,
    #[serde(default)]
    pub libraries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleEdge {
    pub from: String,
    pub to: String,
    pub library: String,
}
