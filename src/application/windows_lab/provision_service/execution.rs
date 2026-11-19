//! Apply the recorded plan, verify guest readback, and finish optional registration.
use super::super::provision::{
    BOOT_TASK_NAME, GuestFile, GuestNetwork, GuestObservationRequest, OfflineDisk,
    ReceiptExpectation, provisioning_spec, sha256, sha256_file, verify_guest_receipt,
};
use super::super::{
    LabPhase, LabProvision, LabState, MediaIdentity, OperationKind, PendingOperation, RunStatus,
    StepFailure, WindowsLab, now,
};
use super::planning::injected_name;
use super::{
    EnvironmentFinish, EnvironmentFinished, PlannedProvision, ProvisionArtifactPaths,
    ProvisionDiskPlan, ProvisionEvidence, ProvisionIntent,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

impl WindowsLab {
    /// Persist the provisioning intent and publish its receipt before the first
    /// VM or guest mutation. `guest_disk` and `vmx_before` are absent only when
    /// the intent precedes the forced baseline deletion; the boot task is a fixed
    /// constant and is not part of the persisted artifacts.
    pub(super) fn persist_provision_intent(
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

    /// Delete exactly the configured baseline snapshot on a powered-off VM and
    /// validate the committed boot disk. The caller has already backed up the
    /// VMX and persisted the intent; every failure here stays recoverable and
    /// names the VMX backup.
    pub(super) async fn replace_baseline_snapshot(
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
    pub(super) fn record_resolved_boot_disk(
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

    pub(super) async fn execute_provision(
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

    pub(super) async fn host_running(&self) -> Result<bool> {
        tokio::time::timeout(self.timeout, self.host.running())
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("VM status probe timed out")))
    }

    /// Boot the VM, write and verify the mapped environment, then restore the
    /// baseline. The returned outcome keeps the written environment even when a
    /// later step fails, so the caller can adopt its configuration identity
    /// exactly once the rewrite is durable.
    pub(super) async fn finish_environment(
        &self,
        finish: &EnvironmentFinish,
        media: &MediaIdentity,
        baseline: &str,
        guest_network: Arc<dyn GuestNetwork>,
    ) -> EnvironmentFinished {
        let mut finished = EnvironmentFinished {
            registration: None,
            verification: None,
            registered: None,
            failure: None,
        };
        let deadline = Instant::now() + finish.timeout;
        let result: Result<()> = async {
            ensure!(
                finish.register || finish.verify,
                "no environment registration or verification was requested"
            );
            if !self.host_running().await? {
                self.step(self.host.start()).await.map_err(step_error)?;
            }
            if finish.register {
                let identity = finish.identity_file.as_deref().context(
                    "registered environment requires the controller-managed SSH identity key",
                )?;
                let guest_ip = self.wait_guest_ip(guest_network.as_ref(), deadline).await?;
                let registered = finish
                    .registrar
                    .register(&finish.connection, media, &guest_ip, identity)
                    .context("register the provisioned environment")?;
                // The configuration rewrite is durable at this point; retain
                // the entry even if the doctor or the baseline revert below
                // fails, or those failures would leave the persisted lab hash
                // stale with no CLI repair path.
                finished.registration = Some(registered.report.clone());
                finished.registered = Some(registered);
            }
            if finish.verify {
                loop {
                    match finish.registrar.verify(&finish.environment_id).await {
                        Ok(value) => {
                            finished.verification = Some(value);
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
            Ok(())
        }
        .await;
        if let Err(error) = result {
            finished.failure = Some(error);
        }
        finished
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
}

fn write_injected_copies(paths: &ProvisionArtifactPaths, files: &[GuestFile]) -> Result<()> {
    for file in files {
        fs::write(paths.injected.join(injected_name(file)?), &file.bytes)?;
    }
    Ok(())
}

pub(super) fn write_synced(path: &Path, bytes: &[u8]) -> Result<()> {
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

pub(super) fn step_error(failure: StepFailure) -> anyhow::Error {
    match failure {
        StepFailure::Failed(error) => error,
        StepFailure::Timeout => {
            anyhow::anyhow!("host operation timed out; the external effect requires reconciliation")
        }
    }
}
