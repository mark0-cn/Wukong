//! Resumable evidence transfer. Network reads never hold the job resource lock.
use super::*;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[cfg(test)]
#[path = "evidence_sync_tests.rs"]
mod tests;

#[derive(Clone)]
pub(super) struct SyncLimits {
    global: Arc<Semaphore>,
    environments: Arc<Mutex<BTreeMap<String, Arc<Semaphore>>>>,
}
impl Default for SyncLimits {
    fn default() -> Self {
        Self {
            global: Arc::new(Semaphore::new(2)),
            environments: Default::default(),
        }
    }
}
impl SyncLimits {
    async fn acquire(
        &self,
        environment: &str,
    ) -> Result<(OwnedSemaphorePermit, OwnedSemaphorePermit)> {
        let local = self
            .environments
            .lock()
            .await
            .entry(environment.into())
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone();
        // A busy environment must not consume a global slot while it waits.
        let local = local.acquire_owned().await?;
        Ok((local, self.global.clone().acquire_owned().await?))
    }
}

pub(super) struct EvidenceSyncService {
    jobs: jobs::JobService,
}
impl EvidenceSyncService {
    pub(super) fn new(jobs: jobs::JobService) -> Self {
        Self { jobs }
    }

    pub(super) async fn sync(&self, agent: &AgentMetadata, call: &str, id: &str) -> Result<Value> {
        use tracing::Instrument as _;
        let started = std::time::Instant::now();
        let span = tracing::info_span!(
            "wukong.evidence.sync",
            run_id = %agent.run_id,
            agent_id = %agent.id,
            call_id = call,
            job_id = id,
            outcome = tracing::field::Empty,
        );
        let result = self
            .sync_inner(agent, call, id)
            .instrument(span.clone())
            .await;
        let state = result
            .as_ref()
            .ok()
            .and_then(|value| value["evidence_sync"]["state"].as_str())
            .unwrap_or("error");
        let outcome = match state {
            "ready" => "ok",
            "pending" | "syncing" => "pending",
            "retry" => "retry",
            "failed" => "failed",
            _ => "error",
        };
        span.record("outcome", outcome);
        if outcome != "pending" {
            let duration = started.elapsed().as_secs_f64();
            tracing::info!(
                target: "wukong_telemetry",
                event = "evidence_sync_finished",
                run_id = %agent.run_id,
                agent_id = %agent.id,
                call_id = call,
                job_id = id,
                outcome,
                duration_seconds = duration,
            );
            self.jobs
                .telemetry
                .metrics()
                .evidence_sync(outcome, duration);
        }
        result
    }

    async fn sync_inner(&self, agent: &AgentMetadata, call: &str, id: &str) -> Result<Value> {
        let lock = {
            let mut locks = self.jobs.locks.lock().await;
            locks
                .entry(format!("evidence:job:{id}"))
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _sync = lock.lock().await;
        let mut job = self.jobs.known_job(agent, id, true)?;
        if job.evidence_ready() {
            return self.complete(&job).await;
        }
        if !job.status.settled()
            || job.evidence_sync.state == SyncState::Failed
            || job.evidence_sync.retry_at.is_some_and(|at| at > now())
        {
            return Ok(Self::progress(&job));
        }
        let _slots = self.jobs.sync_limits.acquire(&job.worker_id).await?;
        job.evidence_sync.state = SyncState::Syncing;
        job.evidence_sync.error = None;
        job.evidence_sync.retry_at = None;
        self.save(&job).await?;
        if !job.evidence_sync.manifest_loaded {
            let request = Request::JobArtifacts(wukong_worker_protocol::params::JobArtifactsArgs {
                job_id: job.worker_resource_id.clone(),
            });
            let response = tokio::time::timeout(
                Duration::from_secs(10),
                self.jobs.gateway.worker_query(request),
            )
            .await;
            let raw = match response {
                Ok(Ok(value)) => value,
                result => {
                    return self
                        .failed(
                            &mut job,
                            false,
                            format!("artifact manifest transport failed: {result:?}"),
                        )
                        .await;
                }
            };
            match manifest(&job.worker_id, &raw) {
                Ok(files) => job.evidence_sync.files = files,
                Err(error) => return self.failed(&mut job, true, error.to_string()).await,
            }
            job.evidence_sync.manifest_loaded = true;
            self.save(&job).await?;
        }
        for index in 0..job.evidence_sync.files.len() {
            let file = job.evidence_sync.files[index].clone();
            if let Some(evidence) = &file.evidence_id {
                if let Err(error) = self
                    .jobs
                    .store
                    .verified_evidence_path(&job.run_id, evidence)
                {
                    // An unreadable file or a failed metadata call can heal; a missing
                    // record, a cross-run link or a digest mismatch cannot. Freezing the
                    // job on every error made a transient fault block completion forever.
                    let permanent = error.downcast_ref::<std::io::Error>().is_none();
                    return self
                        .failed(
                            &mut job,
                            permanent,
                            format!("stored evidence check failed: {error}"),
                        )
                        .await;
                }
                continue;
            }
            // Recover the crash window between artifact registration and the
            // per-file checkpoint without downloading successful content again.
            let retained = self
                .jobs
                .files
                .known_file(agent, &file.path, false)
                .ok()
                .filter(|old| old.sha256 == file.sha256 && old.size == file.size)
                .and_then(|old| old.evidence_id)
                .filter(|id| {
                    self.jobs
                        .store
                        .verified_evidence_path(&job.run_id, id)
                        .is_ok()
                });
            let evidence = if let Some(id) = retained {
                id
            } else {
                let response = tokio::time::timeout(
                    Duration::from_secs(30),
                    self.jobs.gateway.runtime()?.call(
                        "file.get",
                        json!({"path":file.path,"max_bytes":32*1024*1024}),
                    ),
                )
                .await;
                let raw = match response {
                    Ok(Ok(value)) => value,
                    result => {
                        job.evidence_sync.files[index].error =
                            Some("file transport failed; retry pending".into());
                        return self
                            .failed(
                                &mut job,
                                false,
                                format!("file transport failed: {result:?}"),
                            )
                            .await;
                    }
                };
                if let Err(error) = validate_file(&file, &raw) {
                    job.evidence_sync.files[index].error = Some(error.to_string());
                    return self.failed(&mut job, true, error.to_string()).await;
                }
                // The identity/fence check and local evidence publication are
                // serialized with manual status updates, never with network IO.
                let imported = {
                    let resource = self.jobs.resource_lock(id).await;
                    let _guard = resource.lock().await;
                    let current = self.jobs.known_job(agent, id, true)?;
                    jobs::JobService::check_identity(&job, &current)?;
                    ensure!(
                        current.status.settled(),
                        "job cleanup changed during evidence transfer"
                    );
                    self.jobs.files.import_file(agent, call, &raw)
                };
                match imported {
                    Ok(evidence) => evidence.id,
                    Err(error) => {
                        return self
                            .failed(
                                &mut job,
                                false,
                                format!("evidence publication failed: {error}"),
                            )
                            .await;
                    }
                }
            };
            job.evidence_sync.files[index].evidence_id = Some(evidence);
            job.evidence_sync.files[index].error = None;
            job.evidence_sync.attempts = 0;
            self.save(&job).await?;
        }
        match self.complete(&job).await {
            Ok(value) => Ok(value),
            Err(error) => {
                self.failed(
                    &mut job,
                    false,
                    format!("evidence postprocessing failed: {error}"),
                )
                .await
            }
        }
    }

    async fn save(&self, expected: &JobRecord) -> Result<()> {
        let resource = self.jobs.resource_lock(&expected.id).await;
        let _guard = resource.lock().await;
        self.jobs.store.write(|tx| {
            let mut current: JobRecord = tx.get_record(&expected.id)?.context("job missing")?;
            jobs::JobService::check_identity(expected, &current)?;
            ensure!(
                current.status.settled(),
                "job cleanup changed during evidence transfer"
            );
            current.evidence_sync = expected.evidence_sync.clone();
            current.updated_at = now();
            tx.save_record(&current)
        })
    }

    async fn failed(&self, job: &mut JobRecord, permanent: bool, error: String) -> Result<Value> {
        job.evidence_sync.state = if permanent {
            SyncState::Failed
        } else {
            SyncState::Retry
        };
        job.evidence_sync.attempts = job.evidence_sync.attempts.saturating_add(1);
        job.evidence_sync.retry_at = (!permanent).then(|| {
            now()
                + chrono::Duration::seconds(
                    (1_i64 << job.evidence_sync.attempts.saturating_sub(1).min(5)).min(30),
                )
        });
        job.evidence_sync.error = Some(error.chars().take(512).collect());
        self.save(job).await?;
        Ok(Self::progress(job))
    }

    async fn complete(&self, expected: &JobRecord) -> Result<Value> {
        let resource = self.jobs.resource_lock(&expected.id).await;
        let _guard = resource.lock().await;
        let mut job: JobRecord = self.jobs.store.load(&expected.id)?;
        jobs::JobService::check_identity(expected, &job)?;
        ensure!(
            job.status.settled()
                && (job.evidence_ready()
                    || (job.evidence_sync.manifest_loaded
                        && job
                            .evidence_sync
                            .files
                            .iter()
                            .all(|file| file.evidence_id.is_some()))),
            "job evidence is not ready"
        );
        if job.status_evidence_id.is_none() {
            return Ok(Self::progress(&job));
        }
        if job.imported_artifacts.is_none() {
            ensure!(
                job.evidence_sync.manifest_loaded
                    && job
                        .evidence_sync
                        .files
                        .iter()
                        .all(|f| f.evidence_id.is_some()),
                "job evidence manifest is incomplete"
            );
            job.imported_artifacts = Some(
                json!({"job_id":job.id,"artifacts":job.evidence_sync.files.iter().map(|f|
                json!({"artifact":{"path":f.path,"sha256":f.sha256,"size":f.size},"evidence_id":f.evidence_id})).collect::<Vec<_>>() }),
            );
        }
        if job.status_evidence_id.is_some() {
            super::artifact_validation::observe(&self.jobs.store, &job)?;
            self.jobs.store.write(|tx| {
                let mut current: JobRecord = tx.get_record(&job.id)?.context("job missing")?;
                jobs::JobService::check_identity(&job, &current)?;
                current.imported_artifacts = job.imported_artifacts.clone();
                current.evidence_sync.state = SyncState::Ready;
                current.evidence_sync.error = None;
                current.evidence_sync.retry_at = None;
                jobs::JobService::notify(tx, &mut current, true)?;
                tx.save_record(&current)
            })?;
        }
        job.imported_artifacts
            .clone()
            .context("job artifacts missing")
    }

    fn progress(job: &JobRecord) -> Value {
        json!({"job_id":job.id,"artifacts":job.evidence_sync.files.iter().filter(|f|f.evidence_id.is_some())
            .map(|f|json!({"artifact":{"path":f.path,"sha256":f.sha256,"size":f.size},"evidence_id":f.evidence_id})).collect::<Vec<_>>(),
            "evidence_sync":job.evidence_sync.progress(job.evidence_ready())})
    }
}

fn manifest(worker: &str, raw: &Value) -> Result<Vec<SyncFile>> {
    let items = raw
        .as_array()
        .context("worker artifact manifest is not an array")?;
    ensure!(
        items.len() <= 512,
        "worker artifact manifest exceeds 512 files"
    );
    let mut paths = std::collections::BTreeSet::new();
    items
        .iter()
        .map(|item| {
            let path = text(item, "path")?;
            let sha256 = text(item, "sha256")?;
            let size = item["size"]
                .as_u64()
                .context("worker artifact size missing")?;
            ensure!(
                paths.insert(path) && path.len() <= 4096,
                "duplicate or oversized artifact path"
            );
            ensure!(
                sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid artifact hash"
            );
            ensure!(
                size <= 32 * 1024 * 1024,
                "worker artifact exceeds 32 MiB import limit"
            );
            Ok(SyncFile {
                worker_id: worker.into(),
                path: path.into(),
                sha256: sha256.into(),
                size,
                evidence_id: None,
                error: None,
            })
        })
        .collect()
}

fn validate_file(file: &SyncFile, raw: &Value) -> Result<()> {
    ensure!(
        raw["path"] == file.path && raw["sha256"] == file.sha256 && raw["size"] == file.size,
        "job artifact identity, hash or size changed during transfer"
    );
    let encoded = raw["data_base64"]
        .as_str()
        .context("artifact base64 content missing")?;
    ensure!(
        encoded.len() <= 32 * 1024 * 1024 * 4 / 3 + 4,
        "worker artifact exceeds transfer limit"
    );
    let bytes = BASE64.decode(encoded).context("invalid artifact base64")?;
    ensure!(
        bytes.len() as u64 == file.size && binary::sha256_bytes(&bytes) == file.sha256,
        "worker artifact integrity mismatch"
    );
    Ok(())
}
