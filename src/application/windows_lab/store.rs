//! Atomic lab state and cross-process operation exclusion.
use super::*;

pub struct LabStore {
    pub(super) root: PathBuf,
    pub(super) configuration_sha256: Option<String>,
    pub(super) physical_id: Option<String>,
}

impl LabStore {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            root: state_dir.join("windows-lab"),
            configuration_sha256: None,
            physical_id: None,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn packages_root(&self) -> PathBuf {
        self.root.join("packages")
    }

    pub(super) fn state_path(&self, vm_id: &str) -> PathBuf {
        self.root.join(format!("{vm_id}.json"))
    }

    pub(super) fn ensure_root(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create lab state directory {}", self.root.display()))
    }

    /// Cross-process exclusion for one VM. The guard lives for the operation.
    pub(super) fn lock(&self, vm_id: &str) -> Result<File> {
        self.ensure_root()?;
        let path = self.root.join(format!(
            "{}.lock",
            self.physical_id.as_deref().unwrap_or(vm_id)
        ));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open lab lock {}", path.display()))?;
        file.try_lock_exclusive()
            .with_context(|| format!("VM {vm_id} already has an active lab operation"))?;
        if let Some(physical) = &self.physical_id {
            let identity_path = self.root.join(format!("{physical}.identity.json"));
            let identity =
                serde_json::json!({"vm_id":vm_id,"configuration_sha256":self.configuration_sha256});
            if identity_path.exists() {
                ensure!(
                    serde_json::from_slice::<serde_json::Value>(&fs::read(&identity_path)?)?
                        == identity,
                    "physical VM is already registered under another alias or configuration"
                );
            } else {
                let mut staged = tempfile::NamedTempFile::new_in(&self.root)?;
                staged.write_all(&serde_json::to_vec(&identity)?)?;
                staged.as_file().sync_all()?;
                staged
                    .persist_noclobber(identity_path)
                    .map_err(|e| e.error)?;
            }
        }
        Ok(file)
    }

    pub(super) fn load(&self, vm_id: &str, baseline: &str) -> Result<LabState> {
        let path = self.state_path(vm_id);
        let text = match fs::read(&path) {
            Ok(bytes) => String::from_utf8(bytes)
                .with_context(|| format!("lab state is not UTF-8: {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut state = LabState::fresh(vm_id, baseline);
                state.configuration_sha256 = self.configuration_sha256.clone().unwrap_or_default();
                return Ok(state);
            }
            Err(error) => {
                return Err(error).with_context(|| format!("read lab state {}", path.display()));
            }
        };
        let state: LabState = serde_json::from_str(&text)
            .with_context(|| format!("parse lab state {}", path.display()))?;
        ensure!(
            state.schema == STATE_SCHEMA,
            "unsupported lab state schema {} (expected {STATE_SCHEMA})",
            state.schema
        );
        ensure!(
            state.vm_id == vm_id,
            "lab state belongs to a different VM identity"
        );
        ensure!(
            state.baseline_snapshot == baseline,
            "lab state baseline snapshot changed; reconcile the previous baseline first"
        );
        if let Some(expected) = &self.configuration_sha256 {
            ensure!(
                &state.configuration_sha256 == expected,
                "lab VM configuration changed; reconcile the old physical VM before re-registering"
            );
        }
        ensure!(
            !state.updated_at.is_empty(),
            "lab state timestamp is missing"
        );
        if let Some(pending) = &state.pending {
            pending.validate_shape().with_context(|| {
                format!(
                    "lab state {} has an inconsistent pending operation",
                    path.display()
                )
            })?;
        }
        Ok(state)
    }

    pub(super) fn save(&self, state: &LabState) -> Result<()> {
        self.ensure_root()?;
        let mut state = state.clone();
        state.updated_at = now();
        let mut bytes = serde_json::to_vec_pretty(&state)?;
        bytes.push(b'\n');
        let mut staged = tempfile::NamedTempFile::new_in(&self.root)?;
        staged.write_all(&bytes)?;
        staged.as_file().sync_all()?;
        staged
            .persist(self.state_path(&state.vm_id))
            .map_err(|error| error.error)
            .context("publish lab state")?;
        #[cfg(unix)]
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }

    /// Rebind the persisted state and the physical-VM identity to the
    /// configuration identity a successful operation rewrote. `load` validates
    /// the state hash and `lock` validates the identity, so both records must
    /// move together; a stale copy would block every later command. Only
    /// `lab provision --register` reaches this, and only with the hash
    /// recomputed from the environment value read back from the configuration
    /// file; any other configuration change is still refused.
    pub(super) fn adopt_configuration(
        &self,
        state: &mut LabState,
        configuration_sha256: &str,
    ) -> Result<()> {
        let previous = std::mem::replace(
            &mut state.configuration_sha256,
            configuration_sha256.to_owned(),
        );
        let moved_identity = match &self.physical_id {
            Some(physical) => {
                let path = self.root.join(format!("{physical}.identity.json"));
                // The operation opened the lab with the previous configuration,
                // so a different identity file means the physical VM/alias was
                // changed outside this operation and must not be overwritten.
                if path.exists() {
                    let existing = serde_json::from_slice::<serde_json::Value>(&fs::read(&path)?)
                        .with_context(|| {
                        format!("parse physical VM identity {}", path.display())
                    })?;
                    ensure!(
                        existing
                            == serde_json::json!({
                                "vm_id": state.vm_id,
                                "configuration_sha256": previous,
                            }),
                        "physical VM is already registered under another alias or configuration"
                    );
                }
                Some((
                    path,
                    serde_json::json!({
                        "vm_id": state.vm_id,
                        "configuration_sha256": configuration_sha256,
                    }),
                ))
            }
            None => None,
        };
        if let Err(error) = self.save(state) {
            state.configuration_sha256 = previous;
            return Err(error.context("persist the registered lab configuration hash"));
        }
        if let Some((path, identity)) = moved_identity
            && let Err(error) = publish_identity(&path, &identity)
        {
            // Restore the state binding so the next command keeps reporting the
            // same configuration mismatch instead of trusting a half-published
            // identity.
            state.configuration_sha256 = previous;
            let _ = self.save(state);
            return Err(error.context("publish the registered physical-VM identity"));
        }
        Ok(())
    }
}

/// Publish a physical-VM identity record. The caller holds the lab operation
/// lock, so replacement is only ever the move this operation is authorized to
/// make.
fn publish_identity(path: &Path, identity: &serde_json::Value) -> Result<()> {
    let parent = path
        .parent()
        .context("physical VM identity path has no parent directory")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(&serde_json::to_vec(identity)?)?;
    staged.as_file().sync_all()?;
    staged
        .persist(path)
        .map_err(|error| error.error)
        .context("publish physical VM identity")?;
    Ok(())
}
