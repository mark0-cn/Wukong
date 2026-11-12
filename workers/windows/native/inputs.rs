//! Controller-admitted bytes, kept outside Python/target-writable state.
//! No source path supplied by the worker is ever opened with System authority.
use crate::{
    lease_proxy::Identity,
    security::{self, AclPolicy},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ, GetFileInformationByHandle, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

const MAX_PUT: usize = 32 * 1024 * 1024;
const MAX_MEMBERS: usize = 16_384;
const MAX_SCOPE_BYTES: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Owner {
    pub identity: Identity,
    pub run_id: String,
    pub owner_id: String,
    pub epoch: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    schema: u32,
    owner: Owner,
    path: String,
    sha256: String,
    size: u64,
    executable: bool,
}
pub(crate) struct Put {
    declaration: Declaration,
    data: Vec<u8>,
}
impl Put {
    /// Called only after the service has admitted the controller's fence. The
    /// declaration is computed from that request, never a Python file/receipt.
    pub fn parse(owner: Owner, params: &serde_json::Map<String, Value>) -> Result<Self> {
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .context("native upload path is required")?;
        wukong_worker_package::validate_path(path)?;
        let encoded = params
            .get("data_base64")
            .and_then(Value::as_str)
            .context("native upload bytes are required")?;
        ensure!(
            encoded.len() <= 4 * MAX_PUT.div_ceil(3),
            "native upload exceeds 32 MiB"
        );
        let data = STANDARD.decode(encoded)?;
        ensure!(data.len() <= MAX_PUT, "native upload exceeds 32 MiB");
        let executable = match params.get("executable") {
            None => false,
            Some(Value::Bool(value)) => *value,
            _ => anyhow::bail!("native executable policy must be a boolean"),
        };
        let declaration = Declaration {
            schema: 1,
            owner,
            path: format!("files/{path}"),
            sha256: hex::encode(Sha256::digest(&data)),
            size: data.len() as u64,
            executable,
        };
        Ok(Self { declaration, data })
    }
}

pub(crate) struct Inputs {
    root: PathBuf,
    policy: AclPolicy,
}
impl Inputs {
    pub fn open(metadata: &Path, policy: AclPolicy) -> Result<Self> {
        policy.verify(metadata, true)?;
        let root = metadata.join("native-inputs");
        if !root.exists() {
            policy.create_directory(&root)?;
        }
        policy.verify(&root, true)?;
        Ok(Self { root, policy })
    }
    fn directory(&self, declaration: &Declaration) -> Result<PathBuf> {
        self.policy.verify(&self.root, true)?;
        let directory = self.root.join(digest(&declaration.owner)?);
        if !directory.exists() {
            self.policy.create_directory(&directory)?;
        }
        self.policy.verify(&directory, true)?;
        Ok(directory)
    }
    fn read_record(&self, path: &Path) -> Result<Declaration> {
        self.policy.verify(path, false)?;
        Ok(serde_json::from_slice(&crate::deployment::read_bytes(
            path, 8192,
        )?)?)
    }
    fn check_blob(&self, path: &Path, declaration: &Declaration) -> Result<()> {
        self.policy.verify(path, false)?;
        let mut file = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        security::check(unsafe {
            GetFileInformationByHandle(file.as_raw_handle(), &mut information)
        })?;
        ensure!(
            information.nNumberOfLinks == 1
                && information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
            "native input must retain its original single-link file identity"
        );
        ensure!(
            file.metadata()?.len() == declaration.size,
            "native input size differs"
        );
        let mut hash = Sha256::new();
        let count = std::io::copy(&mut file, &mut hash)?;
        ensure!(
            count == declaration.size && hex::encode(hash.finalize()) == declaration.sha256,
            "native input content differs"
        );
        Ok(())
    }
    /// Prepare before forwarding. Bytes are immutable, but there is no admitted
    /// receipt until the corresponding worker result is checked and settled.
    pub fn prepare(&self, put: &Put) -> Result<()> {
        let directory = self.directory(&put.declaration)?;
        let key = digest(&put.declaration.path)?;
        let record = directory.join(format!("{key}.declaration.json"));
        if record.exists() {
            ensure!(
                self.read_record(&record)? == put.declaration,
                "native input path already has a different declaration"
            );
        }
        let blob = directory.join(format!("{key}.data"));
        let receipt = directory.join(format!("{key}.receipt.json"));
        let bytes = serde_json::to_vec(&put.declaration)?;
        {
            // Count every member, including interrupted temporary writes. No
            // unbounded directory scan or ignored orphan can bypass the budget.
            let mut count = 0usize;
            let mut size = 0u64;
            for entry in fs::read_dir(&directory)? {
                count += 1;
                ensure!(count <= MAX_MEMBERS, "native input member budget exhausted");
                let path = entry?.path();
                self.policy.verify(&path, false)?;
                size = size
                    .checked_add(fs::symlink_metadata(path)?.len())
                    .context("native input budget overflow")?;
                ensure!(
                    size <= MAX_SCOPE_BYTES,
                    "native input byte budget exhausted"
                );
            }
            for (path, length) in [
                (&record, bytes.len() as u64),
                (&blob, put.declaration.size),
                (&receipt, bytes.len() as u64),
            ] {
                if !path.exists() {
                    count += 1;
                    size = size
                        .checked_add(length)
                        .context("native input budget overflow")?;
                }
            }
            ensure!(count <= MAX_MEMBERS, "native input member budget exhausted");
            ensure!(
                size <= MAX_SCOPE_BYTES,
                "native input byte budget exhausted"
            );
        }
        if !record.exists() {
            self.publish(&record, &bytes)?;
        }
        if !blob.exists() {
            self.publish(&blob, &put.data)?;
        }
        self.check_blob(&blob, &put.declaration)
    }
    /// Only a valid successful worker reply can expose this proof. A lost,
    /// malformed or conflicting reply leaves the native lease uncertain.
    pub fn complete(&self, put: &Put, result: &mut Value) -> Result<()> {
        let declaration = &put.declaration;
        ensure!(
            result.is_object()
                && result["path"] == declaration.path
                && result["sha256"] == declaration.sha256
                && result["size"] == declaration.size,
            "worker upload receipt differs from controller-admitted bytes"
        );
        ensure!(
            result.get("native_input").is_none(),
            "worker cannot issue native input proof"
        );
        let directory = self.directory(declaration)?;
        let key = digest(&declaration.path)?;
        ensure!(
            self.read_record(&directory.join(format!("{key}.declaration.json")))? == *declaration,
            "native input declaration changed"
        );
        self.check_blob(&directory.join(format!("{key}.data")), declaration)?;
        let receipt = directory.join(format!("{key}.receipt.json"));
        if receipt.exists() {
            ensure!(
                self.read_record(&receipt)? == *declaration,
                "native input receipt changed"
            );
        } else {
            self.publish(&receipt, &serde_json::to_vec(declaration)?)?;
        }
        result["native_input"] = serde_json::json!({"schema":1,"id":digest(declaration)?,
            "identity":declaration.owner.identity,"run_id":declaration.owner.run_id,
            "owner_id":declaration.owner.owner_id,"epoch":declaration.owner.epoch,
            "sha256":declaration.sha256,"size":declaration.size,"executable":declaration.executable});
        Ok(())
    }
    fn publish(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let parent = path.parent().context("native input has no parent")?;
        self.policy.verify(parent, true)?;
        let temporary = parent.join(format!(
            "{}.tmp",
            hex::encode(crate::accounts::random_bytes::<16>()?)
        ));
        let mut file = self.policy.create_file(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        let from = security::wide(temporary.as_os_str())?;
        let to = security::wide(path.as_os_str())?;
        // No REPLACE_EXISTING: an existing declaration/blob/receipt is immutable.
        security::check(unsafe {
            MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH)
        })?;
        self.policy.verify(path, false)
    }
}
fn digest(value: &impl Serialize) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

#[cfg(test)]
#[path = "inputs_tests.rs"]
mod tests;
