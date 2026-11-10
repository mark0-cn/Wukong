//! Bounded native binary inspection and the Ghidra headless worker protocol.
//!
//! All addresses carry their address space. Static inspection never pretends to
//! know a process's ASLR load address; unsupported mitigation checks stay unknown.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

mod ghidra;
mod profile;
mod strings;
pub use ghidra::{GhidraBackend, GhidraConfig, GhidraQuery, GhidraResult, XrefDirection};
pub use profile::{
    BinaryProfile, ImportInfo, Mitigation, MitigationState, SectionInfo, SymbolInfo, inspect,
};
pub use strings::{BinaryString, StringsQuery, StringsResult, strings};

const MAX_BINARY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ITEMS: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AddressSpace {
    FileOffset,
    ModuleRelative,
    Runtime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddressRef {
    pub module_sha256: String,
    pub space: AddressSpace,
    pub value: u64,
    pub runtime_instance_id: Option<String>,
    pub load_base: Option<u64>,
}
impl AddressRef {
    pub fn module_relative(hash: &str, value: u64) -> Self {
        Self {
            module_sha256: hash.into(),
            space: AddressSpace::ModuleRelative,
            value,
            runtime_instance_id: None,
            load_base: None,
        }
    }
    pub fn file_offset(hash: &str, value: u64) -> Self {
        Self {
            space: AddressSpace::FileOffset,
            ..Self::module_relative(hash, value)
        }
    }
    pub fn runtime(hash: &str, value: u64, instance: &str, base: u64) -> Result<Self> {
        if instance.is_empty() || value < base {
            bail!("runtime addresses require an instance and value >= load_base");
        }
        Ok(Self {
            module_sha256: hash.into(),
            space: AddressSpace::Runtime,
            value,
            runtime_instance_id: Some(instance.into()),
            load_base: Some(base),
        })
    }
    pub fn validate(&self) -> Result<()> {
        if self.module_sha256.len() != 64
            || !self.module_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            bail!("address module_sha256 must be a 64-character SHA256");
        }
        match self.space {
            AddressSpace::Runtime
                if self
                    .runtime_instance_id
                    .as_ref()
                    .is_none_or(String::is_empty)
                    || self.load_base.is_none_or(|base| base > self.value) =>
            {
                bail!("runtime address requires an instance and valid load_base")
            }
            AddressSpace::FileOffset | AddressSpace::ModuleRelative
                if self.runtime_instance_id.is_some() || self.load_base.is_some() =>
            {
                bail!("static address cannot contain runtime instance/load base")
            }
            _ => Ok(()),
        }
    }
}

pub fn sha256_bytes(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}
pub fn sha256_file(path: impl AsRef<Path>) -> Result<String> {
    let mut file = fs::File::open(path.as_ref())
        .with_context(|| format!("open {}", path.as_ref().display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}
fn read_binary(path: &Path) -> Result<Vec<u8>> {
    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    if !file.metadata()?.is_file() {
        bail!("target must be a regular file");
    }
    if file.metadata()?.len() > MAX_BINARY_BYTES {
        bail!("binary exceeds 512 MiB inspection limit");
    }
    let mut data = Vec::new();
    file.take(MAX_BINARY_BYTES + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_BINARY_BYTES {
        bail!("binary grew beyond inspection limit");
    }
    Ok(data)
}
