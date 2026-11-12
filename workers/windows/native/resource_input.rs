//! Native resource input admission. A worker path/proof is a selector, never
//! authority: the native caller supplies the already validated current owner.
use crate::{
    inputs::Inputs,
    resource_journal::Owner,
    security::{self, AclPolicy},
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub path: String,
    pub id: String,
    pub sha256: String,
    pub size: u64,
    pub executable: bool,
}

#[derive(Clone, Copy)]
pub enum Purpose {
    Inspect,
    Execute,
}

/// Owns the exact verified file handle, denying write/delete sharing until the
/// native caller finishes copying its bytes. It exposes no arbitrary OS path.
pub struct AdmittedInput {
    file: File,
    reference: Reference,
}
impl AdmittedInput {
    pub(crate) fn new(file: File, reference: Reference) -> Self {
        Self { file, reference }
    }
    pub fn reference(&self) -> &Reference {
        &self.reference
    }
}
impl Read for AdmittedInput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.file.read(buffer)
    }
}

/// Internal System-side API, not a worker RPC. The caller must first validate
/// its current lease; possession of Reference/Owner does not authenticate it.
pub fn open(
    metadata: &Path,
    owner: &Owner,
    reference: &Reference,
    purpose: Purpose,
) -> Result<AdmittedInput> {
    security::require_system()?;
    let policy = AclPolicy::private_vm();
    policy.verify(metadata, true)?;
    // Lookup never creates a missing store or owner scope.
    policy.verify(&metadata.join("native-inputs"), true)?;
    Inputs::open(metadata, policy)?.acquire(owner, reference, purpose)
}
