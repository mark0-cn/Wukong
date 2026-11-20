//! `lab provision`: offline worker installation on a powered-off lab VM.
//!
//! The durable intent is persisted before the first mutation. Any failure after
//! that point becomes `recovery_required`; `lab reconcile` then restores the VMX,
//! removes the generated ISO/media and resolves the operation before the lab is
//! usable again. The guest boot mechanism is a fixed Windows Task Scheduler
//! boot-trigger task that runs the wrapper as SYSTEM without a login: unlike a
//! local Group Policy startup script it does not depend on GP applicability
//! (that mechanism was filtered out as "empty" on every real boot), and unlike a
//! one-shot service it does not need to answer the SCM (7009/7000).
use super::provision::{
    BootTaskInjection, GuestDisk, GuestFile, GuestPaths, IsoBuilder, ProvisionEnvironment,
    RegisteredEnvironment, SnapshotPlan, VmxEdit, boot_task_injection, plan_snapshot,
    provision_paths,
};
use super::{
    InjectedFile, LabPhase, LabProvision, MediaIdentity, ProvisionSource, RunStatus, WindowsLab,
    now,
};

mod execution;
mod planning;
mod recovery;

use crate::application::windows_lab::provision::ProvisionArtifactPaths;
use anyhow::{Context, Result, ensure};
use execution::{step_error, write_synced};
use planning::planned_actions;
use recovery::remove_provision_working_files;
use serde::Serialize;
use serde_json::Value;
use std::{path::PathBuf, sync::Arc, time::Duration};
use wukong_worker_package::ConnectionReceipt;

pub struct ProvisionOptions {
    pub source: ProvisionSource,
    /// One-command override for the configured `mkisofs` executable.
    pub iso_tool: Option<PathBuf>,
    pub replace_baseline: bool,
    pub register: bool,
    pub verify_environment: bool,
    pub dry_run: bool,
    pub timeout_seconds: u64,
}

/// Explicit operator cleanup of one failed attempt's leftovers on the
/// powered-off guest disk. It never mutates controller lab state or the VMX.
pub struct CleanGuestOptions {
    pub operation_id: String,
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
pub struct ProvisionOutcome {
    pub dry_run: bool,
    pub planned_actions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provision: Option<LabProvision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_block: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct CleanGuestOutcome {
    pub dry_run: bool,
    pub vm_id: String,
    pub operation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest_disk: Option<PathBuf>,
    pub actions: Vec<String>,
    pub files_written: usize,
    pub files_removed: Vec<String>,
    pub directories_removed: Vec<String>,
    pub boot_task_removed: bool,
    pub service_key_removed: Option<String>,
}

/// Durable facts for one provisioning attempt that do not depend on the guest
/// disk. They are persisted before the configured baseline is replaced, because
/// the committed boot disk can only be resolved after that deletion.
struct ProvisionIntent {
    provision_id: String,
    paths: ProvisionArtifactPaths,
    guest: GuestPaths,
    media: MediaIdentity,
    source: ProvisionSource,
    identity_file: Option<PathBuf>,
    guest_files: Vec<GuestFile>,
    injected: Vec<InjectedFile>,
}

/// Guest-disk facts established only once the powered-off disk is addressable.
struct ProvisionDiskPlan {
    guest_disk: GuestDisk,
    vmx_edit: VmxEdit,
}

struct PlannedProvision {
    provision_id: String,
    paths: ProvisionArtifactPaths,
    guest: GuestPaths,
    media: MediaIdentity,
    source: ProvisionSource,
    identity_file: Option<PathBuf>,
    guest_disk: GuestDisk,
    vmx_bytes: Vec<u8>,
    vmx_edit: VmxEdit,
    guest_files: Vec<GuestFile>,
    boot_task: BootTaskInjection,
    baseline: String,
    iso_builder: Arc<dyn IsoBuilder>,
    timeout: Duration,
}

impl PlannedProvision {
    #[allow(
        clippy::too_many_arguments,
        reason = "a provisioning plan binds every fixed identity at once"
    )]
    fn assemble(
        intent: ProvisionIntent,
        vmx_bytes: Vec<u8>,
        disk: ProvisionDiskPlan,
        boot_task: BootTaskInjection,
        baseline: String,
        iso_builder: Arc<dyn IsoBuilder>,
        timeout: Duration,
    ) -> Self {
        Self {
            provision_id: intent.provision_id,
            paths: intent.paths,
            guest: intent.guest,
            media: intent.media,
            source: intent.source,
            identity_file: intent.identity_file,
            guest_disk: disk.guest_disk,
            vmx_bytes,
            vmx_edit: disk.vmx_edit,
            guest_files: intent.guest_files,
            boot_task,
            baseline,
            iso_builder,
            timeout,
        }
    }
}

struct ProvisionEvidence {
    iso_sha256: String,
    iso_size: u64,
    vmx_sha256_after: String,
    hive_sha256_before: String,
    hive_sha256_after: String,
    boot_task_sha256: String,
    launcher_sha256: String,
    guest_receipt: Value,
    connection: ConnectionReceipt,
}

struct EnvironmentFinish {
    registrar: Arc<dyn ProvisionEnvironment>,
    environment_id: String,
    connection: ConnectionReceipt,
    identity_file: Option<PathBuf>,
    register: bool,
    verify: bool,
    timeout: Duration,
}

/// Outcome of the registration/verification phase. `registered` is set as soon
/// as the configuration rewrite succeeded, even if the doctor or the baseline
/// revert then fail: the lab must adopt the rewritten configuration identity, or
/// those later failures would strand every command on a stale hash. `failure`
/// is the error the caller reports after restoring the baseline.
struct EnvironmentFinished {
    registration: Option<Value>,
    verification: Option<Value>,
    registered: Option<RegisteredEnvironment>,
    failure: Option<anyhow::Error>,
}

impl WindowsLab {
    pub async fn provision(&self, options: ProvisionOptions) -> Result<ProvisionOutcome> {
        ensure!(
            (60..=7200).contains(&options.timeout_seconds),
            "provisioning timeout must be 60–7200 seconds"
        );
        ensure!(
            (!options.register && !options.verify_environment)
                || self.environment_registrar.is_some(),
            "--register/--verify-environment require the controller environment registrar"
        );
        ensure!(
            options
                .iso_tool
                .as_ref()
                .is_none_or(|path| path.is_absolute()),
            "--iso-tool must be an absolute path"
        );
        let registrar = self.environment_registrar.clone();
        let offline = self.offline.clone().context(
            "offline provisioning tooling is not configured; add [windows_lab.vms.<alias>.offline] with wsl_executable, wsl_distribution, guestfish, hivexsh, mkisofs and timeout_seconds",
        )?;
        let iso_builder = self
            .iso_builder
            .clone()
            .context("offline provisioning ISO builder is unavailable")?;
        let guest_network = self.guest_network.clone();
        let finalizer = self.media_finalizer.clone();
        let _gate = self.gate.lock().await;
        let _lock = if options.dry_run {
            self.store.lock_preview(&self.vm_id)?
        } else {
            self.store.lock(&self.vm_id)?
        };
        let _environment_guard = crate::adapters::environments::lab_guard::acquire(
            &crate::adapters::environments::lab_guard::path(&self.vmx)?,
            crate::adapters::environments::lab_guard::LockMode::Exclusive,
        )?;
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
        if !options.dry_run {
            self.mark_restart_recovery(&mut state)?;
        }
        ensure!(
            matches!(state.phase, LabPhase::Idle | LabPhase::Failed),
            "lab VM {} is {:?}; restore or reconcile the previous experiment before provisioning",
            self.vm_id,
            state.phase
        );
        ensure!(
            state.pending.is_none(),
            "lab VM has a pending operation; reconcile it before provisioning"
        );
        ensure!(
            state.active.is_none(),
            "lab VM has an active experiment; restore it before provisioning"
        );
        let running = self
            .host_running()
            .await
            .context("inspect lab VM power state before provisioning")?;
        ensure!(
            !running,
            "lab VM must be powered off for offline provisioning"
        );
        let snapshots = self.step(self.host.snapshots()).await.map_err(step_error)?;
        let snapshot_plan = plan_snapshot(&snapshots, &self.baseline, options.replace_baseline)?;
        if options.replace_baseline && snapshot_plan == SnapshotPlan::Pristine {
            tracing::info!(
                vm = %self.vm_id,
                baseline = %self.baseline,
                "--replace-baseline requested but no snapshot exists; the flag is a no-op and the baseline is created after provisioning"
            );
        }
        let provision_id = uuid::Uuid::new_v4().simple().to_string();
        let paths = provision_paths(self.store.root(), &provision_id)?;
        let guest = GuestPaths::for_provision(&provision_id)?;
        if options.dry_run {
            return self
                .dry_run_plan(&options, paths, guest, snapshot_plan, iso_builder, offline)
                .await;
        }
        let (media, source, identity_file) = self
            .resolve_media(&options, &paths, finalizer.as_deref())
            .await?;
        let effective_builder = match &options.iso_tool {
            Some(executable) => iso_builder.with_executable(executable)?,
            None => iso_builder,
        };
        let timeout = Duration::from_secs(options.timeout_seconds);
        let replaces_baseline = snapshot_plan == SnapshotPlan::ReplaceBaseline;
        let intent = self.prepare_provision_intent(
            provision_id,
            paths,
            guest,
            media,
            source,
            identity_file,
        )?;
        let (plan, mut record) = if replaces_baseline {
            // The powered-off VM still boots the configured baseline snapshot's
            // delta, so the committed boot disk cannot be resolved yet. Back up
            // the VMX and persist the intent before deleting exactly the
            // configured baseline, then resolve and validate the committed disk.
            let vmx_before = self.read_vmx_bytes()?;
            write_synced(&intent.paths.vmx_backup, &vmx_before)?;
            let (_, mut record) =
                self.persist_provision_intent(&mut state, &intent, None, true, None)?;
            match self
                .replace_baseline_snapshot(&intent, offline.as_ref())
                .await
            {
                Ok((vmx_bytes, disk)) => {
                    let boot_task = boot_task_injection()?;
                    let plan = PlannedProvision::assemble(
                        intent,
                        vmx_bytes,
                        disk,
                        boot_task,
                        self.baseline.clone(),
                        effective_builder,
                        timeout,
                    );
                    match self.record_resolved_boot_disk(&mut state, &mut record, &plan) {
                        Ok(()) => (plan, record),
                        Err(error) => return self.abandon_provision(state, record, error),
                    }
                }
                Err(error) => return self.abandon_provision(state, record, error),
            }
        } else {
            let vmx_bytes = self.read_vmx_bytes()?;
            let disk = self
                .plan_guest_disk(&vmx_bytes, &intent, offline.as_ref())
                .await?;
            let (_, record) = self.persist_provision_intent(
                &mut state,
                &intent,
                Some(&disk.guest_disk.path),
                false,
                Some(&vmx_bytes),
            )?;
            let boot_task = boot_task_injection()?;
            let plan = PlannedProvision::assemble(
                intent,
                vmx_bytes,
                disk,
                boot_task,
                self.baseline.clone(),
                effective_builder,
                timeout,
            );
            (plan, record)
        };
        let mut actions = planned_actions(&plan);
        if replaces_baseline {
            actions.insert(
                0,
                format!(
                    "replace the configured baseline snapshot '{}' before validating the guest disk",
                    plan.baseline
                ),
            );
        }
        let evidence = match self.execute_provision(&plan, offline).await {
            Ok(evidence) => evidence,
            Err(error) => return self.abandon_provision(state, record, error),
        };
        record.status = RunStatus::Completed;
        record.iso_sha256 = Some(evidence.iso_sha256);
        record.iso_size = Some(evidence.iso_size);
        record.vmx_sha256_after = Some(evidence.vmx_sha256_after);
        record.hive_sha256_before = Some(evidence.hive_sha256_before);
        record.hive_sha256_after = Some(evidence.hive_sha256_after);
        record.boot_task_sha256 = Some(evidence.boot_task_sha256);
        record.launcher_sha256 = Some(evidence.launcher_sha256);
        record.guest_receipt = Some(evidence.guest_receipt);
        record.connection = Some(serde_json::to_value(&evidence.connection)?);
        record.baseline_snapshot = Some(plan.baseline.clone());
        record.settled = true;
        record.ended_at = Some(now());
        state.phase = LabPhase::Idle;
        state.pending = None;
        state.active = None;
        state.last_provision = Some(record.clone());
        state.last_error = None;
        self.store.save(&state)?;
        remove_provision_working_files(&plan, &self.store);
        let state_dir = self
            .store
            .root()
            .parent()
            .unwrap_or_else(|| self.store.root());
        let environment_block = super::provision_environment::placeholder_block(
            &plan.media,
            &evidence.connection,
            state_dir,
            plan.identity_file.as_deref(),
        )
        .ok();
        let mut registration = None;
        let mut verification = None;
        if (options.register || options.verify_environment)
            && let Some(registrar) = registrar
        {
            let finish = EnvironmentFinish {
                registrar,
                environment_id: plan.media.environment_id.clone(),
                connection: evidence.connection,
                identity_file: plan.identity_file.clone(),
                register: options.register,
                verify: options.verify_environment,
                timeout: Duration::from_secs(options.timeout_seconds),
            };
            let guest_network = guest_network.context(
                "guest address discovery requires the vmrun adapter; configure [windows_lab.vms.<alias>.offline]",
            )?;
            let finished = self
                .finish_environment(&finish, &plan.media, &plan.baseline, guest_network)
                .await;
            // A successful registration rewrote the mapped `[environments.*]`
            // entry, so the persisted configuration identity must move with it
            // in the same commit. Doing this before any later doctor/revert
            // failure keeps every following command usable even though the
            // command itself fails; a failed registration writes nothing and the
            // previous identity (and the guard that binds it) stays in force.
            let adoption = match &finished.registered {
                Some(registered) => self.adopt_registered_configuration(&mut state, registered),
                None => Ok(()),
            };
            let failure = match (finished.failure, adoption) {
                (Some(error), _) | (None, Err(error)) => Some(error),
                (None, Ok(())) => None,
            };
            if let Some(error) = failure {
                let _ = self.step(self.host.revert(&plan.baseline)).await;
                record.environment_verified = Some(false);
                record.error = Some(format!("{error:#}"));
                record.ended_at = Some(now());
                state.last_provision = Some(record.clone());
                state.last_error = Some(format!("{error:#}"));
                self.store.save(&state)?;
                let _ = self.write_provision_receipt(&record);
                return Err(error.context(
                    "the worker was installed but environment registration/verification failed; the baseline snapshot was retained",
                ));
            }
            registration = finished.registration;
            verification = finished.verification;
            record.verification = verification.clone();
            record.environment_verified =
                Some(options.verify_environment && verification.is_some());
            state.last_provision = Some(record.clone());
            self.store.save(&state)?;
        }
        let receipt = self.write_provision_receipt(&record)?;
        Ok(ProvisionOutcome {
            dry_run: false,
            planned_actions: actions,
            provision: Some(record),
            receipt: Some(receipt),
            environment_block,
            registration,
            verification,
        })
    }
}

#[cfg(test)]
mod tests;
