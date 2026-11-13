//! Controller-admitted bytes, kept outside Python/target-writable state.
//! No source path supplied by the worker is ever opened with System authority.
use crate::{
    lease_proxy::Identity,
    resource_input::{AdmittedInput, Purpose, Reference},
    security::{self, AclPolicy},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Seek, Write},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ, GetFileInformationByHandle,
};

const MAX_PUT: usize = 32 * 1024 * 1024;
const MAX_MEMBERS: usize = 16_384;
const MAX_SCOPE_BYTES: u64 = 32 * 1024 * 1024 * 1024;

#[path = "inputs_transfers.rs"]
mod transfers;
pub(crate) enum Pending {
    Put(Put),
    Transfer(transfers::Transfer),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
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
    pub fn handles(method: &str) -> bool {
        matches!(
            method,
            "file.put"
                | "file.upload.begin"
                | "file.upload.write"
                | "file.upload.status"
                | "file.upload.commit"
        )
    }
    pub fn prepare_rpc(
        &self,
        owner: Owner,
        method: &str,
        params: &serde_json::Map<String, Value>,
    ) -> Result<Pending> {
        if method == "file.put" {
            let put = Put::parse(owner, params)?;
            self.prepare(&put)?;
            Ok(Pending::Put(put))
        } else {
            Ok(Pending::Transfer(
                self.prepare_transfer(owner, method, params)?,
            ))
        }
    }
    pub fn complete_rpc(&self, pending: &Pending, result: &mut Value) -> Result<()> {
        match pending {
            Pending::Put(put) => self.complete(put, result),
            Pending::Transfer(transfer) => self.complete_transfer(transfer, result),
        }
    }
    pub fn open(metadata: &Path, policy: AclPolicy) -> Result<Self> {
        policy.verify(metadata, true)?;
        // Keep Windows' verified verbatim absolute spelling for direct SDK
        // publication calls, including on guests without long-path opt-in.
        let root = metadata.canonicalize()?.join("native-inputs");
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
        self.open_blob(path, declaration).map(drop)
    }
    fn open_blob(&self, path: &Path, declaration: &Declaration) -> Result<fs::File> {
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
        file.rewind()?;
        Ok(file)
    }
    pub(crate) fn acquire(
        &self,
        owner: &Owner,
        reference: &Reference,
        purpose: Purpose,
    ) -> Result<AdmittedInput> {
        let logical = reference
            .path
            .strip_prefix("files/")
            .context("native input needs a staged logical path")?;
        wukong_worker_package::validate_path(logical)?;
        self.policy.verify(&self.root, true)?;
        let directory = self.root.join(digest(owner)?);
        self.policy.verify(&directory, true)?;
        let key = digest(&reference.path)?;
        let declaration = self.read_record(&directory.join(format!("{key}.declaration.json")))?;
        ensure!(
            declaration.schema == 1
                && declaration.owner == *owner
                && owner.epoch > 0
                && declaration.path == reference.path
                && declaration.size <= 16 * 1024 * 1024 * 1024
                && declaration.size == reference.size
                && declaration.sha256 == reference.sha256
                && declaration.executable == reference.executable
                && digest(&declaration)? == reference.id,
            "native input reference differs from controller admission"
        );
        ensure!(
            !matches!(purpose, Purpose::Execute) || declaration.executable,
            "native input was not admitted as executable"
        );
        ensure!(
            self.read_record(&directory.join(format!("{key}.receipt.json")))? == declaration,
            "native input has no matching completed receipt"
        );
        let file = self.open_blob(&directory.join(format!("{key}.data")), &declaration)?;
        Ok(AdmittedInput::new(file, reference.clone()))
    }
    /// Resolve only a completed logical input in the already admitted owner
    /// scope. The System child still verifies and retains the actual bytes.
    pub(crate) fn reference(
        &self,
        owner: &Owner,
        path: &str,
        purpose: Purpose,
    ) -> Result<Reference> {
        let logical = path
            .strip_prefix("files/")
            .context("native program must be a staged files/... path")?;
        wukong_worker_package::validate_path(logical)?;
        let directory = self.root.join(digest(owner)?);
        self.policy.verify(&directory, true)?;
        let key = digest(&path)?;
        let declaration = self.read_record(&directory.join(format!("{key}.declaration.json")))?;
        ensure!(
            declaration.owner == *owner
                && declaration.path == path
                && declaration.schema == 1
                && (!matches!(purpose, Purpose::Execute) || declaration.executable),
            "native logical input differs from admitted owner or execution policy"
        );
        ensure!(
            self.read_record(&directory.join(format!("{key}.receipt.json")))? == declaration,
            "native logical input is not completed"
        );
        Ok(Reference {
            path: path.into(),
            id: digest(&declaration)?,
            sha256: declaration.sha256,
            size: declaration.size,
            executable: declaration.executable,
        })
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
        self.reserve(
            &directory,
            &[
                (&record, bytes.len() as u64),
                (&blob, put.declaration.size),
                (&receipt, bytes.len() as u64),
            ],
        )?;
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
        self.finish(&put.declaration, result)
    }
    fn finish(&self, declaration: &Declaration, result: &mut Value) -> Result<()> {
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
        // Keep the exact verified data handle reserved through receipt
        // publication, just as native resource ingestion retains it while copying.
        let _source = self.open_blob(&directory.join(format!("{key}.data")), declaration)?;
        let receipt = directory.join(format!("{key}.receipt.json"));
        if receipt.exists() {
            ensure!(
                self.read_record(&receipt)? == *declaration,
                "native input receipt changed"
            );
        } else {
            self.reserve(
                &directory,
                &[(&receipt, serde_json::to_vec(declaration)?.len() as u64)],
            )?;
            self.publish(&receipt, &serde_json::to_vec(declaration)?)?;
        }
        result["native_input"] = serde_json::json!({"schema":1,"id":digest(declaration)?,
            "identity":declaration.owner.identity,"run_id":declaration.owner.run_id,
            "owner_id":declaration.owner.owner_id,"epoch":declaration.owner.epoch,
            "sha256":declaration.sha256,"size":declaration.size,"executable":declaration.executable});
        Ok(())
    }
    fn reserve(&self, directory: &Path, pending: &[(&Path, u64)]) -> Result<()> {
        // Include interrupted temporary files and every proposed physical byte.
        let mut count = 0usize;
        let mut size = 0u64;
        for entry in fs::read_dir(directory)? {
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
        for (path, length) in pending {
            let actual = if path.exists() {
                fs::symlink_metadata(path)?.len()
            } else {
                count += 1;
                0
            };
            size = size
                .checked_add(length.saturating_sub(actual))
                .context("native input budget overflow")?;
        }
        ensure!(count <= MAX_MEMBERS, "native input member budget exhausted");
        ensure!(
            size <= MAX_SCOPE_BYTES,
            "native input byte budget exhausted"
        );
        Ok(())
    }
    fn publish(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        self.policy.publish(path, bytes)
    }
}
pub(crate) fn digest(value: &impl Serialize) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

#[cfg(test)]
#[path = "inputs_tests.rs"]
mod tests;
