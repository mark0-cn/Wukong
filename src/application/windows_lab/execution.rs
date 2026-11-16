//! Packaged harness admission and administrative installation receipts.
use super::*;

impl WindowsLab {
    pub fn recorded_run(&self) -> Result<Option<LabRun>> {
        Ok(self.store.load(&self.vm_id, &self.baseline)?.last_run)
    }

    pub async fn record_job_completion(
        &self,
        job_id: &str,
        status: &crate::adapters::worker::protocol::JobStatus,
    ) -> Result<()> {
        ensure!(status.settled(), "worker job cleanup is not settled");
        let _gate = self.gate.lock().await;
        let _lock = self.store.lock(&self.vm_id)?;
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
        self.authorize(
            state
                .active
                .as_ref()
                .context("lab has no active experiment")?,
        )?;
        let run = state.last_run.as_mut().context("lab has no recorded run")?;
        ensure!(
            run.job_id.as_deref() == Some(job_id),
            "job observation belongs to another lab run"
        );
        run.status = run_status(
            serde_json::to_value(status.state)?
                .as_str()
                .unwrap_or("unknown"),
        );
        run.settled = true;
        run.ended_at = Some(now());
        self.store.save(&state)
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
        self.validate_active(&state, lab_id, generation)?;
        if let Some(previous) = state.last_run.as_mut()
            && let Some(job_id) = &previous.job_id
            && (!previous.settled
                || matches!(previous.status, RunStatus::Running | RunStatus::Unknown))
            && let Some(observed) = worker.observe(job_id).await?
        {
            previous.status = run_status(&observed.state);
            previous.settled = observed.settled;
            previous.exit_code = observed.exit_code;
            previous.ended_at = (previous.status != RunStatus::Running).then(now);
            self.store.save(&state)?;
        }

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
        self.authorize(&active)?;
        ensure!(
            active.lab_id == lab_id && active.generation == generation,
            "stale lab lease: request generation {generation} is not the active generation {}",
            state.generation
        );
        ensure!(
            active.spec.launch == LaunchKind::Exe
                && active.spec.account == AccountContext::User
                && matches!(
                    active.spec.input_channel,
                    InputChannel::Stdin | InputChannel::Argv
                ),
            "harness execution supports only exe/user with stdin or empty argv; other requirements are unsupported"
        );
        ensure!(
            state
                .last_run
                .as_ref()
                .is_none_or(|run| run.settled
                    && !matches!(run.status, RunStatus::Running | RunStatus::Unknown))
                && !matches!(
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
        ensure!(
            active.spec.target_sha256 == artifact_sha256,
            "selected executable does not match the experiment target identity"
        );
        ensure!(
            stdin.is_none() || active.spec.input_channel == InputChannel::Stdin,
            "stdin requires input_channel=stdin"
        );
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
        run.ended_at = (run.status != RunStatus::Running).then(now);
        state.last_run = Some(run.clone());
        self.store.save(&state)?;
        let receipt = self.write_run_receipt(&run)?;
        if run.status == RunStatus::Unknown {
            return Err(LabError {
                code: "lab_run_unknown",
                operation_id: run_id,
                generation,
                receipt: Some(receipt),
                message: "inspect the worker and reconcile the existing job before restoring"
                    .into(),
            }
            .into());
        }
        Ok(serde_json::json!({
            "vm_id": self.vm_id,
            "receipt": receipt,
            "run": run,
        }))
    }

    fn load_artifact(&self, package_id: &str, artifact: &str) -> Result<(Vec<u8>, String)> {
        let root = self.store.packages_root().join(package_id);
        let manifest = package::open_verified(&root, package_id)?;
        let entry = manifest
            .artifacts
            .iter()
            .find(|entry| entry.name == artifact)
            .with_context(|| format!("artifact {artifact} is not in package {package_id}"))?;
        let path = root.join(artifact);
        let bytes = package::read_bounded(&path, package::MAX_ARTIFACT_BYTES)
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
        let _environment_guard = crate::adapters::environments::lab_guard::acquire(
            &crate::adapters::environments::lab_guard::path(&self.vmx)?,
            true,
        )?;
        let mut state = self.store.load(&self.vm_id, &self.baseline)?;
        self.validate_active(&state, lab_id, generation)?;
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
        self.authorize(&active)?;
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
        let maintenance = if let Some(worker) = &self.worker
            && self.host.running().await?
        {
            Some(worker.maintenance().await?)
        } else {
            None
        };
        let media = media::prepare(self.store.root(), &package_id)?;
        let install_id = uuid::Uuid::new_v4().simple().to_string();
        let mut record = LabInstall {
            install_id: install_id.clone(),
            lab_id: lab_id.to_owned(),
            generation,
            package_sha256: package_id,
            media_sha256: media.sha256.clone(),
            install_sha256,
            script_sha256: media.script_sha256.clone(),
            guest_receipt: serde_json::Value::Null,
            verified: false,
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
        let outcome = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            admin.install(AdminRequest {
                install_id: install_id.clone(),
                media_sha256: media.sha256,
                script_sha256: media.script_sha256,
                plan: active
                    .spec
                    .install
                    .clone()
                    .context("install plan missing")?,
                files: media
                    .files
                    .iter()
                    .map(|file| AdminFile {
                        name: file.name.clone(),
                        path: file.path.clone(),
                        sha256: file.sha256.clone(),
                    })
                    .collect(),
                timeout_secs,
            }),
        )
        .await
        .unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "guest installation deadline elapsed; effect is unknown"
            ))
        });
        match outcome {
            Ok(outcome) => {
                record.status = if outcome.exit_code == Some(0) && outcome.verified {
                    RunStatus::Completed
                } else {
                    RunStatus::Failed
                };
                record.verified = outcome.verified;
                record.guest_receipt = outcome.guest_receipt;
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
            return Err(LabError {
                code: "lab_install_unknown",
                operation_id: install_id,
                generation,
                receipt: Some(receipt),
                message: "inspect guest execution and reconcile before restoring".into(),
            }
            .into());
        }

        if let Some(maintenance) = maintenance {
            maintenance.finish().await?;
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
}
