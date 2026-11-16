//! State observation and transactional two-phase job notifications.
use super::*;
use crate::adapters::worker::protocol::JobStatus;

#[derive(Clone)]
pub(crate) struct JobService {
    pub(super) store: Store,
    pub(super) gateway: WorkerGateway,
    pub(super) files: FilesService,
    pub(super) locks: ResourceLocks,
    pub(super) sync_limits: evidence_sync::SyncLimits,
}

impl JobService {
    pub(super) async fn cancel(
        &self,
        agent: &AgentMetadata,
        call: &str,
        id: &str,
    ) -> Result<Value> {
        let resource = self.resource_lock(id).await;
        let _guard = resource.lock().await;
        let mut job = self.known_job(agent, id, true)?;
        let intent = job
            .cleanup_intent
            .get_or_insert_with(|| CleanupIntent {
                agent_id: agent.id.clone(),
                call_id: call.into(),
            })
            .clone();
        self.store.save_record(&job)?;
        let actor = require_in_run(&self.store, &intent.agent_id, &job.run_id)?;
        let raw = self
            .gateway
            .worker_call(
                &actor,
                &intent.call_id,
                Request::JobCancel(serde_json::from_value(
                    json!({"job_id":job.worker_resource_id}),
                )?),
            )
            .await?;
        self.present(&job, raw)
    }

    pub(super) async fn resource_lock(&self, job_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        if locks.len() >= 256 {
            locks.retain(|_, held| Arc::strong_count(held) > 1);
        }
        locks
            .entry(format!("resource:job:{job_id}"))
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    #[cfg(test)]
    pub(crate) async fn refresh(&self, run: &str) -> Result<()> {
        if self.gateway.worker.is_none() {
            return Ok(());
        }
        for job in self.store.records::<JobRecord>(run)? {
            if job.notification_message_id.is_some() {
                continue;
            }
            let owner: AgentMetadata = require_in_run(&self.store, &job.agent_id, run)?;
            self.observe(&owner, &format!("job-observation:{}", job.id), &job.id)
                .await?;
            if self.store.load::<JobRecord>(&job.id)?.status.settled() {
                self.import_job_artifacts(&owner, &format!("job-sync:{}", job.id), &job.id)
                    .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn observe(
        &self,
        agent: &AgentMetadata,
        call: &str,
        job_id: &str,
    ) -> Result<Value> {
        let lock = self.resource_lock(job_id).await;
        let _guard = lock.lock().await;
        let job = self.known_job(agent, job_id, false)?;
        let status: JobStatus = if job.status.settled() {
            job.status.clone()
        } else {
            serde_json::from_value(
                tokio::time::timeout(
                    Duration::from_secs(10),
                    self.gateway
                        .worker_query(Request::JobStatus(serde_json::from_value(
                            json!({"job_id":job.worker_resource_id}),
                        )?)),
                )
                .await
                .context("job status timed out")??,
            )?
        };
        ensure!(
            status.job_id == job.worker_resource_id,
            "worker returned another job identity"
        );
        ensure!(
            !job.status.state.terminal() || status.state == job.status.state,
            "worker attempted to regress a terminal job"
        );
        let raw = serde_json::to_value(&status)?;
        // Original status receipts are small, local writes. File transfers have
        // a separate owner and never run while this resource lock is held.
        let terminal = status.state.terminal();
        let execution_evidence = if terminal
            && job.execution_status_evidence_id.is_none()
            && job.notification_message_id.is_none()
        {
            Some(
                self.store
                    .artifact(
                        &agent.run_id,
                        &agent.id,
                        call,
                        "job.execution_status",
                        &serde_json::to_vec(&raw)?,
                        "application/json",
                        json!({"job_id":job_id}),
                    )?
                    .id,
            )
        } else {
            job.execution_status_evidence_id.clone()
        };
        let final_evidence = if status.settled() && job.status_evidence_id.is_none() {
            Some(
                self.store
                    .artifact(
                        &agent.run_id,
                        &agent.id,
                        call,
                        "job.raw_status",
                        &serde_json::to_vec(&raw)?,
                        "application/json",
                        json!({"job_id":job_id}),
                    )?
                    .id,
            )
        } else {
            job.status_evidence_id.clone()
        };
        self.store.write(|tx| {
            let mut current: JobRecord = tx.get_record(job_id)?.context("job missing")?;
            Self::check_identity(&job, &current)?;
            current.status = status.clone();
            current.execution_status_evidence_id = execution_evidence;
            current.status_evidence_id = final_evidence;
            current.updated_at = now();
            if terminal
                && current.notification_message_id.is_none()
                && current.execution_notification_message_id.is_none()
            {
                Self::notify(tx, &mut current, false)?;
            }
            tx.save_record(&current)
        })?;
        if let Some(crashes) = raw["crashes"].as_array() {
            for artifact in crashes {
                self.files.register_file(agent, artifact, None)?;
            }
        }
        let current = self.store.load::<JobRecord>(job_id)?;
        self.present(&current, raw)
    }

    pub(super) fn check_identity(expected: &JobRecord, current: &JobRecord) -> Result<()> {
        ensure!(
            expected.worker_id == current.worker_id
                && expected.worker_resource_id == current.worker_resource_id
                && expected.run_id == current.run_id
                && expected.agent_id == current.agent_id
                && expected.owner_generation == current.owner_generation,
            "job identity changed during observation"
        );
        Ok(())
    }

    pub(super) fn notify(
        tx: &crate::storage::StateSession<'_>,
        job: &mut JobRecord,
        completed: bool,
    ) -> Result<()> {
        if (completed && job.notification_message_id.is_some())
            || (!completed
                && (job.execution_notification_message_id.is_some()
                    || job.notification_message_id.is_some()))
        {
            return Ok(());
        }
        let owner = tx
            .get_record::<AgentMetadata>(&job.agent_id)?
            .context("job owner missing")?;
        let stale = job.owner_generation != owner.generation;
        let evidence = if completed {
            &job.status_evidence_id
        } else {
            &job.execution_status_evidence_id
        };
        let envelope = MessageEnvelope {
            options: MessageOptions {
                kind: MessageKind::Information,
                recipient_generation: Some(job.owner_generation),
                evidence_ids: vec![evidence.clone().context("job status evidence missing")?],
                ..Default::default()
            },
            task_id: job.agent_id.clone(),
            delivery: if stale {
                MessageDelivery::Stale
            } else {
                MessageDelivery::Queued
            },
            stale_reason: stale.then(|| "job notification belongs to an ended generation".into()),
            ..Default::default()
        };
        let kind = if completed {
            "job_completed"
        } else {
            "job_execution_finished"
        };
        let result = job.status.result.as_ref();
        let artifacts = job
            .imported_artifacts
            .as_ref()
            .and_then(|value| value["artifacts"].as_array());
        let brief = json!({"type":kind,"job_id":job.id,"tool":job.tool_name,"state":job.status.state,
            "exit_code":result.and_then(|r|r.exit_code),"passed":result.and_then(|r|r.passed),
            "stdout":job.status.stdout.chars().take(400).collect::<String>(),
            "stderr":job.status.stderr.chars().take(200).collect::<String>(),
            "evidence_id":evidence,"cleanup_confirmed":job.status.settled(),
            "artifact_count":artifacts.map_or(0,Vec::len),
            "artifact_evidence_ids":artifacts.into_iter().flatten().take(8).filter_map(|file|file["evidence_id"].as_str()).collect::<Vec<_>>(),
            "evidence_sync":job.evidence_sync.progress(job.evidence_ready()),
            "note":if completed {"Evidence and postprocessing are ready; inspect the result before claiming success."}
                else {"Execution ended. Evidence or cleanup may be pending; this is not business success."}});
        let key = if completed {
            format!("job-completed:{}", job.id)
        } else {
            format!("job-execution-finished:{}", job.id)
        };
        let (mid, _) = tx.enqueue_message(
            &job.run_id,
            "worker",
            &job.agent_id,
            &serde_json::to_string(&brief)?,
            Some(&key),
            &envelope,
        )?;
        if completed {
            job.notification_message_id = Some(mid);
        } else {
            job.execution_notification_message_id = Some(mid);
        }
        Ok(())
    }

    pub(super) fn present(&self, job: &JobRecord, raw: Value) -> Result<Value> {
        let mut status: JobStatus = serde_json::from_value(raw)?;
        ensure!(
            status.job_id == job.worker_resource_id,
            "worker returned another job identity"
        );
        status
            .extra
            .insert("worker_job_id".into(), json!(status.job_id));
        status.extra.insert(
            "evidence_sync".into(),
            job.evidence_sync.progress(job.evidence_ready()),
        );
        status.job_id = job.id.clone();
        if let Some(validation) = self.store.get_record::<ArtifactValidation>(&job.id)? {
            status.extra.insert("artifact_validation".into(),json!({"validation_id":validation.id,
                "settled":validation.status_evidence_id.is_some(),"passed":validation.passed,"details":validation.result}));
        }
        Ok(serde_json::to_value(status)?)
    }

    pub(super) fn known_job(
        &self,
        agent: &AgentMetadata,
        job_id: &str,
        owned: bool,
    ) -> Result<JobRecord> {
        let job: JobRecord = require_in_run(&self.store, job_id, &agent.run_id)?;
        ensure!(
            job.worker_id == self.gateway.worker_identity()?,
            "job belongs to a different runtime configuration"
        );
        ensure!(
            !owned || job.agent_id == agent.id,
            "this operation requires the job's owning agent"
        );
        Ok(job)
    }

    pub(super) async fn import_job_artifacts(
        &self,
        agent: &AgentMetadata,
        call: &str,
        job_id: &str,
    ) -> Result<Value> {
        evidence_sync::EvidenceSyncService::new(self.clone())
            .sync(agent, call, job_id)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn automatic_and_manual_observations_share_evidence_and_nested_results() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path().join("state"))?;
        let target = dir.path().join("sample");
        fs::write(&target, b"fixture")?;
        let coordinator = Coordinator::new(store.clone());
        let config = AppConfig {
            default_environment: Some("fixture".into()),
            environments: std::collections::BTreeMap::from([(
                "fixture".into(),
                crate::adapters::environments::EnvironmentConfig {
                    name: "Trusted fixture".into(),
                    platform: crate::adapters::environments::Platform::current(),
                    installation_id: None,
                    runtime: crate::adapters::worker::RuntimeConfig {
                        transport: crate::adapters::worker::TransportConfig::TrustedLocal(
                            crate::adapters::worker::TrustedLocalConfig {
                                state_dir: dir.path().join("worker"),
                                ..Default::default()
                            },
                        ),
                        ..Default::default()
                    },
                },
            )]),
            ..Default::default()
        };
        let run = coordinator.create_run(
            &target,
            "fixture",
            Budget::default(),
            serde_json::to_value(&config)?,
        )?;
        let owner = coordinator
            .create_child(
                &run.root_id,
                serde_json::from_value(
                    json!({"name":"worker","role":"analyst","task":"fixture","scope":"fixture"}),
                )?,
            )?
            .agent
            .metadata;
        let service = ToolService::new(config, store.clone()).await?;
        let tools = service.bound_to(&owner).await?;
        let started = service
            .execute(
                &owner.id,
                "start",
                "job_start",
                json!({"argv":["python","-c","import sys;print('observation');sys.exit(7)"]}),
            )
            .await?;
        assert!(!started.is_error, "{started:?}");
        let job_id = text(&started.value["result"], "job_id")?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tools.jobs().refresh(&run.id).await?;
                if store.require::<Value>("job", job_id)?["notification_message_id"].is_i64() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .with_context(|| {
            format!(
                "job completion timed out: {:?}",
                store.load::<JobRecord>(job_id)
            )
        })??;
        let automatic: Value = store.require("job", job_id)?;
        assert_eq!(automatic["status"]["result"]["exit_code"], 7);
        assert!(
            automatic["imported_artifacts"]["artifacts"]
                .as_array()
                .is_some_and(|artifacts| !artifacts.is_empty())
        );
        store.read_evidence(&run.id, text(&automatic, "status_evidence_id")?)?;
        let manual = tools.jobs().observe(&owner, "manual", job_id).await?;
        assert_eq!(manual["job_id"], job_id);
        assert_eq!(manual["worker_job_id"], automatic["status"]["job_id"]);
        assert_eq!(manual["result"], automatic["status"]["result"]);
        let original: Value = serde_json::from_slice(
            &store.read_evidence(&run.id, text(&automatic, "status_evidence_id")?)?,
        )?;
        assert_eq!(original, automatic["status"]);
        assert!(original.get("worker_job_id").is_none());
        assert_eq!(
            store.require::<Value>("job", job_id)?["status_evidence_id"],
            automatic["status_evidence_id"]
        );
        let before_replay: Value = store.require("job", job_id)?;
        store.write(|tx| {
            let mut call: ToolCallRecord = tx.get("call", "start")?.unwrap();
            call.state = CallState::Running;
            call.result = None;
            tx.put("call", "start", &run.id, &call)
        })?;
        let replay = service
            .execute(
                &owner.id,
                "start",
                "job_start",
                json!({"argv":["python","-c","import sys;print('observation');sys.exit(7)"]}),
            )
            .await?;
        assert!(!replay.is_error, "{replay:?}");
        assert_eq!(
            store.require::<Value>("job", job_id)?,
            before_replay,
            "recovery cannot overwrite observations or delivery state with the original launch response"
        );
        let inbox = store.inbox(&owner.id)?;
        assert_eq!(
            inbox.len(),
            2,
            "automatic and manual observations enqueue each phase once"
        );
        let early: Value = serde_json::from_str(&inbox[0].content)?;
        assert_eq!(early["type"], "job_execution_finished");
        let brief: Value = serde_json::from_str(&inbox[1].content)?;
        assert_eq!(brief["type"], "job_completed");
        assert_eq!(brief["exit_code"], 7);
        assert_eq!(brief["evidence_id"], automatic["status_evidence_id"]);
        let reopened = Store::open(store.root())?;
        assert_eq!(
            reopened.inbox(&owner.id)?[0].id,
            inbox[0].id,
            "a restart preserves pending delivery"
        );
        reopened.checkpoint_and_ack_generation(
            &owner.id,
            json!({"fixture":true}),
            &inbox.iter().map(|message| message.id).collect::<Vec<_>>(),
            Some(owner.generation),
        )?;
        assert!(store.inbox(&owner.id)?.is_empty());
        let mut next = owner.clone();
        next.generation += 1;
        store.write(|tx| tx.put_metadata("agent", &next.id, &next.run_id, &next))?;
        let mut lost = automatic;
        lost["id"] = json!("lost");
        lost["worker_resource_id"] = json!("lost");
        lost["status"]["job_id"] = json!("lost");
        lost["status"]["state"] = json!("lost");
        lost.as_object_mut()
            .unwrap()
            .remove("notification_message_id");
        store.put("job", "lost", &run.id, &lost)?;
        tools.jobs().refresh(&run.id).await?;
        assert!(
            store.inbox(&owner.id)?.is_empty(),
            "old generation jobs cannot steer new work"
        );
        let messages = store.read(|tx| tx.messages(&run.id))?;
        let stale = messages
            .iter()
            .find(|mail| {
                serde_json::from_str::<Value>(&mail.content)
                    .is_ok_and(|value| value["job_id"] == "lost")
            })
            .unwrap();
        assert_eq!(stale.envelope.delivery, MessageDelivery::Stale);
        assert!(stale.acknowledged);
        tools.runtime()?.call("worker.shutdown", json!({})).await?;
        service.close().await;
        Ok(())
    }
}
