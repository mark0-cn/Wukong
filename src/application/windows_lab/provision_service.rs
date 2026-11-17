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
    BOOT_TASK_NAME, BootTaskInjection, BootTaskRemoval, DiskInspectionRequest, FinalizeRequest,
    GUEST_SYSTEM_HIVE, GuestDisk, GuestFile, GuestNetwork, GuestObservationRequest, GuestPaths,
    GuestRemoval, IsoBuilder, MediaFinalizer, OfflineDisk, PROVISION_WRAPPER, ProvisionEnvironment,
    PublicConnection, ReceiptExpectation, ServiceRemoval, SnapshotPlan, VmxEdit,
    attach_offline_iso, boot_launcher, boot_task_injection, injected_file, plan_guest_cleanup,
    plan_snapshot, provision_paths, provision_plan, provisioning_spec, read_finalize_spec,
    require_relative_path, resolve_guest_disk, sha256, sha256_file, verify_guest_receipt,
    verify_media,
};
use super::*;
use crate::application::windows_lab::provision::ProvisionArtifactPaths;
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

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
    connection: PublicConnection,
}

struct EnvironmentFinish {
    registrar: Arc<dyn ProvisionEnvironment>,
    environment_id: String,
    connection: PublicConnection,
    identity_file: Option<PathBuf>,
    register: bool,
    verify: bool,
    timeout: Duration,
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
        let _lock = self.store.lock(&self.vm_id)?;
        let _environment_guard = crate::adapters::environments::lab_guard::acquire(
            &crate::adapters::environments::lab_guard::path(&self.vmx)?,
            crate::adapters::environments::lab_guard::LockMode::Exclusive,
        )?;
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
        self.mark_restart_recovery(&mut state)?;
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
            match self
                .finish_environment(&finish, &plan.media, &plan.baseline, guest_network)
                .await
            {
                Ok((registered, verified)) => {
                    registration = registered;
                    verification = verified;
                    record.verification = verification.clone();
                    record.environment_verified =
                        Some(options.verify_environment && verification.is_some());
                    state.last_provision = Some(record.clone());
                    self.store.save(&state)?;
                }
                Err(error) => {
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
            }
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

    /// Collect the media-derived guest files and the durable intent fields that
    /// do not depend on the guest disk. Nothing is mutated here.
    fn prepare_provision_intent(
        &self,
        provision_id: String,
        paths: ProvisionArtifactPaths,
        guest: GuestPaths,
        media: MediaIdentity,
        source: ProvisionSource,
        identity_file: Option<PathBuf>,
    ) -> Result<ProvisionIntent> {
        let plan_bytes = provision_plan(&provision_id, &media, &guest)?;
        let boot_task = boot_task_injection()?;
        let guest_files = vec![
            injected_file(&guest.wrapper, PROVISION_WRAPPER.as_bytes().to_vec()),
            injected_file(&guest.plan, plan_bytes),
            injected_file(
                &guest.launcher,
                boot_launcher(&provision_id, &guest.wrapper)?,
            ),
            boot_task.file.clone(),
        ];
        let injected = injected_records(&guest_files)?;
        Ok(ProvisionIntent {
            provision_id,
            paths,
            guest,
            media,
            source,
            identity_file,
            guest_files,
            injected,
        })
    }

    /// Persist the provisioning intent and publish its receipt before the first
    /// VM or guest mutation. `guest_disk` and `vmx_before` are absent only when
    /// the intent precedes the forced baseline deletion; the boot task is a fixed
    /// constant and is not part of the persisted artifacts.
    fn persist_provision_intent(
        &self,
        state: &mut LabState,
        intent: &ProvisionIntent,
        guest_disk: Option<&Path>,
        replaces_baseline: bool,
        vmx_before: Option<&[u8]>,
    ) -> Result<(PendingOperation, LabProvision)> {
        fs::create_dir_all(&intent.paths.root).with_context(|| {
            format!("create provision directory {}", intent.paths.root.display())
        })?;
        fs::create_dir_all(&intent.paths.injected)?;
        write_injected_copies(&intent.paths, &intent.guest_files)?;
        let generation = state.generation + 1;
        let spec = provisioning_spec(
            &intent.provision_id,
            generation,
            intent.media.clone(),
            intent.source.clone(),
            &intent.paths,
            guest_disk,
            &intent.guest,
            &self.baseline,
            replaces_baseline,
            intent.identity_file.clone(),
            intent.injected.clone(),
        );
        let pending = PendingOperation {
            operation_id: intent.provision_id.clone(),
            kind: OperationKind::Provision,
            generation,
            lab_id: String::new(),
            owner: String::new(),
            purpose: String::new(),
            spec_sha256: String::new(),
            spec: None,
            provision: Some(spec),
            package_sha256: None,
            install_sha256: None,
            snapshot: self.baseline.clone(),
            started_at: now(),
        };
        let record = LabProvision {
            provision_id: intent.provision_id.clone(),
            operation_id: intent.provision_id.clone(),
            generation,
            status: RunStatus::Running,
            source: intent.source.clone(),
            media: intent.media.clone(),
            iso_sha256: None,
            iso_size: None,
            vmx_sha256_before: vmx_before.map(sha256),
            vmx_sha256_after: None,
            hive_sha256_before: None,
            hive_sha256_after: None,
            injected: intent.injected.clone(),
            boot_task_sha256: None,
            launcher_sha256: None,
            scripts_ini_sha256: None,
            gpt_ini_sha256: None,
            identity_file: intent.identity_file.clone(),
            guest_receipt: None,
            connection: None,
            environment_verified: None,
            verification: None,
            baseline_snapshot: None,
            settled: false,
            started_at: now(),
            ended_at: None,
            error: None,
        };
        state.generation = generation;
        state.phase = LabPhase::Provisioning;
        state.pending = Some(pending.clone());
        state.active = None;
        state.last_error = None;
        state.last_provision = Some(record.clone());
        self.store.save(state)?;
        let _ = self.write_provision_receipt(&record);
        Ok((pending, record))
    }

    fn read_vmx_bytes(&self) -> Result<Vec<u8>> {
        super::provision::read_bounded(&self.vmx, 4 * 1024 * 1024)
            .with_context(|| format!("read registered VMX {}", self.vmx.display()))
    }

    /// Resolve one full boot disk and inspect the powered-off system volume. This
    /// is only valid once the VM no longer points at a snapshot delta. The
    /// preflight refuses a disk that still carries an earlier attempt's guest
    /// leftovers and names the explicit cleanup command for that operation.
    async fn plan_guest_disk(
        &self,
        vmx_bytes: &[u8],
        intent: &ProvisionIntent,
        offline: &dyn OfflineDisk,
    ) -> Result<ProvisionDiskPlan> {
        let vmx_text =
            std::str::from_utf8(vmx_bytes).context("registered VMX is not valid UTF-8")?;
        let guest_disk = resolve_guest_disk(vmx_text, &self.vmx)?;
        let inspection = offline
            .inspect(
                &guest_disk.path,
                &DiskInspectionRequest::for_provision(&intent.guest),
            )
            .await
            .context("offline preflight of the powered-off guest disk")?;
        ensure!(
            inspection.windows_volume && inspection.system_hive_present,
            "guest disk has no inspectable Windows system volume"
        );
        ensure!(
            inspection.software_hive_present,
            "guest disk has no inspectable SOFTWARE hive"
        );
        ensure!(
            inspection.control_set.is_some(),
            "guest SYSTEM hive has no readable current control set"
        );
        self.refuse_leftover_guest_state(&inspection)?;
        let vmx_edit = attach_offline_iso(vmx_text, &intent.paths.iso)?;
        Ok(ProvisionDiskPlan {
            guest_disk,
            vmx_edit,
        })
    }

    /// A stale artifact of a previous attempt must be refused with the exact
    /// operator command that removes it, never silently retried over.
    fn refuse_leftover_guest_state(
        &self,
        inspection: &super::provision::DiskInspection,
    ) -> Result<()> {
        let ids = inspection.leftover_operation_ids();
        if ids.is_empty()
            && inspection.existing_service.is_none()
            && inspection.provision_root_entries.is_empty()
            && !inspection.has_boot_task_leftovers()
        {
            return Ok(());
        }
        let operations = if ids.is_empty() {
            "an unknown earlier operation".to_owned()
        } else {
            ids.join(", ")
        };
        let example = ids
            .first()
            .map(|id| {
                format!(
                    "`wukong lab clean-guest --vm {} --operation {id}`",
                    self.vm_id
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "`wukong lab clean-guest --vm {} --operation <operation_id>`",
                    self.vm_id
                )
            });
        bail!(
            "guest still carries offline provisioning leftovers from {operations} (scheduled task registration or launcher, provision directory, \
             retired startup policy entry or legacy service key); power off the VM and clean that operation first with {example}, then retry provisioning"
        )
    }

    /// Delete exactly the configured baseline snapshot on a powered-off VM and
    /// validate the committed boot disk. The caller has already backed up the
    /// VMX and persisted the intent; every failure here stays recoverable and
    /// names the VMX backup.
    async fn replace_baseline_snapshot(
        &self,
        intent: &ProvisionIntent,
        offline: &dyn OfflineDisk,
    ) -> Result<(Vec<u8>, ProvisionDiskPlan)> {
        let backup = intent.paths.vmx_backup.display();
        self.step(self.host.delete_snapshot(&self.baseline))
            .await
            .map_err(step_error)
            .with_context(|| {
                format!(
                    "delete the configured baseline snapshot '{}'; the VMX backup is {backup}",
                    self.baseline
                )
            })?;
        let vmx_bytes = self.read_vmx_bytes().with_context(|| {
            format!(
                "re-read the VMX after deleting the configured baseline snapshot '{}'; the VMX backup is {backup}",
                self.baseline
            )
        })?;
        // Replacing the backup with the committed VMX keeps reconciliation able
        // to restore a bootable VMX even though the snapshot delta is gone.
        write_synced(&intent.paths.vmx_backup, &vmx_bytes).with_context(|| {
            format!(
                "refresh the VMX backup after deleting the configured baseline snapshot '{}'",
                self.baseline
            )
        })?;
        self.plan_guest_disk(&vmx_bytes, intent, offline)
            .await
            .with_context(|| {
                format!(
                    "the configured baseline snapshot '{}' was deleted and the VMX backup is {backup}; inspect the powered-off VM",
                    self.baseline
                )
            })
            .map(|disk| (vmx_bytes, disk))
    }

    /// Record the boot disk and VMX hash that only become known after the forced
    /// baseline deletion. A failure here keeps the persisted intent and is
    /// resolved through recovery.
    fn record_resolved_boot_disk(
        &self,
        state: &mut LabState,
        record: &mut LabProvision,
        plan: &PlannedProvision,
    ) -> Result<()> {
        let spec = state
            .pending
            .as_mut()
            .and_then(|pending| pending.provision.as_mut())
            .context("provisioning intent was lost after the baseline deletion")?;
        spec.guest_disk = Some(plan.guest_disk.path.clone());
        record.vmx_sha256_before = Some(sha256(&plan.vmx_bytes));
        state.last_provision = Some(record.clone());
        self.store.save(state)?;
        let _ = self.write_provision_receipt(record);
        Ok(())
    }

    async fn resolve_media(
        &self,
        options: &ProvisionOptions,
        paths: &ProvisionArtifactPaths,
        finalizer: Option<&dyn MediaFinalizer>,
    ) -> Result<(MediaIdentity, ProvisionSource, Option<PathBuf>)> {
        match &options.source {
            ProvisionSource::Media { directory } => {
                ensure!(
                    directory.is_absolute(),
                    "offline media directory must be absolute"
                );
                let media = verify_media(directory)?;
                ensure!(
                    !options.register,
                    "--register requires --template media so lab provision can generate the controller-managed SSH identity key"
                );
                Ok((media, options.source.clone(), None))
            }
            ProvisionSource::Template {
                template,
                keys,
                spec,
            } => {
                ensure!(
                    template.is_absolute() && keys.is_absolute() && spec.is_absolute(),
                    "template, key directory and finalize spec paths must be absolute"
                );
                ensure!(
                    template.is_dir(),
                    "offline media template directory is missing: {}",
                    template.display()
                );
                ensure!(
                    keys.is_dir(),
                    "controller key directory is missing: {}",
                    keys.display()
                );
                let finalize_spec = read_finalize_spec(spec)?;
                let identity_file = if finalize_spec.ssh_public_key.is_none() {
                    let ssh = paths.root.join("ssh");
                    fs::create_dir_all(&ssh).with_context(|| {
                        format!("create controller SSH identity directory {}", ssh.display())
                    })?;
                    Some(ssh.join("management_ed25519"))
                } else {
                    None
                };
                ensure!(
                    !options.register || identity_file.is_some(),
                    "--register requires a controller-generated SSH identity; omit ssh_public_key from the finalize spec"
                );
                let finalizer = finalizer.context(
                    "the offline media finalizer is unavailable; run lab provision from the wukong checkout",
                )?;
                let request = FinalizeRequest {
                    template: template.clone(),
                    keys: keys.clone(),
                    spec: spec.clone(),
                    output: paths.media.clone(),
                    generate_ssh_key: identity_file.clone(),
                };
                finalizer
                    .finalize(&request)
                    .await
                    .context("finalize offline worker media")?;
                let verified = verify_media(&paths.media)?;
                verified.check_finalize_spec(&finalize_spec)?;
                if let Some(identity) = &identity_file {
                    let public = read_generated_identity(identity)?;
                    ensure!(
                        public == verified.ssh_public_key,
                        "generated controller SSH identity differs from the finalized media"
                    );
                }
                Ok((verified, options.source.clone(), identity_file))
            }
        }
    }

    /// Validate and describe the plan without mutating the VM, the guest, the
    /// controller state or (for `--template`) the finalized media directory.
    async fn dry_run_plan(
        &self,
        options: &ProvisionOptions,
        paths: ProvisionArtifactPaths,
        guest: GuestPaths,
        snapshot_plan: SnapshotPlan,
        iso_builder: Arc<dyn IsoBuilder>,
        offline: Arc<dyn OfflineDisk>,
    ) -> Result<ProvisionOutcome> {
        ensure!(
            options
                .iso_tool
                .as_ref()
                .is_none_or(|path| path.is_absolute()),
            "--iso-tool must be an absolute path"
        );
        if snapshot_plan == SnapshotPlan::ReplaceBaseline {
            return self.dry_run_replace_baseline(options, paths, guest).await;
        }
        let mut actions = Vec::new();
        match &options.source {
            ProvisionSource::Media { directory } => {
                ensure!(
                    directory.is_absolute(),
                    "offline media directory must be absolute"
                );
                let media = verify_media(directory)?;
                ensure!(
                    !options.register,
                    "--register requires --template media so lab provision can generate the controller-managed SSH identity key"
                );
                let builder = match &options.iso_tool {
                    Some(executable) => iso_builder.with_executable(executable)?,
                    None => iso_builder,
                };
                let intent = self.prepare_provision_intent(
                    guest.provision_id().to_owned(),
                    paths,
                    guest,
                    media,
                    options.source.clone(),
                    None,
                )?;
                let vmx_bytes = self.read_vmx_bytes()?;
                let disk = self
                    .plan_guest_disk(&vmx_bytes, &intent, offline.as_ref())
                    .await?;
                let plan = PlannedProvision::assemble(
                    intent,
                    vmx_bytes,
                    disk,
                    boot_task_injection()?,
                    self.baseline.clone(),
                    builder,
                    Duration::from_secs(options.timeout_seconds),
                );
                actions = planned_actions(&plan);
            }
            ProvisionSource::Template {
                template,
                keys,
                spec,
            } => {
                ensure!(
                    template.is_absolute() && keys.is_absolute() && spec.is_absolute(),
                    "template, key directory and finalize spec paths must be absolute"
                );
                ensure!(
                    template.is_dir(),
                    "offline media template directory is missing: {}",
                    template.display()
                );
                ensure!(
                    keys.is_dir(),
                    "controller key directory is missing: {}",
                    keys.display()
                );
                let finalize_spec = read_finalize_spec(spec)?;
                let identity_file = if finalize_spec.ssh_public_key.is_none() {
                    Some(paths.root.join("ssh").join("management_ed25519"))
                } else {
                    None
                };
                ensure!(
                    !options.register || identity_file.is_some(),
                    "--register requires a controller-generated SSH identity; omit ssh_public_key from the finalize spec"
                );
                ensure!(
                    !paths.media.exists(),
                    "finalize media output already exists: {}",
                    paths.media.display()
                );
                let vmx_bytes = super::provision::read_bounded(&self.vmx, 4 * 1024 * 1024)
                    .with_context(|| format!("read registered VMX {}", self.vmx.display()))?;
                let vmx_text =
                    String::from_utf8(vmx_bytes).context("registered VMX is not valid UTF-8")?;
                let guest_disk = resolve_guest_disk(&vmx_text, &self.vmx)?;
                let inspection = offline
                    .inspect(
                        &guest_disk.path,
                        &DiskInspectionRequest::for_provision(&guest),
                    )
                    .await
                    .context("offline preflight of the powered-off guest disk")?;
                ensure!(
                    inspection.windows_volume && inspection.system_hive_present,
                    "guest disk has no inspectable Windows system volume"
                );
                ensure!(
                    inspection.software_hive_present,
                    "guest disk has no inspectable SOFTWARE hive"
                );
                self.refuse_leftover_guest_state(&inspection)?;
                let edit = attach_offline_iso(&vmx_text, &paths.iso)?;
                actions.push(format!(
                    "validate the finalize spec for environment {} (installation {}, generation {}, ssh_port {})",
                    finalize_spec.environment_id,
                    finalize_spec.installation_id,
                    finalize_spec.generation,
                    finalize_spec.ssh_port
                ));
                actions.push(format!(
                    "run python -m scripts.worker.media finalize --template {} --keys {} --spec {} --output {}{}",
                    template.display(),
                    keys.display(),
                    spec.display(),
                    paths.media.display(),
                    match &identity_file {
                        Some(key) => format!(" --generate-ssh-key {}", key.display()),
                        None => String::new(),
                    }
                ));
                actions.push(
                    "re-verify the finalized media signatures, receipt hashes and generated SSH identity"
                        .into(),
                );
                actions.push(format!(
                    "build ISO {} with the configured mkisofs (-V WUKONG_OFFLINE)",
                    paths.iso.display()
                ));
                actions.push(format!(
                    "back up VMX bytes to {}",
                    paths.vmx_backup.display()
                ));
                actions.push(format!(
                    "attach {} to {} in the VMX",
                    paths.iso.display(),
                    edit.device
                ));
                actions.push(format!(
                    "write fixed guest files {}, {} and {} through libguestfs",
                    guest.wrapper, guest.plan, guest.launcher
                ));
                actions.push(format!(
                    "write the fixed scheduled-task file {} and register the boot task {} ({}) in the guest SOFTWARE hive",
                    guest.task_file, BOOT_TASK_NAME, guest.task_guid
                ));
                actions.push(format!(
                    "start the VM; the {BOOT_TASK_NAME} boot task runs the wrapper as SYSTEM without a login, then powers the guest off"
                ));
                actions.push(
                    "restore the original VMX bytes and verify the guest receipt plus the removed boot task offline"
                        .into(),
                );
                actions.push(format!(
                    "create the baseline snapshot '{}' and leave the VM powered off",
                    self.baseline
                ));
                actions.push(format!(
                    "publish the provisioning receipt under {}/provisions",
                    self.store.root().display()
                ));
                actions.push(format!(
                    "guest disk {} (device {}) is provisioned offline",
                    guest_disk.path.display(),
                    guest_disk.device
                ));
            }
        }
        Ok(ProvisionOutcome {
            dry_run: true,
            planned_actions: actions,
            provision: None,
            receipt: None,
            environment_block: None,
            registration: None,
            verification: None,
        })
    }

    /// `--dry-run` with `--replace-baseline`: the powered-off VM still points at
    /// the baseline snapshot's delta, so the committed guest disk can only be
    /// validated after the deletion. Report the ordering and every non-disk
    /// action without mutating the VM, the guest or the controller state.
    async fn dry_run_replace_baseline(
        &self,
        options: &ProvisionOptions,
        paths: ProvisionArtifactPaths,
        guest: GuestPaths,
    ) -> Result<ProvisionOutcome> {
        let mut actions = Vec::new();
        match &options.source {
            ProvisionSource::Media { directory } => {
                ensure!(
                    directory.is_absolute(),
                    "offline media directory must be absolute"
                );
                let media = verify_media(directory)?;
                ensure!(
                    !options.register,
                    "--register requires --template media so lab provision can generate the controller-managed SSH identity key"
                );
                actions.push(format!(
                    "re-verify finalized media {} (setup {}, enrollment {}, manifest {})",
                    media.media_root.display(),
                    media.setup_sha256,
                    media.enrollment_sha256,
                    media.manifest_sha256
                ));
            }
            ProvisionSource::Template {
                template,
                keys,
                spec,
            } => {
                ensure!(
                    template.is_absolute() && keys.is_absolute() && spec.is_absolute(),
                    "template, key directory and finalize spec paths must be absolute"
                );
                ensure!(
                    template.is_dir(),
                    "offline media template directory is missing: {}",
                    template.display()
                );
                ensure!(
                    keys.is_dir(),
                    "controller key directory is missing: {}",
                    keys.display()
                );
                let finalize_spec = read_finalize_spec(spec)?;
                let identity_file = if finalize_spec.ssh_public_key.is_none() {
                    Some(paths.root.join("ssh").join("management_ed25519"))
                } else {
                    None
                };
                ensure!(
                    !options.register || identity_file.is_some(),
                    "--register requires a controller-generated SSH identity; omit ssh_public_key from the finalize spec"
                );
                ensure!(
                    !paths.media.exists(),
                    "finalize media output already exists: {}",
                    paths.media.display()
                );
                actions.push(format!(
                    "validate the finalize spec for environment {} (installation {}, generation {}, ssh_port {})",
                    finalize_spec.environment_id,
                    finalize_spec.installation_id,
                    finalize_spec.generation,
                    finalize_spec.ssh_port
                ));
                actions.push(format!(
                    "run python -m scripts.worker.media finalize --template {} --keys {} --spec {} --output {}{}",
                    template.display(),
                    keys.display(),
                    spec.display(),
                    paths.media.display(),
                    match &identity_file {
                        Some(key) => format!(" --generate-ssh-key {}", key.display()),
                        None => String::new(),
                    }
                ));
                actions.push(
                    "re-verify the finalized media signatures, receipt hashes and generated SSH identity"
                        .into(),
                );
            }
        }
        let vmx_bytes = self.read_vmx_bytes()?;
        let vmx_text = String::from_utf8(vmx_bytes).context("registered VMX is not valid UTF-8")?;
        let edit = attach_offline_iso(&vmx_text, &paths.iso)?;
        actions.push(format!(
            "back up VMX bytes to {}",
            paths.vmx_backup.display()
        ));
        actions.push(format!(
            "persist the provisioning intent and delete the configured baseline snapshot '{}'",
            self.baseline
        ));
        actions.push(
            "resolve and validate the guest disk and powered-off system volume after the baseline deletion"
                .into(),
        );
        actions.push(format!(
            "build ISO {} with the configured mkisofs (-V WUKONG_OFFLINE)",
            paths.iso.display()
        ));
        actions.push(format!(
            "attach {} to {} in the VMX",
            paths.iso.display(),
            edit.device
        ));
        actions.push(format!(
            "write fixed guest files {}, {} and {} through libguestfs",
            guest.wrapper, guest.plan, guest.launcher
        ));
        actions.push(format!(
            "write the fixed scheduled-task file {} and register the boot task {} ({}) in the guest SOFTWARE hive",
            guest.task_file, BOOT_TASK_NAME, guest.task_guid
        ));
        actions.push(format!(
            "start the VM; the {BOOT_TASK_NAME} boot task runs the wrapper as SYSTEM without a login, then powers the guest off"
        ));
        actions.push(
            "restore the original VMX bytes and verify the guest receipt plus the removed boot task offline"
                .into(),
        );
        actions.push(format!(
            "create the baseline snapshot '{}' and leave the VM powered off",
            self.baseline
        ));
        actions.push(format!(
            "publish the provisioning receipt under {}/provisions",
            self.store.root().display()
        ));
        Ok(ProvisionOutcome {
            dry_run: true,
            planned_actions: actions,
            provision: None,
            receipt: None,
            environment_block: None,
            registration: None,
            verification: None,
        })
    }

    async fn execute_provision(
        &self,
        plan: &PlannedProvision,
        offline: Arc<dyn OfflineDisk>,
    ) -> Result<ProvisionEvidence> {
        plan.iso_builder
            .build(&plan.media.media_root, &plan.paths.iso)
            .await
            .context("build the WUKONG_OFFLINE ISO")?;
        let (iso_sha256, iso_size) = sha256_file(&plan.paths.iso)?;
        ensure!(iso_size > 0, "generated ISO is empty");
        write_synced(&plan.paths.vmx_backup, &plan.vmx_bytes)?;
        write_synced(&self.vmx, plan.vmx_edit.text.as_bytes())?;
        offline
            .write_guest_files(&plan.guest_disk.path, &plan.guest_files)
            .await
            .context("write guest provisioning files and scheduled-task boot launcher")?;
        let task = offline
            .inject_boot_task(&plan.guest_disk.path, &plan.boot_task)
            .await
            .context("register the scheduled-task boot trigger in the guest SOFTWARE hive")?;
        tracing::info!(
            vm = %self.vm_id,
            operation = %plan.provision_id,
            task = BOOT_TASK_NAME,
            "guest scheduled-task boot trigger injected; no SYSTEM service key or local-GPO startup script is written"
        );
        self.step(self.host.start()).await.map_err(step_error)?;
        self.wait_power_off(plan.timeout).await?;
        write_synced(&self.vmx, &plan.vmx_bytes)?;
        let vmx_sha256_after = sha256(&plan.vmx_bytes);
        let observation = offline
            .observe_guest(
                &plan.guest_disk.path,
                &GuestObservationRequest::for_provision(&plan.guest),
            )
            .await
            .context("read the provisioned guest offline")?;
        ensure!(
            !observation.task_file_present,
            "the guest scheduled-task file was not removed"
        );
        ensure!(
            !observation.boot_task_registered,
            "the guest scheduled-task registration was not removed"
        );
        ensure!(
            !observation.launcher_present,
            "the guest scheduled-task boot launcher was not removed"
        );
        ensure!(
            !observation.startup_entry_present
                && !observation.scripts_ini_backup_present
                && !observation.gpt_ini_backup_present,
            "the guest still carries retired local-GPO provisioning artifacts"
        );
        ensure!(
            !observation.wrapper_present && !observation.plan_present,
            "the guest provisioning wrapper or plan was not removed"
        );
        let receipt_bytes = observation
            .receipt
            .context("guest provisioning receipt is missing")?;
        let connection = verify_guest_receipt(
            &receipt_bytes,
            &ReceiptExpectation {
                provision_id: plan.provision_id.clone(),
                media: plan.media.clone(),
            },
        )?;
        let guest_receipt: Value = serde_json::from_slice(
            receipt_bytes
                .strip_prefix(&[0xef, 0xbb, 0xbf])
                .unwrap_or(&receipt_bytes),
        )
        .context("parse the guest provisioning receipt for the record")?;
        fs::write(plan.paths.root.join("guest-receipt.json"), &receipt_bytes)?;
        self.step(self.host.snapshot(&plan.baseline))
            .await
            .map_err(step_error)?;
        Ok(ProvisionEvidence {
            iso_sha256,
            iso_size,
            vmx_sha256_after,
            hive_sha256_before: task.hive_sha256_before,
            hive_sha256_after: task.hive_sha256_after,
            boot_task_sha256: plan.boot_task.file.sha256.clone(),
            launcher_sha256: plan
                .guest_files
                .iter()
                .find(|file| file.windows_path == plan.guest.launcher)
                .map(|file| file.sha256.clone())
                .context("boot launcher was not part of the injected guest files")?,
            guest_receipt,
            connection,
        })
    }

    async fn wait_power_off(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        tokio::time::sleep(Duration::from_secs(2)).await;
        loop {
            if !self.host_running().await? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!(
                    "guest did not power off within {} seconds; provisioning requires reconciliation",
                    timeout.as_secs()
                );
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    async fn host_running(&self) -> Result<bool> {
        tokio::time::timeout(self.timeout, self.host.running())
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("VM status probe timed out")))
    }

    async fn finish_environment(
        &self,
        finish: &EnvironmentFinish,
        media: &MediaIdentity,
        baseline: &str,
        guest_network: Arc<dyn GuestNetwork>,
    ) -> Result<(Option<Value>, Option<Value>)> {
        ensure!(
            finish.register || finish.verify,
            "no environment registration or verification was requested"
        );
        if !self.host_running().await? {
            self.step(self.host.start()).await.map_err(step_error)?;
        }
        let deadline = Instant::now() + finish.timeout;
        let mut registration = None;
        if finish.register {
            let identity = finish.identity_file.as_deref().context(
                "registered environment requires the controller-managed SSH identity key",
            )?;
            let guest_ip = self.wait_guest_ip(guest_network.as_ref(), deadline).await?;
            registration = Some(
                finish
                    .registrar
                    .register(&finish.connection, media, &guest_ip, identity)
                    .context("register the provisioned environment")?,
            );
        }
        let mut verification = None;
        if finish.verify {
            loop {
                match finish.registrar.verify(&finish.environment_id).await {
                    Ok(value) => {
                        verification = Some(value);
                        break;
                    }
                    Err(error) => {
                        if Instant::now() >= deadline {
                            bail!(
                                "registered environment doctor did not pass before the provisioning deadline: {error:#}"
                            );
                        }
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                }
            }
        }
        self.step(self.host.revert(baseline))
            .await
            .map_err(step_error)?;
        Ok((registration, verification))
    }

    async fn wait_guest_ip(
        &self,
        guest_network: &dyn GuestNetwork,
        deadline: Instant,
    ) -> Result<String> {
        loop {
            match guest_network.guest_ip().await {
                Ok(address) => return Ok(address),
                Err(error) => {
                    if Instant::now() >= deadline {
                        return Err(error.context(
                            "the hypervisor did not report a guest IPv4 address before the provisioning deadline",
                        ));
                    }
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
        }
    }

    fn abandon_provision<T>(
        &self,
        mut state: LabState,
        mut record: LabProvision,
        error: anyhow::Error,
    ) -> Result<T> {
        let pending = state
            .pending
            .clone()
            .context("provisioning intent was lost before its failure was recorded")?;
        record.status = RunStatus::Unknown;
        record.error = Some(format!("{error:#}"));
        record.ended_at = Some(now());
        record.settled = false;
        state.phase = LabPhase::RecoveryRequired;
        state.pending = Some(pending.clone());
        state.last_provision = Some(record.clone());
        state.last_error = Some(format!(
            "provisioning failed after the intent was persisted: {error:#}; inspect the VM and reconcile operation {}",
            pending.operation_id
        ));
        self.store.save(&state)?;
        let receipt = self.write_provision_receipt(&record).ok();
        Err(LabError {
            code: "lab_provision_unknown",
            operation_id: pending.operation_id,
            generation: pending.generation,
            receipt,
            message: format!(
                "provisioning failed after intent: {error:#}; inspect the VM, then reconcile with the operation id"
            ),
        }
        .into())
    }

    fn write_provision_receipt(&self, record: &LabProvision) -> Result<PathBuf> {
        let directory = self.store.root().join("provisions");
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{}.json", record.provision_id));
        let mut bytes = serde_json::to_vec_pretty(record)?;
        bytes.push(b'\n');
        let mut staged = tempfile::NamedTempFile::new_in(&directory)?;
        staged.write_all(&bytes)?;
        staged.as_file().sync_all()?;
        staged
            .persist(&path)
            .map_err(|error| error.error)
            .context("publish provisioning receipt")?;
        Ok(path)
    }

    /// Remove exactly one failed attempt's leftovers from the powered-off guest
    /// disk. The VMX is not edited and controller lab state is not changed: the
    /// provisioning operation is resolved separately with `lab reconcile`.
    pub async fn clean_guest_leftovers(
        &self,
        options: CleanGuestOptions,
    ) -> Result<CleanGuestOutcome> {
        let paths = GuestPaths::for_provision(&options.operation_id)
            .context("--operation must name one offline provisioning operation")?;
        let offline = self.offline.clone().context(
            "offline provisioning tooling is not configured; add [windows_lab.vms.<alias>.offline] with wsl_executable, wsl_distribution, guestfish, hivexsh, mkisofs and timeout_seconds",
        )?;
        let _gate = self.gate.lock().await;
        let _lock = self.store.lock(&self.vm_id)?;
        let _environment_guard = crate::adapters::environments::lab_guard::acquire(
            &crate::adapters::environments::lab_guard::path(&self.vmx)?,
            crate::adapters::environments::lab_guard::LockMode::Exclusive,
        )?;
        let state = self.store.load(&self.vm_id, &self.baseline)?;
        self.require_known_provision_operation(&state, &paths)?;
        let running = self
            .host_running()
            .await
            .context("inspect lab VM power state before cleaning guest leftovers")?;
        ensure!(
            !running,
            "power off lab VM {} before cleaning guest leftovers",
            self.vm_id
        );
        let snapshots = self.step(self.host.snapshots()).await.map_err(step_error)?;
        ensure!(
            snapshots.is_empty(),
            "lab VM {} still has snapshot(s) {snapshots:?}; guest cleanup requires a committed powered-off disk",
            self.vm_id
        );
        let vmx_bytes = self.read_vmx_bytes()?;
        let vmx_text =
            std::str::from_utf8(&vmx_bytes).context("registered VMX is not valid UTF-8")?;
        let guest_disk = resolve_guest_disk(vmx_text, &self.vmx)?;
        let inspection = offline
            .inspect(
                &guest_disk.path,
                &DiskInspectionRequest::for_provision(&paths),
            )
            .await
            .context("offline preflight of the powered-off guest disk")?;
        ensure!(
            inspection.windows_volume && inspection.system_hive_present,
            "guest disk has no inspectable Windows system volume"
        );
        ensure!(
            inspection.software_hive_present,
            "guest disk has no inspectable SOFTWARE hive"
        );
        // Only the last recorded provisioning operation can own a CLI-created
        // `gpt.ini` (that document carries no operation identity).
        let last_operation = state
            .last_provision
            .as_ref()
            .is_some_and(|record| record.provision_id == paths.provision_id());
        let plan = plan_guest_cleanup(&paths, &inspection, last_operation)?;
        let mut outcome = CleanGuestOutcome {
            dry_run: options.dry_run,
            vm_id: self.vm_id.clone(),
            operation_id: options.operation_id.clone(),
            guest_disk: Some(guest_disk.path.clone()),
            actions: plan.actions.clone(),
            files_written: 0,
            files_removed: Vec::new(),
            directories_removed: Vec::new(),
            boot_task_removed: false,
            service_key_removed: None,
        };
        if options.dry_run || plan.is_empty() {
            if plan.is_empty() {
                outcome.actions.push(format!(
                    "no offline provisioning leftovers from operation {} were found on the guest disk",
                    options.operation_id
                ));
            }
            return Ok(outcome);
        }
        // Mutations run least-destructive first: the fixed scheduled-task
        // registration (the only artifact that can repeat a boot), the legacy
        // SYSTEM-hive service key, then the restored machine startup policy, then
        // the operation's leftover files, and only last its provision directory.
        // Every step is idempotent, so a partial failure stops before later
        // removals and a retry only repeats the reported pending steps.
        let boot_task_step = "remove the fixed scheduled-task boot registration";
        let service_step = "remove the legacy one-shot provisioning service key";
        let policy_step = "restore the machine startup policy";
        let files_step = "remove the operation's leftover guest files";
        let directory_step = "remove the operation's provision directory";
        let mut pending: Vec<&'static str> = Vec::new();
        if plan.boot_task {
            pending.push(boot_task_step);
        }
        if plan.service_key.is_some() {
            pending.push(service_step);
        }
        if !plan.files.is_empty() {
            pending.push(policy_step);
        }
        if !plan.files_to_remove.is_empty() {
            pending.push(files_step);
        }
        if !plan.directories_to_remove.is_empty() {
            pending.push(directory_step);
        }
        let mut completed: Vec<&'static str> = Vec::new();
        if plan.boot_task {
            let step = pending.remove(0);
            let removal = BootTaskRemoval {
                software_hive: super::provision::GUEST_SOFTWARE_HIVE.into(),
                registry: boot_task_injection()?.registry,
            };
            offline
                .remove_boot_task(&guest_disk.path, &removal)
                .await
                .with_context(|| cleanup_failure(step, &completed, &pending))?;
            outcome.boot_task_removed = true;
            completed.push(step);
        }
        if let Some(service) = &plan.service_key {
            let step = pending.remove(0);
            let evidence = offline
                .remove_service_key(
                    &guest_disk.path,
                    &ServiceRemoval {
                        system_hive: GUEST_SYSTEM_HIVE.into(),
                        service: service.clone(),
                    },
                )
                .await
                .with_context(|| cleanup_failure(step, &completed, &pending))?;
            if evidence.removed {
                outcome.service_key_removed = Some(service.clone());
            }
            completed.push(step);
        }
        if !plan.files.is_empty() {
            let step = pending.remove(0);
            offline
                .write_guest_files(&guest_disk.path, &plan.files)
                .await
                .with_context(|| cleanup_failure(step, &completed, &pending))?;
            outcome.files_written = plan.files.len();
            completed.push(step);
        }
        let leftover_files = GuestRemoval {
            directories: Vec::new(),
            files: plan.files_to_remove.clone(),
        };
        if !leftover_files.is_empty() {
            let step = pending.remove(0);
            let evidence = offline
                .remove_guest_paths(&guest_disk.path, &leftover_files)
                .await
                .with_context(|| cleanup_failure(step, &completed, &pending))?;
            outcome.files_removed = evidence.removed_files;
            completed.push(step);
        }
        let provision_directories = GuestRemoval {
            directories: plan.directories_to_remove.clone(),
            files: Vec::new(),
        };
        if !provision_directories.is_empty() {
            let step = pending.remove(0);
            let evidence = offline
                .remove_guest_paths(&guest_disk.path, &provision_directories)
                .await
                .with_context(|| cleanup_failure(step, &completed, &pending))?;
            outcome.directories_removed = evidence.removed_directories;
            completed.push(step);
        }
        debug_assert!(pending.is_empty(), "every planned cleanup step ran");
        tracing::info!(
            vm = %self.vm_id,
            operation = %options.operation_id,
            files_written = outcome.files_written,
            files_removed = outcome.files_removed.len(),
            directories_removed = outcome.directories_removed.len(),
            "removed offline provisioning guest leftovers"
        );
        Ok(outcome)
    }

    /// Only a recorded provision operation of this VM may be cleaned. An
    /// unknown or foreign identity is refused before the guest disk is touched,
    /// and an unrelated pending operation must be reconciled first.
    fn require_known_provision_operation(
        &self,
        state: &LabState,
        paths: &GuestPaths,
    ) -> Result<()> {
        let operation_id = paths.provision_id();
        if let Some(pending) = &state.pending {
            ensure!(
                pending.kind == OperationKind::Provision && pending.operation_id == operation_id,
                "lab VM {} has pending {:?} operation {}; reconcile it before cleaning operation {operation_id}",
                self.vm_id,
                pending.kind,
                pending.operation_id
            );
        }
        let recorded = state.pending.as_ref().is_some_and(|pending| {
            pending.kind == OperationKind::Provision && pending.operation_id == operation_id
        }) || state.last_provision.as_ref().is_some_and(|record| {
            record.provision_id == operation_id || record.operation_id == operation_id
        });
        ensure!(
            recorded,
            "operation {operation_id} is not a recorded offline provisioning of lab VM {}; refusing to modify the guest disk",
            self.vm_id
        );
        Ok(())
    }

    /// Deterministic cleanup used by `lab reconcile` before an operation is
    /// completed or failed: restore the VMX from its backup and remove the
    /// generated ISO and template-finalized media.
    pub(crate) async fn cleanup_provision_artifacts(
        &self,
        operation: &PendingOperation,
    ) -> Result<()> {
        let spec = operation
            .provision
            .as_ref()
            .context("pending provision operation lost its intent")?;
        let provisions_root = self.store.root().join("provisions");
        require_relative_path(&spec.iso, &provisions_root)?;
        require_relative_path(&spec.vmx_backup, &provisions_root)?;
        if self.host_running().await? {
            bail!(
                "power off lab VM {} before reconciling provisioning artifacts",
                self.vm_id
            );
        }
        if spec.vmx_backup.is_file() {
            let original = super::provision::read_bounded(&spec.vmx_backup, 4 * 1024 * 1024)
                .context("read the VMX backup")?;
            write_synced(&self.vmx, &original)?;
            // Reconcile is not atomic: a transient sharing violation on the
            // backup or ISO can stop it after the VMX was restored. Retry the
            // removal briefly, and because every step is idempotent a second
            // reconcile still converges when the file remains locked.
            retry_file_removal(&spec.vmx_backup).context(
                "the VMX was restored but its backup could not be removed; rerun lab reconcile",
            )?;
        }
        if spec.iso.is_file() {
            retry_file_removal(&spec.iso)
                .context("the generated ISO could not be removed; rerun lab reconcile")?;
        }
        if let ProvisionSource::Template { .. } = &spec.source {
            let media = spec
                .vmx_backup
                .parent()
                .context("provision intent has no artifact directory")?
                .join("media");
            require_relative_path(&media, &provisions_root)?;
            if media.is_dir() {
                retry_directory_removal(&media)
                    .context("the finalized media could not be removed; rerun lab reconcile")?;
            }
        }
        Ok(())
    }
}

/// Bounded retry for a filesystem removal that can transiently fail with a
/// sharing violation (an antivirus scan, the hypervisor still closing the ISO, or
/// the VMX backup being read). The operation is idempotent: a file that vanished
/// between attempts counts as success.
fn retry_bounded(mut operation: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match operation() {
            Ok(()) => return Ok(()),
            Err(error) if attempts >= 5 => return Err(error),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(200)),
        }
    }
}

fn retry_file_removal(path: &Path) -> std::io::Result<()> {
    retry_bounded(|| match fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    })
}

fn retry_directory_removal(path: &Path) -> std::io::Result<()> {
    retry_bounded(|| match fs::remove_dir_all(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    })
}

/// Guest cleanup cannot roll back disk mutations, so a partial failure reports
/// the failed step, the steps that already completed and those it deliberately
/// skipped. The ordering is least-destructive first and every step is
/// idempotent, which makes retrying only the reported pending steps safe.
fn cleanup_failure(step: &str, completed: &[&str], not_attempted: &[&str]) -> String {
    let format_steps = |steps: &[&str]| {
        if steps.is_empty() {
            "none".to_owned()
        } else {
            steps.join(", ")
        }
    };
    format!(
        "failed to {step}; completed: [{}]; not attempted: [{}]",
        format_steps(completed),
        format_steps(not_attempted)
    )
}

fn planned_actions(plan: &PlannedProvision) -> Vec<String> {
    let mut actions = vec![
        format!(
            "re-verify finalized media {} (setup {}, enrollment {}, manifest {})",
            plan.media.media_root.display(),
            plan.media.setup_sha256,
            plan.media.enrollment_sha256,
            plan.media.manifest_sha256
        ),
        format!(
            "build ISO {} from the verified media with the configured mkisofs (-V WUKONG_OFFLINE)",
            plan.paths.iso.display()
        ),
        format!("back up VMX bytes to {}", plan.paths.vmx_backup.display()),
        format!(
            "attach {} to {} in the VMX",
            plan.paths.iso.display(),
            plan.vmx_edit.device
        ),
        format!(
            "write fixed guest files {}, {} and {} through libguestfs",
            plan.guest.wrapper, plan.guest.plan, plan.guest.launcher
        ),
        format!(
            "write the fixed scheduled-task file {} and register the boot task {} ({}) in the guest SOFTWARE hive",
            plan.guest.task_file, BOOT_TASK_NAME, plan.guest.task_guid
        ),
        format!(
            "start the VM; the {BOOT_TASK_NAME} boot task runs the wrapper as SYSTEM without a login, then powers the guest off"
        ),
        "restore the original VMX bytes, verify the guest receipt and the removed scheduled-task registration offline"
            .into(),
        format!(
            "create the baseline snapshot '{}' and leave the VM powered off",
            plan.baseline
        ),
        format!(
            "publish the provisioning receipt under {}/provisions",
            plan.paths
                .root
                .parent()
                .unwrap_or(&plan.paths.root)
                .display()
        ),
    ];
    actions.push(format!(
        "guest disk {} (device {}) is provisioned offline; the ISO is {:.1} MiB",
        plan.guest_disk.path.display(),
        plan.guest_disk.device,
        plan.media
            .media_root
            .metadata()
            .map(|m| m.len() as f64 / 1_048_576.0)
            .unwrap_or_default()
    ));
    actions
}

fn injected_name(file: &GuestFile) -> Result<String> {
    Path::new(&file.windows_path)
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .with_context(|| format!("guest file has no file name: {}", file.windows_path))
}

fn injected_records(files: &[GuestFile]) -> Result<Vec<InjectedFile>> {
    files
        .iter()
        .map(|file| {
            Ok(InjectedFile {
                guest_path: file.windows_path.clone(),
                sha256: file.sha256.clone(),
                size: file.bytes.len() as u64,
                // The scheduled-task mechanism only writes files it creates:
                // nothing on the guest is preserved or backed up. The retired
                // local-GPO mechanism recorded preserved policy files this way.
                preexisting: false,
            })
        })
        .collect()
}

fn write_injected_copies(paths: &ProvisionArtifactPaths, files: &[GuestFile]) -> Result<()> {
    for file in files {
        fs::write(paths.injected.join(injected_name(file)?), &file.bytes)?;
    }
    Ok(())
}

fn remove_provision_working_files(plan: &PlannedProvision, store: &LabStore) {
    let _ = fs::remove_file(&plan.paths.iso);
    let _ = fs::remove_file(&plan.paths.vmx_backup);
    if let ProvisionSource::Template { .. } = &plan.source {
        let provisions_root = store.root().join("provisions");
        let media = plan.paths.root.join("media");
        if media.starts_with(&provisions_root) {
            let _ = fs::remove_dir_all(media);
        }
    }
}

fn read_generated_identity(private: &Path) -> Result<super::SshKey> {
    let public = private.with_extension("pub");
    let bytes = super::provision::read_bounded(&public, 4 * 1024)
        .with_context(|| format!("read the generated SSH public key {}", public.display()))?;
    let text = String::from_utf8(bytes).context("generated SSH public key is not UTF-8")?;
    let mut fields = text.split_whitespace();
    let algorithm = fields
        .next()
        .context("generated SSH public key is empty")?
        .to_owned();
    let key = fields
        .next()
        .context("generated SSH public key is incomplete")?;
    let key = super::SshKey {
        algorithm,
        key: key.to_owned(),
    };
    key.validate()?;
    Ok(key)
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(bytes)?;
    staged.as_file().sync_all()?;
    staged
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("publish {}", path.display()))?;
    #[cfg(unix)]
    {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn step_error(failure: StepFailure) -> anyhow::Error {
    match failure {
        StepFailure::Failed(error) => error,
        StepFailure::Timeout => {
            anyhow::anyhow!("host operation timed out; the external effect requires reconciliation")
        }
    }
}

#[cfg(test)]
mod tests;
