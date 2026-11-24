//! Persist failed effects and clean up only artifacts owned by a recorded operation.
use super::super::provision::{
    BootTaskRemoval, DiskInspectionRequest, GUEST_SYSTEM_HIVE, GuestPaths, GuestRemoval,
    ServiceRemoval, boot_task_injection, plan_guest_cleanup, require_relative_path,
    resolve_guest_disk, restore_vmx_preserving_active_disk, staged_base_snapshot,
};
use super::super::{
    LabError, LabPhase, LabProvision, LabState, LabStore, OperationKind, PendingOperation,
    ProvisionSource, ReconcileOutcome, RunStatus, WindowsLab, now,
};
use super::execution::{step_error, write_synced};
use super::{CleanGuestOptions, CleanGuestOutcome, PlannedProvision};
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};
use std::{fs, io::Write};

impl WindowsLab {
    pub(super) fn abandon_provision<T>(
        &self,
        mut record: LabProvision,
        error: anyhow::Error,
    ) -> Result<T> {
        // Readiness may have advanced the durable replacement marker after the
        // caller's original state clone. Never overwrite that marker on failure.
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
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

    pub(super) fn write_provision_receipt(&self, record: &LabProvision) -> Result<PathBuf> {
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
        let _lock = if options.dry_run {
            self.store.lock_preview(&self.vm_id)?
        } else {
            self.store.lock(&self.vm_id)?
        };
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
                software_hive: super::super::provision::GUEST_SOFTWARE_HIVE.into(),
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

    /// Resolve only snapshot names owned by this provisioning operation. A
    /// completed replacement must have passed readiness and identity checks.
    /// Failed reconciliation is allowed only while the old fixed base still
    /// exists; after its retirement there is no old state to roll back to.
    pub(crate) async fn reconcile_provision_snapshots(
        &self,
        operation: &PendingOperation,
        outcome: ReconcileOutcome,
    ) -> Result<()> {
        let spec = operation
            .provision
            .as_ref()
            .context("pending provision operation lost its intent")?;
        ensure!(
            spec.baseline_snapshot == self.baseline,
            "pending provision names a different fixed base"
        );
        ensure!(
            !self.host_running().await?,
            "power off lab VM {} before reconciling provisioning snapshots",
            self.vm_id
        );
        let names = self.step(self.host.snapshots()).await.map_err(step_error)?;
        let has = |name: &str| names.iter().any(|candidate| candidate == name);
        let base = &self.baseline;
        if !spec.replaces_baseline {
            match outcome {
                ReconcileOutcome::Completed => ensure!(
                    has(base),
                    "cannot complete provisioning without fixed base snapshot '{base}'"
                ),
                ReconcileOutcome::Failed => {
                    if has(base) {
                        self.step(self.host.delete_snapshot(base))
                            .await
                            .map_err(step_error)?;
                    }
                }
                ReconcileOutcome::Unknown => bail!("unknown outcome cannot resolve snapshots"),
            }
            return Ok(());
        }

        let staged = staged_base_snapshot(&spec.provision_id)?;
        match outcome {
            ReconcileOutcome::Completed => {
                ensure!(
                    spec.base_verified,
                    "a replacement base cannot be completed before staged worker readiness"
                );
                ensure!(
                    !spec.base_published || spec.base_retired,
                    "a published replacement must have a durable retirement marker"
                );
                if !has(base) {
                    ensure!(
                        has(&staged),
                        "completed replacement has neither fixed nor staged base"
                    );
                    if !spec.base_retired {
                        // The old base was deleted before its marker was saved.
                        // Record that observed irreversible fact before continuing.
                        self.mark_base_retired(&spec.provision_id)?;
                    }
                    // Recreate the fixed name from the verified staged state,
                    // never from an unverified disk.
                    self.step(self.host.revert(&staged))
                        .await
                        .map_err(step_error)?;
                    self.step(self.host.snapshot(base))
                        .await
                        .map_err(step_error)?;
                } else if !spec.base_retired {
                    ensure!(
                        has(&staged),
                        "an unpublished replacement requires its verified staged base"
                    );
                    // The operator confirmed completion before the old base was
                    // retired. Finish the same verified transition explicitly and
                    // persist each irreversible observation before continuing.
                    self.step(self.host.revert(&staged))
                        .await
                        .map_err(step_error)?;
                    self.step(self.host.delete_snapshot(base))
                        .await
                        .map_err(step_error)?;
                    self.mark_base_retired(&spec.provision_id)?;
                    self.step(self.host.snapshot(base))
                        .await
                        .map_err(step_error)?;
                }
                if !spec.base_published {
                    self.mark_base_published(&spec.provision_id)?;
                }
                if has(&staged) {
                    self.step(self.host.delete_snapshot(&staged))
                        .await
                        .map_err(step_error)?;
                }
            }
            ReconcileOutcome::Failed => {
                ensure!(
                    !spec.base_retired && !spec.base_published && has(base),
                    "the old fixed base has been retired or is missing; failed reconciliation cannot roll back this replacement"
                );
                if has(&staged) {
                    self.step(self.host.delete_snapshot(&staged))
                        .await
                        .map_err(step_error)?;
                }
                self.step(self.host.revert(base))
                    .await
                    .map_err(step_error)?;
            }
            ReconcileOutcome::Unknown => bail!("unknown outcome cannot resolve snapshots"),
        }
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
            let original = super::super::provision::read_bounded(&spec.vmx_backup, 4 * 1024 * 1024)
                .context("read the VMX backup")?;
            let current = super::super::provision::read_bounded(&self.vmx, 4 * 1024 * 1024)
                .context("read the post-reconciliation VMX")?;
            let restored = restore_vmx_preserving_active_disk(&original, &current, &self.vmx)
                .context("restore the VMX without losing VMware's active delta")?;
            write_synced(&self.vmx, &restored)?;
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

pub(super) fn remove_provision_working_files(plan: &PlannedProvision, store: &LabStore) {
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

#[cfg(test)]
mod tests {
    use super::retry_bounded;

    #[test]
    fn transient_file_removal_failures_are_retried_with_a_bound() {
        let mut attempts = 0;
        retry_bounded(|| {
            attempts += 1;
            if attempts < 3 {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            } else {
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(attempts, 3, "a transient failure is retried");

        let mut attempts = 0;
        let error = retry_bounded(|| {
            attempts += 1;
            Err::<(), _>(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        })
        .unwrap_err();
        assert_eq!(attempts, 5, "retries are bounded");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);

        let mut attempts = 0;
        retry_bounded(|| {
            attempts += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(attempts, 1, "a success is not retried");
    }
}
