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
use runner::{LabWorker, LabWorkerRequest};
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

pub mod admin;
pub mod install;
pub mod media;
pub mod package;
pub mod runner;
use admin::{AdminFile, AdminRequest, LabAdmin};

pub const STATE_SCHEMA: u32 = 1;

/// Fixed host operations the lab may request. There is no generic run method.
#[async_trait]
pub trait VmHost: Send + Sync {
    async fn running(&self) -> Result<bool>;
    async fn snapshots(&self) -> Result<Vec<String>>;
    async fn snapshot(&self, name: &str) -> Result<()>;
    async fn revert(&self, name: &str) -> Result<()>;
    async fn start(&self) -> Result<()>;
    async fn delete_snapshot(&self, name: &str) -> Result<()>;
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
    pub snapshot: Option<String>,
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

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Lost,
    Cancelled,
    Unknown,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LabRun {
    pub run_id: String,
    pub lab_id: String,
    pub generation: u64,
    pub artifact: String,
    pub artifact_sha256: String,
    pub status: RunStatus,
    #[serde(default)]
    pub environment_id: Option<String>,
    #[serde(default)]
    pub installation_id: Option<String>,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    #[serde(default)]
    pub executable_path: Option<String>,
    pub settled: bool,
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LabInstall {
    pub install_id: String,
    pub lab_id: String,
    pub generation: u64,
    pub package_sha256: String,
    pub media_sha256: String,
    pub install_sha256: String,
    pub status: RunStatus,
    #[serde(default)]
    pub exit_code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    #[serde(default)]
    pub guest_media: Option<String>,
    #[serde(default)]
    pub guest_script: Option<String>,
    pub settled: bool,
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
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
    #[serde(default)]
    pub last_run: Option<LabRun>,
    #[serde(default)]
    pub last_install: Option<LabInstall>,
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
            last_run: None,
            last_install: None,
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
    worker: Option<Arc<dyn LabWorker>>,
    admin: Option<Arc<dyn LabAdmin>>,
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
            worker: None,
            admin: None,
            gate: Mutex::new(()),
        })
    }

    /// Attach the registered worker that executes packaged harnesses.
    pub fn with_worker(mut self, worker: Arc<dyn LabWorker>) -> Self {
        self.worker = Some(worker);
        self
    }

    /// Attach the administrative channel used only for installation media.
    pub fn with_admin(mut self, admin: Arc<dyn LabAdmin>) -> Self {
        self.admin = Some(admin);
        self
    }

    pub async fn finish(&self) {
        if let Some(worker) = &self.worker {
            worker.finish().await;
        }
        if let Some(admin) = &self.admin {
            admin.finish().await;
        }
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
        state.last_run = None;
        state.last_install = None;
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
            snapshot: Some(snapshot.clone()),
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
        if let Some(run) = state.last_run.as_mut() {
            ensure!(
                run.status != RunStatus::Running,
                "harness run is still in flight; wait for it or inspect the worker"
            );
            if run.status == RunStatus::Unknown {
                run.status = RunStatus::Lost;
                run.ended_at = Some(now());
                run.error = Some("baseline restore abandoned an unknown harness run".into());
                self.store.save(&state)?;
            }
        }
        if let Some(install) = state.last_install.as_mut() {
            ensure!(
                install.status != RunStatus::Running,
                "installation is still in flight; wait for it or inspect the VM"
            );
            if install.status == RunStatus::Unknown {
                install.status = RunStatus::Lost;
                install.ended_at = Some(now());
                install.error = Some("baseline restore abandoned an unknown installation".into());
                self.store.save(&state)?;
            }
        }
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
        let experiment_snapshot = active.snapshot.clone();
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
        let mut pruned_snapshot: Option<String> = None;
        let mut prune_error: Option<String> = None;
        if let Some(snapshot) = experiment_snapshot {
            match tokio::time::timeout(self.timeout, self.host.delete_snapshot(&snapshot)).await {
                Ok(Ok(())) => pruned_snapshot = Some(snapshot),
                Ok(Err(error)) => prune_error = Some(format!("{error:#}")),
                Err(_) => prune_error = Some("snapshot deletion timed out".into()),
            }
        }
        Ok(serde_json::json!({
            "vm_id": self.vm_id,
            "generation": next_generation,
            "phase": state.phase,
            "baseline_snapshot": self.baseline,
            "pruned_snapshot": pruned_snapshot,
            "prune_error": prune_error,
        }))
    }

    /// Execute one packaged, prebuilt user-mode EXE through the registered
    /// worker. The store lock is held for the whole run so another operation
    /// cannot restore the VM underneath the job.
    pub async fn run(
        &self,
        lab_id: &str,
        generation: u64,
        artifact: &str,
        stdin: Option<Vec<u8>>,
        timeout_secs: u64,
    ) -> Result<serde_json::Value> {
        ensure!(
            package::valid_artifact_name(artifact),
            "invalid packaged artifact name"
        );
        ensure!(
            (1..=600).contains(&timeout_secs),
            "harness timeout must be 1–600 seconds"
        );
        if let Some(stdin) = &stdin {
            ensure!(stdin.len() <= 1024 * 1024, "harness stdin exceeds 1 MiB");
        }
        let worker = self.worker.as_ref().cloned().context(
            "registered lab VM has no worker environment; set windows_lab.vms.<id>.environment",
        )?;
        let _gate = self.gate.lock().await;
        let _lock = self.store.lock(&self.vm_id)?;
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
        self.mark_restart_recovery(&mut state)?;
        ensure!(
            state.phase == LabPhase::Ready,
            "lab VM {} is {:?}, not ready",
            self.vm_id,
            state.phase
        );
        let active = state
            .active
            .as_ref()
            .context("lab has no active experiment")?
            .clone();
        ensure!(
            active.lab_id == lab_id && active.generation == generation,
            "stale lab lease: request generation {generation} is not the active generation {}",
            state.generation
        );
        ensure!(
            active.spec.launch == LaunchKind::Exe,
            "this phase runs only launch=exe harnesses"
        );
        ensure!(
            !matches!(
                state.last_run.as_ref().map(|run| run.status),
                Some(RunStatus::Running | RunStatus::Unknown)
            ) && !matches!(
                state.last_install.as_ref().map(|install| install.status),
                Some(RunStatus::Running | RunStatus::Unknown)
            ),
            "reconcile the previous harness run or installation (restore the baseline) before starting another"
        );
        let package_id = active
            .package_sha256
            .clone()
            .context("experiment has no verified offline package")?;
        let (executable, artifact_sha256) = self.load_artifact(&package_id, artifact)?;
        let run_id = uuid::Uuid::new_v4().simple().to_string();
        let mut run = LabRun {
            run_id: run_id.clone(),
            lab_id: lab_id.to_owned(),
            generation,
            artifact: artifact.to_owned(),
            artifact_sha256,
            status: RunStatus::Running,
            environment_id: None,
            installation_id: None,
            job_id: None,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            stdout_bytes: 0,
            stderr_bytes: 0,
            executable_path: None,
            settled: false,
            started_at: now(),
            ended_at: None,
            error: None,
        };
        state.last_run = Some(run.clone());
        self.store.save(&state)?;
        let outcome = worker
            .execute(LabWorkerRequest {
                run_id: run_id.clone(),
                owner: active.owner.clone(),
                artifact_name: artifact.to_owned(),
                executable,
                stdin,
                timeout_secs,
            })
            .await;
        match outcome {
            Ok(outcome) => {
                run.status = run_status(&outcome.state);
                run.settled = outcome.settled;
                run.environment_id = Some(outcome.environment_id);
                run.installation_id = Some(outcome.installation_id);
                run.job_id = Some(outcome.job_id);
                run.exit_code = outcome.exit_code;
                run.executable_path = outcome.executable_path;
                run.stdout = outcome.stdout;
                run.stderr = outcome.stderr;
                run.stdout_bytes = outcome.stdout_bytes;
                run.stderr_bytes = outcome.stderr_bytes;
            }
            Err(error) => {
                run.status = RunStatus::Unknown;
                run.error = Some(format!("{error:#}"));
            }
        }
        run.ended_at = Some(now());
        state.last_run = Some(run.clone());
        self.store.save(&state)?;
        let receipt = self.write_run_receipt(&run)?;
        if run.status == RunStatus::Unknown {
            bail!(
                "harness run {run_id} outcome is unknown; inspect the VM and restore the baseline (receipt {})",
                receipt.display()
            );
        }
        Ok(serde_json::json!({
            "vm_id": self.vm_id,
            "receipt": receipt,
            "run": run,
        }))
    }

    fn load_artifact(&self, package_id: &str, artifact: &str) -> Result<(Vec<u8>, String)> {
        let root = self.store.packages_root().join(package_id);
        let manifest: package::PackageManifest = serde_json::from_slice(
            &fs::read(root.join("manifest.json"))
                .with_context(|| format!("read package manifest {}", root.display()))?,
        )?;
        let entry = manifest
            .artifacts
            .iter()
            .find(|entry| entry.name == artifact)
            .with_context(|| format!("artifact {artifact} is not in package {package_id}"))?;
        let path = root.join(artifact);
        let bytes = fs::read(&path)
            .with_context(|| format!("read packaged artifact {}", path.display()))?;
        let hash = hex::encode(Sha256::digest(&bytes));
        ensure!(
            bytes.len() as u64 == entry.size && hash == entry.sha256,
            "packaged artifact {artifact} failed verification"
        );
        Ok((bytes, hash))
    }

    fn write_run_receipt(&self, run: &LabRun) -> Result<PathBuf> {
        let directory = self.store.root().join("runs");
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{}.json", run.run_id));
        if path.exists() {
            return Ok(path);
        }
        let mut bytes = serde_json::to_vec_pretty(run)?;
        bytes.push(b'\n');
        let mut staged = tempfile::NamedTempFile::new_in(&directory)?;
        staged.write_all(&bytes)?;
        staged.as_file().sync_all()?;
        staged
            .persist_noclobber(&path)
            .map_err(|error| error.error)
            .context("publish harness receipt")?;
        Ok(path)
    }

    /// Build and run the experiment's offline installation media through the
    /// administrative channel. The service/driver logic lives in the
    /// hash-pinned `install.ps1`; the controller only transports and records it.
    pub async fn install(
        &self,
        lab_id: &str,
        generation: u64,
        timeout_secs: u64,
    ) -> Result<serde_json::Value> {
        ensure!(
            (1..=600).contains(&timeout_secs),
            "installation timeout must be 1–600 seconds"
        );
        let admin = self.admin.as_ref().cloned().context(
            "administrative channel is unavailable; configure windows_lab.vms.<id>.admin and set its password environment variable",
        )?;
        let _gate = self.gate.lock().await;
        let _lock = self.store.lock(&self.vm_id)?;
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
        self.mark_restart_recovery(&mut state)?;
        ensure!(
            state.phase == LabPhase::Ready,
            "lab VM {} is {:?}, not ready",
            self.vm_id,
            state.phase
        );
        let active = state
            .active
            .as_ref()
            .context("lab has no active experiment")?
            .clone();
        ensure!(
            active.lab_id == lab_id && active.generation == generation,
            "stale lab lease: request generation {generation} is not the active generation {}",
            state.generation
        );
        ensure!(
            active.spec.install.is_some(),
            "experiment has no install plan"
        );
        ensure!(
            !matches!(
                state.last_run.as_ref().map(|run| run.status),
                Some(RunStatus::Running | RunStatus::Unknown)
            ) && !matches!(
                state.last_install.as_ref().map(|install| install.status),
                Some(RunStatus::Running | RunStatus::Unknown)
            ),
            "reconcile the previous harness run or installation (restore the baseline) first"
        );
        let package_id = active
            .package_sha256
            .clone()
            .context("experiment has no verified offline package")?;
        let install_sha256 = active
            .install_sha256
            .clone()
            .context("experiment has no install plan identity")?;
        let media = media::prepare(self.store.root(), &package_id)?;
        let install_id = uuid::Uuid::new_v4().simple().to_string();
        let mut record = LabInstall {
            install_id: install_id.clone(),
            lab_id: lab_id.to_owned(),
            generation,
            package_sha256: package_id,
            media_sha256: media.sha256.clone(),
            install_sha256,
            status: RunStatus::Running,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            guest_media: None,
            guest_script: None,
            settled: false,
            started_at: now(),
            ended_at: None,
            error: None,
        };
        state.last_install = Some(record.clone());
        self.store.save(&state)?;
        let outcome = admin
            .install(AdminRequest {
                install_id: install_id.clone(),
                media_sha256: media.sha256,
                files: media
                    .files
                    .iter()
                    .map(|file| AdminFile {
                        name: file.name.clone(),
                        path: file.path.clone(),
                    })
                    .collect(),
                timeout_secs,
            })
            .await;
        match outcome {
            Ok(outcome) => {
                record.status = if outcome.exit_code == Some(0) {
                    RunStatus::Completed
                } else {
                    RunStatus::Failed
                };
                record.exit_code = outcome.exit_code;
                record.stdout = outcome.stdout;
                record.stderr = outcome.stderr;
                record.guest_media = Some(outcome.guest_media);
                record.guest_script = Some(outcome.guest_script);
                record.settled = outcome.settled;
            }
            Err(error) => {
                record.status = RunStatus::Unknown;
                record.error = Some(format!("{error:#}"));
            }
        }
        record.ended_at = Some(now());
        state.last_install = Some(record.clone());
        self.store.save(&state)?;
        let receipt = self.write_install_receipt(&record)?;
        if record.status == RunStatus::Unknown {
            bail!(
                "installation {install_id} outcome is unknown; inspect the VM and restore the baseline (receipt {})",
                receipt.display()
            );
        }
        Ok(serde_json::json!({
            "vm_id": self.vm_id,
            "receipt": receipt,
            "install": record,
        }))
    }

    fn write_install_receipt(&self, record: &LabInstall) -> Result<PathBuf> {
        let directory = self.store.root().join("installs");
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{}.json", record.install_id));
        if path.exists() {
            return Ok(path);
        }
        let mut bytes = serde_json::to_vec_pretty(record)?;
        bytes.push(b'\n');
        let mut staged = tempfile::NamedTempFile::new_in(&directory)?;
        staged.write_all(&bytes)?;
        staged.as_file().sync_all()?;
        staged
            .persist_noclobber(&path)
            .map_err(|error| error.error)
            .context("publish installation receipt")?;
        Ok(path)
    }

    /// Delete unused experiment snapshots. The baseline and the active
    /// experiment snapshot are always kept.
    pub async fn prune(&self) -> Result<serde_json::Value> {
        let _gate = self.gate.lock().await;
        let _lock = self.store.lock(&self.vm_id)?;
        let state = self.store.load(&self.vm_id, &self.baseline)?;
        let active = state
            .active
            .as_ref()
            .and_then(|active| active.snapshot.clone());
        let names = tokio::time::timeout(self.timeout, self.host.snapshots())
            .await
            .map_err(|_| anyhow::anyhow!("snapshot listing timed out"))??;
        let prefix = format!("{}-", self.snapshot_prefix);
        let mut deleted = Vec::new();
        let mut failed = Vec::new();
        let mut kept = vec![self.baseline.clone()];
        if let Some(active) = &active {
            kept.push(active.clone());
        }
        for name in names {
            if !name.starts_with(&prefix) || name == self.baseline || Some(&name) == active.as_ref()
            {
                continue;
            }
            match tokio::time::timeout(self.timeout, self.host.delete_snapshot(&name)).await {
                Ok(Ok(())) => deleted.push(name),
                Ok(Err(error)) => {
                    failed.push(serde_json::json!({"snapshot":name,"error":format!("{error:#}")}))
                }
                Err(_) => failed.push(serde_json::json!({"snapshot":name,"error":"timed out"})),
            }
        }
        Ok(serde_json::json!({
            "vm_id": self.vm_id,
            "deleted": deleted,
            "failed": failed,
            "kept": kept,
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
                    snapshot: (pending.kind == OperationKind::Prepare)
                        .then(|| pending.snapshot.clone()),
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
        if state
            .last_run
            .as_ref()
            .is_some_and(|run| run.status == RunStatus::Running)
        {
            if let Some(run) = state.last_run.as_mut() {
                run.status = RunStatus::Unknown;
                run.ended_at = Some(now());
                run.error = Some(
                    "controller restarted during the harness run; restore the baseline to reconcile"
                        .into(),
                );
            }
            state.last_error = Some(
                "controller restarted during a harness run; inspect the worker and restore the baseline"
                    .into(),
            );
            self.store.save(state)?;
        }
        if state
            .last_install
            .as_ref()
            .is_some_and(|install| install.status == RunStatus::Running)
        {
            if let Some(install) = state.last_install.as_mut() {
                install.status = RunStatus::Unknown;
                install.ended_at = Some(now());
                install.error = Some(
                    "controller restarted during installation; restore the baseline to reconcile"
                        .into(),
                );
            }
            state.last_error = Some(
                "controller restarted during installation; inspect the VM and restore the baseline"
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

fn run_status(state: &str) -> RunStatus {
    match state {
        "completed" => RunStatus::Completed,
        "failed" => RunStatus::Failed,
        "timed_out" => RunStatus::TimedOut,
        "lost" => RunStatus::Lost,
        "cancelled" => RunStatus::Cancelled,
        _ => RunStatus::Unknown,
    }
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
