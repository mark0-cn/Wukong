//! Apply the recorded plan, verify guest readback, and finish optional registration.
use super::super::provision::{
    BOOT_TASK_NAME, GuestFile, GuestNetwork, GuestObservationRequest, OfflineDisk,
    ReceiptExpectation, provisioning_spec, sha256, sha256_file, staged_base_snapshot,
    verify_guest_receipt,
};
use super::super::{
    LabPhase, LabProvision, LabState, MediaIdentity, OperationKind, PendingOperation, RunStatus,
    StepFailure, WindowsLab, now,
};
use super::planning::injected_name;
use super::{
    EnvironmentFinish, EnvironmentFinished, PlannedProvision, ProvisionArtifactPaths,
    ProvisionEvidence, ProvisionIntent,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
enum BaseTransition {
    Verified,
    Retired,
    Published,
}

impl WindowsLab {
    /// Persist the provisioning intent and publish its receipt before the first
    /// VM or guest mutation. The fixed boot task is a constant and is not part of
    /// the persisted artifacts.
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
        if replaces_baseline {
            if let Some(previous) = state.last_provision.as_ref().filter(|record| {
                record.status == RunStatus::Completed
                    && record.settled
                    && record.baseline_snapshot.as_deref() == Some(self.baseline.as_str())
            }) {
                state.rollback_provision = Some(previous.clone());
            }
        } else {
            state.rollback_provision = None;
        }
        state.last_provision = Some(record.clone());
        self.store.save(state)?;
        let _ = self.write_provision_receipt(&record);
        Ok((pending, record))
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
        let snapshot = if plan.replaces_baseline {
            staged_base_snapshot(&plan.provision_id)?
        } else {
            plan.baseline.clone()
        };
        self.step(self.host.snapshot(&snapshot))
            .await
            .map_err(step_error)?;
        if plan.replaces_baseline {
            self.publish_verified_base(plan, &snapshot).await?;
        }
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

    /// Boot the staged replacement before retiring the last known-good base.
    /// Reverting the powered-off staging snapshot after doctor removes runtime
    /// changes. VMware has no snapshot-rename operation: delete the old fixed
    /// base only after verification, record that irreversible transition, then
    /// capture the verified staged state under the fixed name.
    async fn publish_verified_base(&self, plan: &PlannedProvision, staged: &str) -> Result<()> {
        let worker = self
            .worker
            .as_ref()
            .context("base replacement requires a configured lab worker for post-install doctor")?;
        if !self.host_running().await? {
            self.step(self.host.start()).await.map_err(step_error)?;
        }
        let observation = self
            .worker_readiness(plan.timeout)
            .await
            .context("upgraded worker did not become ready before base replacement")?;
        let identity = observation
            .get("observation")
            .and_then(|observation| observation.get("identity"))
            .context("upgraded worker doctor has no identity")?;
        let installation = identity
            .get("installation_id")
            .and_then(Value::as_str)
            .context("upgraded worker identity has no installation ID")?;
        let package = identity
            .get("package_sha256")
            .and_then(Value::as_str)
            .context("upgraded worker identity has no package hash")?;
        ensure!(
            installation == plan.media.installation_id && package == plan.media.manifest_sha256,
            "upgraded worker doctor identity differs from the signed replacement media"
        );
        let maintenance = worker.maintenance().await?;
        self.mark_base_verified(&plan.provision_id)?;
        self.step(self.host.revert(staged))
            .await
            .map_err(step_error)?;
        if self.host_running().await? {
            self.step(self.host.stop()).await.map_err(step_error)?;
        }
        self.step(self.host.delete_snapshot(&plan.baseline))
            .await
            .map_err(step_error)?;
        self.mark_base_retired(&plan.provision_id)?;
        self.step(self.host.snapshot(&plan.baseline))
            .await
            .map_err(step_error)?;
        self.mark_base_published(&plan.provision_id)?;
        self.step(self.host.delete_snapshot(staged))
            .await
            .map_err(step_error)?;
        maintenance.finish().await?;
        Ok(())
    }

    /// Make readiness durable before the first irreversible base transition. If
    /// any later host effect is interrupted, reconcile can distinguish a verified
    /// replacement from a staging snapshot that must not become the base.
    fn mark_base_verified(&self, provision_id: &str) -> Result<()> {
        self.mark_base_transition(provision_id, BaseTransition::Verified)
    }

    pub(super) fn mark_base_retired(&self, provision_id: &str) -> Result<()> {
        self.mark_base_transition(provision_id, BaseTransition::Retired)
    }

    pub(super) fn mark_base_published(&self, provision_id: &str) -> Result<()> {
        self.mark_base_transition(provision_id, BaseTransition::Published)
    }

    fn mark_base_transition(&self, provision_id: &str, transition: BaseTransition) -> Result<()> {
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
        let pending = state
            .pending
            .as_mut()
            .context("replacement provisioning intent is missing")?;
        ensure!(
            pending.kind == OperationKind::Provision && pending.operation_id == provision_id,
            "replacement provisioning operation changed before verification"
        );
        let spec = pending
            .provision
            .as_mut()
            .context("replacement provisioning spec is missing")?;
        ensure!(spec.replaces_baseline);
        match transition {
            BaseTransition::Verified => {
                ensure!(!spec.base_verified && !spec.base_retired && !spec.base_published);
                spec.base_verified = true;
            }
            BaseTransition::Retired => {
                ensure!(spec.base_verified && !spec.base_retired && !spec.base_published);
                spec.base_retired = true;
            }
            BaseTransition::Published => {
                ensure!(spec.base_verified && spec.base_retired && !spec.base_published);
                spec.base_published = true;
            }
        }
        self.store.save(&state)
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
