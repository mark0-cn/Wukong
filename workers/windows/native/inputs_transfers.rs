//! Native provenance for the existing bounded resumable-upload protocol.
use super::*;
use std::io::{Read, Seek, SeekFrom};

const CHUNK: u64 = 1024 * 1024;
const MAX_TRANSFER: u64 = 16 * 1024 * 1024 * 1024;
pub(crate) struct Transfer {
    declaration: Declaration,
    transfer_id: String,
    action: Action,
}
#[derive(Clone, Copy)]
enum Action {
    Begin,
    Status,
    Write { end: u64 },
    Commit,
}
fn text<'a>(params: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .context("native transfer requires a string field")
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn transfer_id(declaration: &Declaration) -> Result<String> {
    // Match the existing Python protocol's sorted JSON, including separators.
    // Portable ASCII paths make its ensure_ascii behavior unambiguous.
    let spec = format!(
        "{{\"executable\": {}, \"path\": {}, \"sha256\": {}, \"size\": {}}}",
        declaration.executable,
        serde_json::to_string(
            declaration
                .path
                .strip_prefix("files/")
                .context("invalid native logical path")?
        )?,
        serde_json::to_string(&declaration.sha256)?,
        declaration.size
    );
    Ok(hex::encode(Sha256::digest(spec.as_bytes())))
}
impl Inputs {
    pub(super) fn prepare_transfer(
        &self,
        owner: Owner,
        method: &str,
        params: &serde_json::Map<String, Value>,
    ) -> Result<Transfer> {
        if method == "file.upload.begin" {
            let path = text(params, "path")?;
            wukong_worker_package::validate_path(path)?;
            let sha256 = text(params, "sha256")?;
            ensure!(hash(sha256), "invalid native transfer SHA256");
            let size = params
                .get("size")
                .and_then(Value::as_u64)
                .filter(|size| *size <= MAX_TRANSFER)
                .context("native transfer exceeds 16 GiB")?;
            let executable = match params.get("executable") {
                None => false,
                Some(Value::Bool(value)) => *value,
                _ => anyhow::bail!("native executable policy must be a boolean"),
            };
            let declaration = Declaration {
                schema: 1,
                owner,
                path: format!("files/{path}"),
                sha256: sha256.into(),
                size,
                executable,
            };
            let id = transfer_id(&declaration)?;
            let directory = self.directory(&declaration)?;
            let key = digest(&declaration.path)?;
            let record = directory.join(format!("{key}.declaration.json"));
            let alias = directory.join(format!("{id}.transfer.json"));
            let bytes = serde_json::to_vec(&declaration)?;
            self.reserve(
                &directory,
                &[(&record, bytes.len() as u64), (&alias, bytes.len() as u64)],
            )?;
            for path in [&record, &alias] {
                if path.exists() {
                    ensure!(
                        self.read_record(path)? == declaration,
                        "native transfer declaration changed"
                    );
                } else {
                    self.publish(path, &bytes)?;
                }
            }
            return Ok(Transfer {
                declaration,
                transfer_id: id,
                action: Action::Begin,
            });
        }
        let id = text(params, "transfer_id")?;
        ensure!(hash(id), "invalid native transfer ID");
        let directory = self.root.join(digest(&owner)?);
        self.policy.verify(&directory, true)?;
        let declaration = self.read_record(&directory.join(format!("{id}.transfer.json")))?;
        ensure!(
            declaration.schema == 1
                && declaration.owner == owner
                && transfer_id(&declaration)? == id,
            "native transfer belongs to another controller fence"
        );
        ensure!(
            declaration.size <= MAX_TRANSFER && hash(&declaration.sha256),
            "invalid native transfer declaration"
        );
        let key = digest(&declaration.path)?;
        ensure!(
            self.read_record(&directory.join(format!("{key}.declaration.json")))? == declaration,
            "native transfer declaration changed"
        );
        let action = match method {
            "file.upload.status" => Action::Status,
            "file.upload.commit" => {
                let blob = directory.join(format!("{key}.data"));
                if !blob.exists() {
                    let partial = directory.join(format!("{key}.partial"));
                    if !partial.exists() && declaration.size == 0 {
                        self.reserve(&directory, &[(&partial, 0)])?;
                        self.publish(&partial, b"")?;
                    }
                    self.check_blob(&partial, &declaration)?;
                    let from = security::wide(partial.as_os_str())?;
                    let to = security::wide(blob.as_os_str())?;
                    security::check(unsafe {
                        MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH)
                    })?;
                }
                self.check_blob(&blob, &declaration)?;
                Action::Commit
            }
            "file.upload.write" => {
                let offset = params
                    .get("offset")
                    .and_then(Value::as_u64)
                    .context("invalid native transfer offset")?;
                let encoded = text(params, "data_base64")?;
                ensure!(
                    encoded.len() <= 4 * (CHUNK as usize).div_ceil(3),
                    "native transfer chunk exceeds 1 MiB"
                );
                let data = STANDARD.decode(encoded)?;
                let end = offset
                    .checked_add(data.len() as u64)
                    .context("native transfer offset overflow")?;
                ensure!(
                    !data.is_empty() && data.len() as u64 <= CHUNK && end <= declaration.size,
                    "invalid native transfer chunk range"
                );
                let blob = directory.join(format!("{key}.data"));
                let path = if blob.exists() {
                    blob
                } else {
                    directory.join(format!("{key}.partial"))
                };
                let complete = path
                    .extension()
                    .is_some_and(|extension| extension == "data");
                self.reserve(&directory, &[(&path, end)])?;
                if !path.exists() {
                    drop(self.policy.create_file(&path)?);
                }
                self.policy.verify(&path, false)?;
                let mut file = fs::OpenOptions::new()
                    .read(true)
                    .write(!complete)
                    .share_mode(0)
                    .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                    .open(&path)?;
                let mut info = BY_HANDLE_FILE_INFORMATION::default();
                security::check(unsafe {
                    GetFileInformationByHandle(file.as_raw_handle(), &mut info)
                })?;
                ensure!(
                    info.nNumberOfLinks == 1
                        && info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
                    "native transfer identity changed"
                );
                let current = file.metadata()?.len();
                ensure!(
                    offset <= current
                        && current <= declaration.size
                        && (!complete || current == declaration.size),
                    "native transfer chunks must be contiguous"
                );
                file.seek(SeekFrom::Start(offset))?;
                let repeated = data.len().min((current - offset) as usize);
                let mut prior = vec![0; repeated];
                file.read_exact(&mut prior)?;
                ensure!(
                    prior == data[..repeated],
                    "native duplicate chunk has different content"
                );
                if repeated < data.len() {
                    ensure!(!complete, "native committed input cannot grow");
                    file.write_all(&data[repeated..])?;
                    file.sync_all()?;
                }
                Action::Write { end }
            }
            _ => anyhow::bail!("unsupported native transfer operation"),
        };
        Ok(Transfer {
            declaration,
            transfer_id: id.into(),
            action,
        })
    }
    pub(super) fn complete_transfer(&self, transfer: &Transfer, result: &mut Value) -> Result<()> {
        ensure!(
            result.is_object() && result.get("native_input").is_none(),
            "worker cannot issue native transfer proof"
        );
        let declaration = &transfer.declaration;
        match transfer.action {
            Action::Begin => ensure!(
                result["transfer_id"] == transfer.transfer_id && result["chunk_bytes"] == CHUNK,
                "worker transfer identity differs"
            ),
            Action::Commit => self.finish(declaration, result)?,
            Action::Status | Action::Write { .. } => {
                let offset = result["offset"]
                    .as_u64()
                    .filter(|offset| *offset <= declaration.size)
                    .context("invalid worker transfer offset")?;
                let complete = result["complete"]
                    .as_bool()
                    .context("invalid worker transfer completion")?;
                ensure!(
                    result["transfer_id"] == transfer.transfer_id,
                    "worker transfer identity differs"
                );
                let directory = self.directory(declaration)?;
                let key = digest(&declaration.path)?;
                let blob = directory.join(format!("{key}.data"));
                let path = if blob.exists() {
                    blob
                } else {
                    directory.join(format!("{key}.partial"))
                };
                let available = if path.exists() {
                    self.policy.verify(&path, false)?;
                    fs::symlink_metadata(&path)?.len()
                } else {
                    0
                };
                ensure!(
                    available <= declaration.size,
                    "native transfer size changed"
                );
                let acknowledged = match transfer.action {
                    Action::Write { end } => {
                        ensure!(
                            offset >= end && available >= end,
                            "worker did not acknowledge the entire chunk"
                        );
                        end
                    }
                    _ => offset.min(available),
                };
                let admitted = complete
                    && acknowledged == declaration.size
                    && directory.join(format!("{key}.receipt.json")).exists();
                if complete {
                    let artifact = result
                        .get("artifact")
                        .context("completed worker transfer has no artifact")?;
                    ensure!(
                        artifact["path"] == declaration.path
                            && artifact["sha256"] == declaration.sha256
                            && artifact["size"] == declaration.size
                            && artifact.get("native_input").is_none(),
                        "worker transfer artifact differs"
                    );
                }
                result["offset"] = Value::from(acknowledged);
                result["complete"] = Value::from(admitted);
                if admitted {
                    self.finish(declaration, &mut result["artifact"])?;
                } else {
                    result.as_object_mut().unwrap().remove("artifact");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "inputs_transfers_tests.rs"]
mod tests;
