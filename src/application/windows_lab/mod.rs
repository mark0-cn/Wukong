//! Controller-owned Windows experiment lifecycle.
//!
//! A registered VMware VM runs at most one experiment at a time. Every prepare
//! or restore advances a persisted generation before the host operation starts,
//! so a lease from an older generation can never regain execution rights. Host
//! operations are fixed, bounded and addressed by operator configuration; model
//! input never supplies a VMX path or a VMware command. An interrupted or
//! timed-out operation stays `recovery_required` until an operator reconciles
//! it, and no state transition re-issues an uncertain host effect.
use crate::config::WindowsLabVm;
use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;

pub mod install;
pub mod package;

pub const STATE_SCHEMA: u32 = 1;

/// Fixed host operations the lab may request. There is no generic run method.
#[async_trait]
pub trait VmHost: Send + Sync {
    async fn running(&self) -> Result<bool>;
    async fn snapshots(&self) -> Result<Vec<String>>;
    async fn snapshot(&self, name: &str) -> Result<()>;
    async fn revert(&self, name: &str) -> Result<()>;
    async fn start(&self) -> Result<()>;
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LabPhase {
    Idle,
    Preparing,
    Ready,
    Restoring,
    RecoveryRequired,
    Failed,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Prepare,
    Restore,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaunchKind {
    Exe,
    DllHarness,
    Service,
    Gui,
    Driver,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputChannel {
    Argv,
    Stdin,
    File,
    Network,
    NamedPipe,
    Com,
    Gui,
    Ioctl,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountContext {
    User,
    ElevatedUser,
    System,
    Driver,
}

/// Bound to one experiment generation together with its target and toolchain
/// identities. A valid description is a precondition, not proof of execution.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExperimentSpec {
    pub target_sha256: String,
    pub launch: LaunchKind,
    pub input_channel: InputChannel,
    pub account: AccountContext,
    pub os_build: String,
    pub toolchain_sha256: String,
    #[serde(default)]
    pub recovery: String,
    /// Controller-fetched, hash-verified artifacts assembled into an offline package.
    #[serde(default)]
    pub dependencies: Vec<package::Dependency>,
    /// Guest-side installation and cleanup ownership. Validation only in Phase 1.
    #[serde(default)]
    pub install: Option<install::InstallPlan>,
}

impl ExperimentSpec {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            hex_digest(&self.target_sha256),
            "experiment target_sha256 must be 64 lowercase hex characters"
        );
        ensure!(
            hex_digest(&self.toolchain_sha256),
            "experiment toolchain_sha256 must be 64 lowercase hex characters"
        );
        ensure!(
            !self.os_build.trim().is_empty() && self.os_build.len() <= 128,
            "experiment os_build must be 1–128 characters"
        );
        ensure!(
            self.recovery.len() <= 512,
            "experiment recovery note exceeds 512 characters"
        );
        package::validate_dependencies(&self.dependencies)?;
        if let Some(install) = &self.install {
            install.validate(self.launch)?;
        }
        Ok(())
    }

    pub fn sha256(&self) -> Result<String> {
        self.validate()?;
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(self)?)))
    }
}

fn hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ActiveExperiment {
    pub lab_id: String,
    pub owner: String,
    pub purpose: String,
    pub generation: u64,
    pub spec_sha256: String,
    pub spec: ExperimentSpec,
    #[serde(default)]
    pub package_sha256: Option<String>,
    #[serde(default)]
    pub install_sha256: Option<String>,
    pub started_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PendingOperation {
    pub operation_id: String,
    pub kind: OperationKind,
    pub generation: u64,
    pub lab_id: String,
    pub owner: String,
    pub purpose: String,
    pub spec_sha256: String,
    pub spec: ExperimentSpec,
    #[serde(default)]
    pub package_sha256: Option<String>,
    #[serde(default)]
    pub install_sha256: Option<String>,
    pub snapshot: String,
    pub started_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LabState {
    pub schema: u32,
    pub vm_id: String,
    pub generation: u64,
    pub phase: LabPhase,
    pub baseline_snapshot: String,
    pub active: Option<ActiveExperiment>,
    pub pending: Option<PendingOperation>,
    pub last_error: Option<String>,
    pub updated_at: String,
}

impl LabState {
    fn fresh(vm_id: &str, baseline: &str) -> Self {
        Self {
            schema: STATE_SCHEMA,
            vm_id: vm_id.to_owned(),
            generation: 0,
            phase: LabPhase::Idle,
            baseline_snapshot: baseline.to_owned(),
            active: None,
            pending: None,
            last_error: None,
            updated_at: now(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LabLease {
    pub vm_id: String,
    pub lab_id: String,
    pub generation: u64,
    pub snapshot: String,
    pub spec_sha256: String,
    pub package_sha256: Option<String>,
    pub install_sha256: Option<String>,
    pub owner: String,
}

pub struct LabStore {
    root: PathBuf,
}

impl LabStore {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            root: state_dir.join("windows-lab"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn packages_root(&self) -> PathBuf {
        self.root.join("packages")
    }

    fn state_path(&self, vm_id: &str) -> PathBuf {
        self.root.join(format!("{vm_id}.json"))
    }

    fn ensure_root(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create lab state directory {}", self.root.display()))
    }

    /// Cross-process exclusion for one VM. The guard lives for the operation.
    fn lock(&self, vm_id: &str) -> Result<File> {
        self.ensure_root()?;
        let path = self.root.join(format!("{vm_id}.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open lab lock {}", path.display()))?;
        file.try_lock_exclusive()
            .with_context(|| format!("VM {vm_id} already has an active lab operation"))?;
        Ok(file)
    }

    fn load(&self, vm_id: &str, baseline: &str) -> Result<LabState> {
        let path = self.state_path(vm_id);
        let text = match fs::read(&path) {
            Ok(bytes) => String::from_utf8(bytes)
                .with_context(|| format!("lab state is not UTF-8: {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LabState::fresh(vm_id, baseline));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("read lab state {}", path.display()));
            }
        };
        let mut state: LabState = serde_json::from_str(&text)
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
        if state.updated_at.is_empty() {
            state.updated_at = now();
        }
        Ok(state)
    }

    fn save(&self, state: &LabState) -> Result<()> {
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
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

/// Materialize the spec's dependencies without touching lab state or the VM.
pub async fn materialize_dependencies(
    state_dir: &Path,
    spec: &ExperimentSpec,
) -> Result<Option<package::Package>> {
    spec.validate()?;
    if spec.dependencies.is_empty() {
        return Ok(None);
    }
    let store = LabStore::new(state_dir);
    package::materialize(&store.packages_root(), &spec.dependencies)
        .await
        .map(Some)
}

fn valid_label(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.contains(['\n', '\r', '\0'])
}

pub struct WindowsLab {
    vm_id: String,
    baseline: String,
    snapshot_prefix: String,
    timeout: Duration,
    start_after_restore: bool,
    store: LabStore,
    host: Arc<dyn VmHost>,
    gate: Mutex<()>,
}

impl WindowsLab {
    pub fn open(
        state_dir: &Path,
        vm_id: &str,
        config: &WindowsLabVm,
        host: Arc<dyn VmHost>,
    ) -> Result<Self> {
        crate::adapters::environments::validate_id(vm_id)?;
        ensure!(
            config.vmx.is_absolute() && config.vmrun.is_absolute(),
            "registered lab VM paths must be absolute"
        );
        Ok(Self {
            vm_id: vm_id.to_owned(),
            baseline: config.baseline_snapshot.clone(),
            snapshot_prefix: config.snapshot_prefix.clone(),
            timeout: Duration::from_secs(config.operation_timeout_seconds),
            start_after_restore: config.start_after_restore,
            store: LabStore::new(state_dir),
            host,
            gate: Mutex::new(()),
        })
    }

    pub async fn status(&self) -> Result<serde_json::Value> {
        let _gate = self.gate.lock().await;
        let _lock = self.store.lock(&self.vm_id)?;
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
        self.mark_restart_recovery(&mut state)?;
        let running = self.host.running().await;
        let snapshots = self.host.snapshots().await;
        Ok(serde_json::json!({
            "vm_id": self.vm_id,
            "state": state,
            "vm_running": running.as_ref().ok().copied(),
            "snapshots": snapshots.as_ref().ok(),
            "host_error": host_error(&running, &snapshots),
            "baseline_present": snapshots.as_ref().ok().map(|names| names.iter().any(|name| name == &self.baseline)),
            "state_dir": self.store.root(),
        }))
    }

    pub async fn prepare(
        &self,
        owner: &str,
        purpose: &str,
        spec: ExperimentSpec,
    ) -> Result<LabLease> {
        ensure!(valid_label(owner, 64), "lab owner must be 1–64 characters");
        ensure!(
            valid_label(purpose, 256),
            "lab purpose must be 1–256 characters"
        );
        spec.validate()?;
        let spec_sha256 = spec.sha256()?;
        let package = if spec.dependencies.is_empty() {
            None
        } else {
            Some(package::materialize(&self.store.packages_root(), &spec.dependencies).await?)
        };
        let package_sha256 = package.map(|package| package.id);
        let install_sha256 = spec
            .install
            .as_ref()
            .map(|install| install.sha256(spec.launch))
            .transpose()?;
        let _gate = self.gate.lock().await;
        let _lock = self.store.lock(&self.vm_id)?;
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
        self.mark_restart_recovery(&mut state)?;
        ensure!(
            matches!(state.phase, LabPhase::Idle | LabPhase::Failed),
            "lab VM {} is {:?}; restore or reconcile the previous experiment first",
            self.vm_id,
            state.phase
        );
        let generation = state.generation + 1;
        let lab_id = uuid::Uuid::new_v4().simple().to_string();
        let snapshot = format!("{}-{}", self.snapshot_prefix, lab_id);
        let pending = PendingOperation {
            operation_id: uuid::Uuid::new_v4().simple().to_string(),
            kind: OperationKind::Prepare,
            generation,
            lab_id: lab_id.clone(),
            owner: owner.to_owned(),
            purpose: purpose.to_owned(),
            spec_sha256: spec_sha256.clone(),
            spec: spec.clone(),
            package_sha256: package_sha256.clone(),
            install_sha256: install_sha256.clone(),
            snapshot: snapshot.clone(),
            started_at: now(),
        };
        state.generation = generation;
        state.phase = LabPhase::Preparing;
        state.pending = Some(pending.clone());
        state.active = None;
        state.last_error = None;
        self.store.save(&state)?;

        if let Err(failure) = self.ensure_baseline().await {
            return match failure {
                StepFailure::Timeout => self.mark_unknown(state, pending),
                StepFailure::Failed(error) => self.mark_failed(state, pending, error, false),
            };
        }
        if let Err(failure) = self.step(self.host.snapshot(&snapshot)).await {
            return match failure {
                StepFailure::Timeout => self.mark_unknown(state, pending),
                StepFailure::Failed(error) => self.mark_failed(state, pending, error, false),
            };
        }
        state.phase = LabPhase::Ready;
        state.active = Some(ActiveExperiment {
            lab_id: lab_id.clone(),
            owner: owner.to_owned(),
            purpose: purpose.to_owned(),
            generation,
            spec_sha256: spec_sha256.clone(),
            spec,
            package_sha256: package_sha256.clone(),
            install_sha256: install_sha256.clone(),
            started_at: now(),
        });
        state.pending = None;
        self.store.save(&state)?;
        Ok(LabLease {
            vm_id: self.vm_id.clone(),
            lab_id,
            generation,
            snapshot,
            spec_sha256,
            package_sha256,
            install_sha256,
            owner: owner.to_owned(),
        })
    }

    /// Fences the lease first: the generation advances and the previous active
    /// experiment is dropped before VMware is asked to revert.
    pub async fn restore(&self, lab_id: &str, generation: u64) -> Result<serde_json::Value> {
        let _gate = self.gate.lock().await;
        let _lock = self.store.lock(&self.vm_id)?;
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
        self.mark_restart_recovery(&mut state)?;
        let active = state
            .active
            .as_ref()
            .context("lab has no active experiment to restore")?;
        ensure!(
            state.phase == LabPhase::Ready,
            "lab VM {} is {:?}, not ready",
            self.vm_id,
            state.phase
        );
        ensure!(
            active.lab_id == lab_id && active.generation == generation,
            "stale lab lease: request generation {generation} is not the active generation {}",
            state.generation
        );
        let next_generation = state.generation + 1;
        let pending = PendingOperation {
            operation_id: uuid::Uuid::new_v4().simple().to_string(),
            kind: OperationKind::Restore,
            generation: next_generation,
            lab_id: active.lab_id.clone(),
            owner: active.owner.clone(),
            purpose: active.purpose.clone(),
            spec_sha256: active.spec_sha256.clone(),
            spec: active.spec.clone(),
            package_sha256: active.package_sha256.clone(),
            install_sha256: active.install_sha256.clone(),
            snapshot: state.baseline_snapshot.clone(),
            started_at: now(),
        };
        state.generation = next_generation;
        state.phase = LabPhase::Restoring;
        state.active = None;
        state.pending = Some(pending.clone());
        state.last_error = None;
        self.store.save(&state)?;

        if let Err(failure) = self.step(self.host.revert(&self.baseline)).await {
            return match failure {
                StepFailure::Timeout => self.mark_unknown(state, pending),
                StepFailure::Failed(error) => self.mark_failed(state, pending, error, true),
            };
        }
        if self.start_after_restore {
            match self.step(self.host.running()).await {
                Ok(true) => {}
                Ok(false) => {
                    if let Err(failure) = self.step(self.host.start()).await {
                        return match failure {
                            StepFailure::Timeout => self.mark_unknown(state, pending),
                            StepFailure::Failed(error) => {
                                self.mark_failed(state, pending, error, true)
                            }
                        };
                    }
                }
                Err(failure) => {
                    return match failure {
                        StepFailure::Timeout => self.mark_unknown(state, pending),
                        StepFailure::Failed(error) => self.mark_failed(state, pending, error, true),
                    };
                }
            }
        }
        state.phase = LabPhase::Idle;
        state.pending = None;
        state.last_error = None;
        self.store.save(&state)?;
        Ok(serde_json::json!({
            "vm_id": self.vm_id,
            "generation": next_generation,
            "phase": state.phase,
            "baseline_snapshot": self.baseline,
        }))
    }

    pub async fn reconcile(
        &self,
        operation_id: &str,
        outcome: ReconcileOutcome,
    ) -> Result<serde_json::Value> {
        let _gate = self.gate.lock().await;
        let _lock = self.store.lock(&self.vm_id)?;
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
        let pending = state
            .pending
            .clone()
            .context("lab has no pending operation to reconcile")?;
        ensure!(
            pending.operation_id == operation_id,
            "pending operation is {}, not {operation_id}",
            pending.operation_id
        );
        match outcome {
            ReconcileOutcome::Completed => {
                state.phase = match pending.kind {
                    OperationKind::Prepare => LabPhase::Ready,
                    OperationKind::Restore => LabPhase::Idle,
                };
                state.active = (pending.kind == OperationKind::Prepare).then(|| ActiveExperiment {
                    lab_id: pending.lab_id.clone(),
                    owner: pending.owner.clone(),
                    purpose: pending.purpose.clone(),
                    generation: pending.generation,
                    spec_sha256: pending.spec_sha256.clone(),
                    spec: pending.spec.clone(),
                    package_sha256: pending.package_sha256.clone(),
                    install_sha256: pending.install_sha256.clone(),
                    started_at: pending.started_at.clone(),
                });
                state.pending = None;
                state.last_error = None;
            }
            ReconcileOutcome::Failed => {
                state.phase = LabPhase::Failed;
                state.active = None;
                state.pending = None;
                state.last_error = Some(format!(
                    "operator recorded failure for {:?} operation {operation_id}",
                    pending.kind
                ));
            }
            ReconcileOutcome::Unknown => {
                state.phase = LabPhase::RecoveryRequired;
                state.last_error = Some(format!(
                    "operator kept {:?} operation {operation_id} unresolved",
                    pending.kind
                ));
            }
        }
        self.store.save(&state)?;
        Ok(serde_json::json!({"vm_id": self.vm_id, "state": state}))
    }

    async fn ensure_baseline(&self) -> std::result::Result<(), StepFailure> {
        let names = self.step(self.host.snapshots()).await?;
        if names.iter().any(|name| name == &self.baseline) {
            return Ok(());
        }
        self.step(self.host.snapshot(&self.baseline)).await?;
        Ok(())
    }

    async fn step<T>(
        &self,
        future: impl std::future::Future<Output = Result<T>>,
    ) -> std::result::Result<T, StepFailure> {
        match tokio::time::timeout(self.timeout, future).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(StepFailure::Failed(error)),
            Err(_) => Err(StepFailure::Timeout),
        }
    }

    /// A controller restart must not silently re-issue an unfinished effect.
    fn mark_restart_recovery(&self, state: &mut LabState) -> Result<()> {
        if state.pending.is_some()
            && matches!(state.phase, LabPhase::Preparing | LabPhase::Restoring)
        {
            state.phase = LabPhase::RecoveryRequired;
            state.last_error = Some(
                "controller restarted while a host operation was pending; inspect the VM and reconcile with its operation id"
                    .into(),
            );
            self.store.save(state)?;
        }
        Ok(())
    }

    fn mark_unknown<T>(&self, mut state: LabState, pending: PendingOperation) -> Result<T> {
        state.phase = LabPhase::RecoveryRequired;
        state.pending = Some(pending.clone());
        state.last_error = Some(format!(
            "{:?} operation {} timed out; inspect VM state before reconciling",
            pending.kind, pending.operation_id
        ));
        self.store.save(&state)?;
        bail!(
            "lab operation {} outcome is unknown after timeout; inspect the VM and reconcile (generation {})",
            pending.operation_id,
            pending.generation
        )
    }

    fn mark_failed<T>(
        &self,
        mut state: LabState,
        pending: PendingOperation,
        error: anyhow::Error,
        keep_pending: bool,
    ) -> Result<T> {
        if keep_pending {
            state.phase = LabPhase::RecoveryRequired;
            state.pending = Some(pending);
        } else {
            state.phase = LabPhase::Failed;
            state.pending = None;
        }
        state.active = None;
        state.last_error = Some(error.to_string());
        self.store.save(&state)?;
        Err(error)
    }
}

enum StepFailure {
    Failed(anyhow::Error),
    Timeout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconcileOutcome {
    Completed,
    Failed,
    Unknown,
}

fn host_error(running: &Result<bool>, snapshots: &Result<Vec<String>>) -> Option<String> {
    let mut messages = Vec::new();
    if let Err(error) = running {
        messages.push(format!("running: {error:#}"));
    }
    if let Err(error) = snapshots {
        messages.push(format!("snapshots: {error:#}"));
    }
    (!messages.is_empty()).then(|| messages.join("; "))
}

#[cfg(test)]
mod tests;
