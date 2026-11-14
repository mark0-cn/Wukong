//! Jobs tool operations.
use super::*;
use crate::runtime::protocol::JobStatus;

#[derive(Clone)]
pub(crate) struct JobService {
    pub(super) store: Store,
    pub(super) gateway: WorkerGateway,
    pub(super) files: FilesService,
    pub(super) locks: ResourceLocks,
}

impl JobService {
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
            let job_id = &job.id;
            self.observe(&owner, &format!("job-observation:{job_id}"), job_id)
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn observe(
        &self,
        agent: &AgentMetadata,
        call: &str,
        job_id: &str,
    ) -> Result<Value> {
        // Manual status calls and automatic observations share resource
        // serialization, evidence import and the same transactional update.
        let lock = {
            let mut locks = self.locks.lock().await;
            if locks.len() >= 256 {
                locks.retain(|_, held| Arc::strong_count(held) > 1);
            }
            locks
                .entry(format!("resource:job:{job_id}"))
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        let job = self.known_job(agent, job_id, false)?;
        let cached = Some(job.status.clone());
        let status = if let Some(status) = cached.filter(|status| status.state.terminal()) {
            status
        } else {
            serde_json::from_value(
                self.gateway
                    .worker_query(Request::JobStatus(serde_json::from_value(
                        json!({"job_id":job.worker_resource_id}),
                    )?))
                    .await?,
            )?
        };
        ensure!(
            status.job_id == job.worker_resource_id,
            "worker returned another job identity"
        );
        let raw = serde_json::to_value(&status)?;
        self.store.write(|tx| {
            let mut current: JobRecord = tx.get_record(job_id)?.context("job missing")?;
            current.status = status.clone();
            current.updated_at = now();
            tx.save_record(&current)
        })?;
        if let Some(crashes) = raw["crashes"].as_array() {
            for artifact in crashes {
                self.files.register_file(agent, artifact, None)?;
                if let Some(path) = artifact["path"].as_str() {
                    let existing = self.files.known_file(agent, path, false)?;
                    if existing.evidence_id.is_none() {
                        let bytes = self
                            .gateway
                            .runtime()?
                            .call("file.get", json!({"path":path,"max_bytes":32*1024*1024}))
                            .await?;
                        self.files.import_file(agent, call, &bytes)?;
                    }
                }
            }
        }
        if status.state.terminal() {
            let evidence = if let Some(evidence) = &job.status_evidence_id {
                evidence.clone()
            } else {
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
                    .id
            };
            self.store.write(|tx| {
                let mut current: JobRecord = tx.get_record(job_id)?.context("job missing")?;
                current.status_evidence_id = Some(evidence);
                tx.save_record(&current)
            })?;
            self.import_job_artifacts(agent, call, job_id).await?;
            self.store.write(|tx| {
                let mut current: JobRecord = tx.get_record(job_id)?.context("job missing")?;
                if current.notification_message_id.is_some() {
                    return Ok(());
                }
                let owner_id = &current.agent_id;
                let owner = tx
                    .get_record::<AgentMetadata>(owner_id)?
                    .context("job owner missing")?;
                let generation = current.owner_generation;
                let stale = generation != owner.generation;
                let envelope = MessageEnvelope {
                    options: MessageOptions {
                        kind: MessageKind::Information,
                        recipient_generation: Some(generation),
                        evidence_ids: vec![
                            current
                                .status_evidence_id
                                .clone()
                                .context("job status evidence missing")?,
                        ],
                        ..Default::default()
                    },
                    task_id: owner_id.clone(),
                    delivery: if stale {
                        MessageDelivery::Stale
                    } else {
                        MessageDelivery::Queued
                    },
                    stale_reason: stale
                        .then(|| "job completed after its owning generation ended".into()),
                    ..Default::default()
                };
                let brief = Self::brief(&current, &status);
                let (mid, _) = tx.enqueue_message(
                    &agent.run_id,
                    "worker",
                    owner_id,
                    &serde_json::to_string(&brief)?,
                    Some(&format!("job-completed:{job_id}")),
                    &envelope,
                )?;
                current.notification_message_id = Some(mid);
                tx.save_record(&current)
            })?;
        }
        self.present(&job, raw)
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
        status.job_id = job.id.clone();
        Ok(serde_json::to_value(status)?)
    }

    fn brief(job: &JobRecord, status: &JobStatus) -> Value {
        let result = status.result.as_ref();
        let artifacts = job
            .imported_artifacts
            .as_ref()
            .and_then(|value| value["artifacts"].as_array());
        json!({"type":"job_completed","job_id":job.id,"tool":job.tool_name,"state":status.state,
            "exit_code":result.and_then(|result|result.exit_code),"passed":result.and_then(|result|result.passed),
            "stdout":status.stdout.chars().take(400).collect::<String>(),"stderr":status.stderr.chars().take(200).collect::<String>(),
            "evidence_id":job.status_evidence_id,"artifact_evidence_ids":artifacts.map(|items|items.iter().take(8).map(|item|item["evidence_id"].clone()).collect::<Vec<_>>()),
            "artifacts_total":artifacts.map_or(0,Vec::len),
            "note":"Completed job observation; use evidence_read for the full status."})
    }
}

impl JobService {
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
        if owned {
            ensure!(
                job.agent_id == agent.id,
                "this operation requires the job's owning agent"
            );
        }
        Ok(job)
    }

    pub(super) async fn import_job_artifacts(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        job_id: &str,
    ) -> Result<Value> {
        let mut job = self.known_job(agent, job_id, false)?;
        if let Some(imported) = &job.imported_artifacts {
            return Ok(imported.clone());
        }
        let artifacts = self
            .gateway
            .worker_query(Request::JobArtifacts(
                wukong_worker_protocol::params::JobArtifactsArgs {
                    job_id: job.worker_resource_id.clone(),
                },
            ))
            .await?;
        let mut imported = Vec::new();
        for artifact in artifacts
            .as_array()
            .context("worker artifact list is not an array")?
        {
            ensure!(
                artifact["size"]
                    .as_u64()
                    .is_some_and(|size| size <= 32 * 1024 * 1024),
                "worker artifact exceeds 32 MiB import limit"
            );
            let raw = self
                .gateway
                .runtime()?
                .call(
                    "file.get",
                    json!({"path":artifact["path"],"max_bytes":32*1024*1024}),
                )
                .await?;
            ensure!(
                raw["sha256"] == artifact["sha256"],
                "job artifact changed while being imported"
            );
            let evidence = self.files.import_file(agent, call_id, &raw)?;
            imported.push(json!({"artifact":artifact,"evidence_id":evidence.id}));
        }
        let result = json!({"job_id":job_id,"artifacts":imported});
        if job.status.state.terminal() {
            self.store.write(|tx| {
                job = tx.get_record(job_id)?.context("job missing")?;
                job.imported_artifacts = Some(result.clone());
                tx.save_record(&job)
            })?;
        }
        Ok(result)
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
                crate::environments::EnvironmentConfig {
                    name: "Trusted fixture".into(),
                    platform: crate::environments::Platform::current(),
                    installation_id: None,
                    runtime: crate::runtime::RuntimeConfig {
                        transport: crate::runtime::TransportConfig::TrustedLocal(
                            crate::runtime::TrustedLocalConfig {
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
        let started = tools
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
        .await??;
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
        let replay = tools
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
            1,
            "automatic and manual observations enqueue one notice"
        );
        let brief: Value = serde_json::from_str(&inbox[0].content)?;
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
            &[inbox[0].id],
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
        tools.close().await;
        Ok(())
    }
}
