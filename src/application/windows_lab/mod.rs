//! Controller-owned Windows experiment lifecycle.
//!
//! A registered VMware VM runs at most one experiment at a time. Every prepare
//! or restore advances a persisted generation before the host operation starts,
//! so a lease from an older generation can never regain execution rights. Host
//! operations are fixed, bounded and addressed by operator configuration; model
//! input never supplies a VMX path or a VMware command. An interrupted or
//! timed-out operation stays `recovery_required` until an operator reconciles
//! it, and no state transition re-issues an uncertain host effect.
use crate::adapters::environments::EnvironmentConfig;
use crate::config::WindowsLabVm;
use anyhow::{Context, Result, ensure};
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
mod execution;
pub mod install;
pub mod media;
pub mod package;
pub mod provision;
pub mod provision_environment;
mod provision_service;
pub mod runner;
pub mod service;
use admin::{AdminFile, AdminRequest, LabAdmin};
use provision::{
    GuestNetwork, IsoBuilder, MediaFinalizer, OfflineDisk, ProvisionEnvironment,
    RegisteredEnvironment,
};
pub use provision_service::{
    CleanGuestOptions, CleanGuestOutcome, ProvisionOptions, ProvisionOutcome,
};

pub const STATE_SCHEMA: u32 = 2;

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

mod types;
pub use types::*;

mod store;
pub use store::LabStore;

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

/// Canonical configuration identity of one registered lab: the physical VM, the
/// operator `[windows_lab.vms.<alias>]` entry and the mapped
/// `[environments.<id>]` entry. `open` refuses a persisted state that records a
/// different identity, so only the operation that legitimately rewrites the
/// mapped environment (`lab provision --register`) may persist a recomputed
/// hash. Both hash creators share this function so the adoption writes exactly
/// the value the next command computes.
fn configuration_sha256(
    physical_id: &str,
    config: &WindowsLabVm,
    environment: Option<&EnvironmentConfig>,
) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(
        &serde_json::json!({
            "physical_vm": physical_id,
            "config": config,
            "environment": environment,
        }),
    )?)))
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
    offline: Option<Arc<dyn OfflineDisk>>,
    iso_builder: Option<Arc<dyn IsoBuilder>>,
    guest_network: Option<Arc<dyn GuestNetwork>>,
    media_finalizer: Option<Arc<dyn MediaFinalizer>>,
    environment_registrar: Option<Arc<dyn ProvisionEnvironment>>,
    gate: Mutex<()>,
    vmx: PathBuf,
    caller: Option<String>,
    /// Operator configuration this lab was opened with, retained verbatim so a
    /// successful `--register` can persist the guard hash the next command
    /// recomputes from the rewritten configuration file.
    vm_config: WindowsLabVm,
}

impl WindowsLab {
    pub fn open(
        state_dir: &Path,
        vm_id: &str,
        config: &WindowsLabVm,
        host: Arc<dyn VmHost>,
    ) -> Result<Self> {
        Self::open_with_environment(state_dir, vm_id, config, None, host)
    }

    pub(crate) fn open_with_environment(
        state_dir: &Path,
        vm_id: &str,
        config: &WindowsLabVm,
        environment: Option<&EnvironmentConfig>,
        host: Arc<dyn VmHost>,
    ) -> Result<Self> {
        crate::adapters::environments::validate_id(vm_id)?;
        ensure!(
            config.vmx.is_absolute() && config.vmrun.is_absolute(),
            "registered lab VM paths must be absolute"
        );
        let physical_id = physical_identity(&config.vmx)?;
        let mut store = LabStore::new(state_dir);
        store.physical_id = Some(physical_id.clone());
        store.configuration_sha256 = Some(configuration_sha256(&physical_id, config, environment)?);
        // Read validates the persisted configuration before opening any capability.
        store.load(vm_id, &config.baseline_snapshot)?;
        Ok(Self {
            vm_id: vm_id.to_owned(),
            baseline: config.baseline_snapshot.clone(),
            snapshot_prefix: config.snapshot_prefix.clone(),
            timeout: Duration::from_secs(config.operation_timeout_seconds),
            start_after_restore: config.start_after_restore,
            store,
            host,
            worker: None,
            admin: None,
            offline: None,
            iso_builder: None,
            guest_network: None,
            media_finalizer: None,
            environment_registrar: None,
            gate: Mutex::new(()),
            vmx: config.vmx.clone(),
            caller: None,
            vm_config: config.clone(),
        })
    }

    pub fn with_caller(mut self, caller: String) -> Self {
        self.caller = Some(caller);
        self
    }

    /// Reject stale or foreign requests before observation or lease release.
    pub fn validate_lease(&self, lab_id: &str, generation: u64) -> Result<()> {
        let state = self.store.load(&self.vm_id, &self.baseline)?;
        self.validate_active(&state, lab_id, generation)
    }

    fn validate_active(&self, state: &LabState, lab_id: &str, generation: u64) -> Result<()> {
        let active = state
            .active
            .as_ref()
            .context("lab has no active experiment")?;
        ensure!(state.phase == LabPhase::Ready, "lab is not ready");
        self.authorize(active)?;
        ensure!(
            active.lab_id == lab_id && active.generation == generation,
            "stale lab lease"
        );
        Ok(())
    }

    fn authorize(&self, active: &ActiveExperiment) -> Result<()> {
        ensure!(
            self.caller
                .as_ref()
                .is_none_or(|caller| caller == &active.owner),
            "lab lease belongs to another assessment agent"
        );
        Ok(())
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

    /// Attach the offline disk-injection adapter for `lab provision`.
    pub fn with_offline(mut self, offline: Arc<dyn OfflineDisk>) -> Self {
        self.offline = Some(offline);
        self
    }

    /// Attach the ISO builder used by `lab provision`.
    pub fn with_iso_builder(mut self, builder: Arc<dyn IsoBuilder>) -> Self {
        self.iso_builder = Some(builder);
        self
    }

    /// Attach hypervisor guest-address discovery used for environment registration.
    pub fn with_guest_network(mut self, network: Arc<dyn GuestNetwork>) -> Self {
        self.guest_network = Some(network);
        self
    }

    /// Attach the repository media finalize entry point.
    pub fn with_media_finalizer(mut self, finalizer: Arc<dyn MediaFinalizer>) -> Self {
        self.media_finalizer = Some(finalizer);
        self
    }

    /// Attach the controller environment registration/doctor hook. The CLI
    /// supplies it only for `--register`/`--verify-environment`.
    pub fn with_environment_registrar(mut self, registrar: Arc<dyn ProvisionEnvironment>) -> Self {
        self.environment_registrar = Some(registrar);
        self
    }

    /// Adopt the configuration identity written by this operation's successful
    /// environment registration. Registration rewrites the mapped
    /// `[environments.<id>]` entry, which changes the guard hash `open` compares
    /// against, so the persisted state and physical-VM identity must be rebound
    /// in the same commit. Only the mapped entry may move: a registration that
    /// writes an environment this lab does not map leaves the guard untouched,
    /// and any change this operation did not write keeps being refused by
    /// `LabStore::load`/`lock`.
    pub(super) fn adopt_registered_configuration(
        &self,
        state: &mut LabState,
        registered: &RegisteredEnvironment,
    ) -> Result<()> {
        if self.vm_config.environment.as_deref() != Some(registered.environment_id.as_str()) {
            return Ok(());
        }
        let physical_id = self
            .store
            .physical_id
            .as_deref()
            .context("registered lab VM has no physical identity")?;
        let configuration_sha256 =
            configuration_sha256(physical_id, &self.vm_config, Some(&registered.environment))?;
        self.store
            .adopt_configuration(state, &configuration_sha256)
            .context("adopt the registered lab environment")
    }

    pub async fn status(&self) -> Result<serde_json::Value> {
        let state = self.store.load(&self.vm_id, &self.baseline)?;
        let running = tokio::time::timeout(Duration::from_secs(5), self.host.running())
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("VM status probe timed out")));
        let snapshots = tokio::time::timeout(Duration::from_secs(5), self.host.snapshots())
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("VM snapshot probe timed out")));
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
        ensure!(
            self.caller.as_ref().is_none_or(|caller| caller == owner),
            "owner must match the admitted caller"
        );
        ensure!(valid_label(owner, 256), "lab owner must be 1–64 characters");
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
        let _environment_guard = crate::adapters::environments::lab_guard::acquire(
            &crate::adapters::environments::lab_guard::path(&self.vmx)?,
            crate::adapters::environments::lab_guard::LockMode::Exclusive,
        )?;
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
        self.mark_restart_recovery(&mut state)?;
        ensure!(
            matches!(state.phase, LabPhase::Idle | LabPhase::Failed),
            "lab VM {} is {:?}; restore or reconcile the previous experiment first",
            self.vm_id,
            state.phase
        );
        ensure!(
            state
                .last_provision
                .as_ref()
                .is_none_or(|provision| !matches!(
                    provision.status,
                    RunStatus::Running | RunStatus::Unknown
                )),
            "offline provisioning state is uncertain; re-provision the VM and reconcile it before preparing an experiment"
        );
        let maintenance = if let Some(worker) = &self.worker
            && self.host.running().await?
        {
            Some(worker.maintenance().await?)
        } else {
            None
        };
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
            spec: Some(spec.clone()),
            provision: None,
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
        if let Some(maintenance) = maintenance {
            maintenance.finish().await?;
        }
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
        let _environment_guard = crate::adapters::environments::lab_guard::acquire(
            &crate::adapters::environments::lab_guard::path(&self.vmx)?,
            crate::adapters::environments::lab_guard::LockMode::Exclusive,
        )?;
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
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
        self.authorize(active)?;
        self.mark_restart_recovery(&mut state)?;
        let active = state.active.as_ref().context("active experiment missing")?;
        ensure!(
            state
                .last_run
                .as_ref()
                .is_none_or(|r| r.status != RunStatus::Running)
                && state
                    .last_install
                    .as_ref()
                    .is_none_or(|r| r.status != RunStatus::Running),
            "an operation is still running; reconcile it before restoring"
        );
        self.authorize(active)?;
        let maintenance = if let Some(worker) = &self.worker
            && self.host.running().await?
        {
            Some(worker.maintenance().await?)
        } else {
            None
        };
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
            spec: Some(active.spec.clone()),
            provision: None,
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
        if let Some(run) = state.last_run.as_mut()
            && run.status == RunStatus::Unknown
        {
            run.status = RunStatus::Lost;
            run.ended_at = Some(now());
            run.error = Some("unknown harness abandoned after successful baseline restore".into());
        }
        if let Some(install) = state.last_install.as_mut()
            && install.status == RunStatus::Unknown
        {
            install.status = RunStatus::Lost;
            install.ended_at = Some(now());
            install.error =
                Some("unknown installation abandoned after successful baseline restore".into());
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
        if let Some(maintenance) = maintenance {
            maintenance.finish().await?;
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

    /// Delete unused experiment snapshots. The baseline and the active
    /// experiment snapshot are always kept.
    pub async fn prune(&self) -> Result<serde_json::Value> {
        let _gate = self.gate.lock().await;
        let _lock = self.store.lock(&self.vm_id)?;
        let _environment_guard = crate::adapters::environments::lab_guard::acquire(
            &crate::adapters::environments::lab_guard::path(&self.vmx)?,
            crate::adapters::environments::lab_guard::LockMode::Exclusive,
        )?;
        let state = self.store.load(&self.vm_id, &self.baseline)?;
        ensure!(
            state.pending.is_none() && matches!(state.phase, LabPhase::Idle | LabPhase::Ready),
            "cannot prune while a host operation needs recovery"
        );
        if let Some(active) = &state.active {
            self.authorize(active)?;
        }
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
        let _environment_guard = crate::adapters::environments::lab_guard::acquire(
            &crate::adapters::environments::lab_guard::path(&self.vmx)?,
            crate::adapters::environments::lab_guard::LockMode::Exclusive,
        )?;
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
            ReconcileOutcome::Completed => match pending.kind {
                OperationKind::Prepare => {
                    state.phase = LabPhase::Ready;
                    state.active = Some(ActiveExperiment {
                        lab_id: pending.lab_id.clone(),
                        owner: pending.owner.clone(),
                        purpose: pending.purpose.clone(),
                        generation: pending.generation,
                        spec_sha256: pending.spec_sha256.clone(),
                        spec: pending
                            .spec
                            .clone()
                            .context("pending prepare operation lost its spec")?,
                        snapshot: Some(pending.snapshot.clone()),
                        package_sha256: pending.package_sha256.clone(),
                        install_sha256: pending.install_sha256.clone(),
                        started_at: pending.started_at.clone(),
                    });
                    state.pending = None;
                    state.last_error = None;
                }
                OperationKind::Restore => {
                    state.phase = LabPhase::Idle;
                    state.active = None;
                    state.pending = None;
                    state.last_error = None;
                }
                OperationKind::Provision => {
                    self.cleanup_provision_artifacts(&pending).await?;
                    state.phase = LabPhase::Idle;
                    state.active = None;
                    state.pending = None;
                    state.last_error = None;
                    if let Some(record) = state.last_provision.as_mut() {
                        record.status = RunStatus::Completed;
                        record.settled = true;
                        record.ended_at = record.ended_at.clone().or_else(|| Some(now()));
                        record.error = None;
                    }
                }
            },
            ReconcileOutcome::Failed => {
                if pending.kind == OperationKind::Provision {
                    self.cleanup_provision_artifacts(&pending).await.context(
                        "restore the VMX and remove the generated ISO before recording failure",
                    )?;
                }
                state.phase = LabPhase::Failed;
                state.active = None;
                state.pending = None;
                state.last_error = Some(format!(
                    "operator recorded failure for {:?} operation {operation_id}",
                    pending.kind
                ));
                if pending.kind == OperationKind::Provision
                    && let Some(record) = state.last_provision.as_mut()
                {
                    record.status = RunStatus::Failed;
                    record.settled = true;
                    record.ended_at = Some(now());
                    record.error = Some(format!(
                        "operator recorded failure for provisioning operation {operation_id}"
                    ));
                }
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
            Ok(Err(error)) if error.is::<crate::adapters::windows_lab::VmwareTimeout>() => {
                Err(StepFailure::Timeout)
            }
            Ok(Err(error)) => Err(StepFailure::Failed(error)),
            Err(_) => Err(StepFailure::Timeout),
        }
    }

    /// A controller restart must not silently re-issue an unfinished effect.
    fn mark_restart_recovery(&self, state: &mut LabState) -> Result<()> {
        if state.pending.is_some()
            && matches!(
                state.phase,
                LabPhase::Preparing | LabPhase::Restoring | LabPhase::Provisioning
            )
        {
            state.phase = LabPhase::RecoveryRequired;
            state.last_error = Some(
                "controller restarted while a host operation was pending; inspect the VM and reconcile with its operation id"
                    .into(),
            );
            self.store.save(state)?;
        }
        if state
            .last_provision
            .as_ref()
            .is_some_and(|provision| provision.status == RunStatus::Running)
            && state.pending.is_none()
        {
            if let Some(provision) = state.last_provision.as_mut() {
                provision.status = RunStatus::Unknown;
                provision.ended_at = Some(now());
                provision.error = Some(
                    "controller restarted during offline provisioning; re-provision the VM before reuse"
                        .into(),
                );
            }
            state.last_error = Some(
                "controller restarted during offline provisioning; inspect the powered-off VM, remove guest leftovers with `wukong lab clean-guest --vm <alias> --operation <operation_id>` before retrying"
                    .into(),
            );
            self.store.save(state)?;
        }
        if state
            .last_run
            .as_ref()
            .is_some_and(|run| run.status == RunStatus::Running && run.job_id.is_none())
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
        Err(LabError {
            code: "lab_operation_unknown",
            operation_id: pending.operation_id,
            generation: pending.generation,
            receipt: None,
            message: "inspect the VM and reconcile the existing operation".into(),
        }
        .into())
    }

    fn mark_failed<T>(
        &self,
        mut state: LabState,
        pending: PendingOperation,
        error: anyhow::Error,
        keep_pending: bool,
    ) -> Result<T> {
        let operation_id = pending.operation_id.clone();
        let generation = pending.generation;
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
        if keep_pending {
            Err(LabError {
                code: "lab_operation_unknown",
                operation_id,
                generation,
                receipt: None,
                message: format!(
                    "host operation failed after intent: {error:#}; inspect and reconcile"
                ),
            }
            .into())
        } else {
            Err(error)
        }
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
        "running" | "queued" => RunStatus::Running,
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

pub use crate::adapters::environments::lab_guard::physical_identity;

#[derive(Debug, Serialize)]
pub struct LabError {
    pub code: &'static str,
    pub operation_id: String,
    pub generation: u64,
    pub receipt: Option<PathBuf>,
    pub message: String,
}
impl std::fmt::Display for LabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} (operation {}, generation {})",
            self.code, self.message, self.operation_id, self.generation
        )
    }
}
impl std::error::Error for LabError {}

/// Ordinary worker starts share the persisted maintenance fence. Observation
/// and cleanup remain available to reconcile resources after an interruption.
pub(crate) fn check_execution_admission(
    state_dir: &Path,
    labs: &crate::config::WindowsLabConfig,
    environment: Option<&str>,
) -> Result<()> {
    for (id, vm) in &labs.vms {
        if vm.environment.as_deref() != environment || environment.is_none() {
            continue;
        }
        let state = LabStore::new(state_dir).load(id, &vm.baseline_snapshot)?;
        ensure!(
            state.pending.is_none()
                && !matches!(
                    state.phase,
                    LabPhase::Preparing
                        | LabPhase::Restoring
                        | LabPhase::Provisioning
                        | LabPhase::RecoveryRequired
                )
                && state
                    .last_run
                    .as_ref()
                    .is_none_or(|run| run.status != RunStatus::Unknown)
                && state.last_install.as_ref().is_none_or(|install| !matches!(
                    install.status,
                    RunStatus::Running | RunStatus::Unknown
                ))
                && state
                    .last_provision
                    .as_ref()
                    .is_none_or(|provision| !matches!(
                        provision.status,
                        RunStatus::Running | RunStatus::Unknown
                    )),
            "lab VM has an unresolved maintenance/execution effect; observe and reconcile it before starting new work"
        );
    }
    Ok(())
}
