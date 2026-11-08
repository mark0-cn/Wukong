//! Jobs tool operations.
use super::*;
use crate::runtime::protocol::{JobState, JobStatus};

pub(crate) struct JobService<'a> {
    tools: &'a ToolService,
}

impl JobService<'_> {
    pub(crate) async fn refresh(&self, run: &str) -> Result<()> {
        if self.tools.runtime.is_none() {
            return Ok(());
        }
        for job in self.tools.store.list::<Value>("job", run)? {
            if job["notification_message_id"].is_i64() {
                continue;
            }
            let owner: AgentMetadata =
                self.tools
                    .require_in_run("agent", text(&job, "agent_id")?, run)?;
            let job_id = text(&job, "id")?;
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
            let mut locks = self.tools.locks.lock().await;
            if locks.len() >= 256 {
                locks.retain(|_, held| Arc::strong_count(held) > 1);
            }
            locks
                .entry(format!("resource:job:{job_id}"))
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        let job = self.tools.known_job(agent, job_id, false)?;
        let cached = serde_json::from_value::<JobStatus>(job["status"].clone()).ok();
        let status = if let Some(status) = cached.filter(|status| status.state.terminal()) {
            status
        } else {
            self.tools.runtime()?.job_status(job_id).await?
        };
        let raw = serde_json::to_value(&status)?;
        self.tools.store.write(|tx| {
            let mut current: Value = tx.get("job", job_id)?.context("job missing")?;
            current["status"] = raw.clone();
            current["updated_at"] = json!(now());
            tx.put("job", job_id, &agent.run_id, &current)
        })?;
        if let Some(crashes) = raw["crashes"].as_array() {
            for artifact in crashes {
                self.tools.register_file(agent, artifact, None)?;
                if let Some(path) = artifact["path"].as_str() {
                    let existing = self.tools.known_file(agent, path, false)?;
                    if existing["evidence_id"].is_null() {
                        let bytes = self
                            .tools
                            .runtime()?
                            .call("file.get", json!({"path":path,"max_bytes":32*1024*1024}))
                            .await?;
                        self.tools.import_file(agent, call, &bytes)?;
                    }
                }
            }
        }
        if status.state.terminal() {
            let evidence = if job["status_evidence_id"].is_string() {
                job["status_evidence_id"].clone()
            } else {
                json!(
                    self.tools
                        .store
                        .artifact(
                            &agent.run_id,
                            &agent.id,
                            call,
                            "job.raw_status",
                            &serde_json::to_vec(&raw)?,
                            "application/json",
                            json!({"job_id":job_id})
                        )?
                        .id
                )
            };
            self.tools.store.write(|tx| {
                let mut current: Value = tx.get("job", job_id)?.context("job missing")?;
                current["status_evidence_id"] = evidence;
                tx.put("job", job_id, &agent.run_id, &current)
            })?;
            self.tools.import_job_artifacts(agent, call, job_id).await?;
            self.tools.store.write(|tx| {
                let mut current: Value = tx.get("job", job_id)?.context("job missing")?;
                if current["notification_message_id"].is_i64() {
                    return Ok(());
                }
                let owner_id = text(&current, "agent_id")?;
                let owner = tx
                    .get_metadata("agent", owner_id)?
                    .context("job owner missing")?;
                let generation = current["owner_generation"]
                    .as_u64()
                    .context("job owner generation missing")?;
                let stale = generation != owner.generation;
                let envelope = MessageEnvelope {
                    options: MessageOptions {
                        kind: MessageKind::Information,
                        recipient_generation: Some(generation),
                        evidence_ids: vec![text(&current, "status_evidence_id")?.into()],
                        ..Default::default()
                    },
                    task_id: owner_id.into(),
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
                current["notification_message_id"] = json!(mid);
                tx.put("job", job_id, &agent.run_id, &current)
            })?;
        }
        Ok(raw)
    }

    fn brief(job: &Value, status: &JobStatus) -> Value {
        let result = status.result.as_ref();
        let artifacts = job["imported_artifacts"]["artifacts"].as_array();
        json!({"type":"job_completed","job_id":status.job_id,"tool":job["tool_name"],"state":status.state,
            "exit_code":result.and_then(|result|result.exit_code),"passed":result.and_then(|result|result.passed),
            "stdout":status.stdout.chars().take(400).collect::<String>(),"stderr":status.stderr.chars().take(200).collect::<String>(),
            "evidence_id":job["status_evidence_id"],"artifact_evidence_ids":artifacts.map(|items|items.iter().take(8).map(|item|item["evidence_id"].clone()).collect::<Vec<_>>()),
            "artifacts_total":artifacts.map_or(0,Vec::len),
            "note":"Completed job observation; use evidence_read for the full status."})
    }
}

impl ToolService {
    pub(crate) fn jobs(&self) -> JobService<'_> {
        JobService { tools: self }
    }
    pub(super) fn known_job(
        &self,
        agent: &AgentMetadata,
        job_id: &str,
        owned: bool,
    ) -> Result<Value> {
        let job: Value = self.require_in_run("job", job_id, &agent.run_id)?;
        ensure!(
            job["worker_id"] == self.worker_identity()?,
            "job belongs to a different runtime configuration"
        );
        if owned {
            ensure!(
                job["agent_id"] == agent.id,
                "this operation requires the job's owning agent"
            );
        }
        Ok(job)
    }

    pub(super) async fn observe_job(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        job_id: &str,
    ) -> Result<Value> {
        self.jobs().observe(agent, call_id, job_id).await
    }

    pub(super) async fn import_job_artifacts(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        job_id: &str,
    ) -> Result<Value> {
        let mut job = self.known_job(agent, job_id, false)?;
        if let Some(imported) = job.get("imported_artifacts") {
            return Ok(imported.clone());
        }
        let artifacts = self
            .runtime()?
            .call("job.artifacts", json!({"job_id":job_id}))
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
            let evidence = self.import_file(agent, call_id, &raw)?;
            imported.push(json!({"artifact":artifact,"evidence_id":evidence.id}));
        }
        let result = json!({"job_id":job_id,"artifacts":imported});
        if serde_json::from_value::<JobState>(job["status"]["state"].clone())?.terminal() {
            self.store.write(|tx| {
                job = tx.get("job", job_id)?.context("job missing")?;
                job["imported_artifacts"] = result.clone();
                tx.put("job", job_id, &agent.run_id, &job)
            })?;
        }
        Ok(result)
    }

    pub(super) async fn verification(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        args: &Value,
    ) -> Result<Value> {
        let finding: Finding =
            self.require_in_run("finding", text(args, "finding_id")?, &agent.run_id)?;
        ensure!(
            finding.agent_id != agent.id,
            "verification must be independent of the finding producer"
        );
        let job_id = text(args, "job_id")?;
        let job = self.known_job(agent, job_id, true)?;
        ensure!(
            job["tool_name"] == "poc_verify" && job["finding_id"] == finding.id,
            "job is not an independent verification of this finding"
        );
        let status = self.observe_job(agent, call_id, job_id).await?;
        ensure!(
            status["state"] == "completed" && status["kind"] == "verify",
            "verification job has not completed successfully"
        );
        let result = &status["result"];
        let run: RunRecord = self.store.require("run", &agent.run_id)?;
        ensure!(
            result["target"]["sha256"] == run.target_sha256
                && job["target_sha256"] == run.target_sha256,
            "verification target SHA256 differs from finding target"
        );
        ensure!(
            result["verifier_agent_id"] == agent.id
                && result["producer_agent_id"] == finding.agent_id
                && result["call_id"] == job["worker_request_id"],
            "worker verification identity differs from its durable request"
        );
        ensure!(
            result["fresh_working_directory_per_trial"].as_bool() == Some(true),
            "verification did not use clean trial directories"
        );
        let attempts = result["attempts"]
            .as_u64()
            .context("worker attempt count missing")?;
        let trials = result["trials"]
            .as_u64()
            .context("worker trial count missing")?;
        let successes = result["successes"]
            .as_u64()
            .context("worker success count missing")?;
        let requested = result["requested_trials"]
            .as_u64()
            .context("worker requested trial count missing")?;
        ensure!(
            attempts > 0
                && attempts == trials
                && attempts <= 16
                && successes <= attempts
                && requested >= attempts,
            "worker trial counts are inconsistent"
        );
        let observations = result["results"]
            .as_array()
            .context("worker trial results missing")?;
        ensure!(
            observations.len() as u64 == attempts,
            "worker trial observations do not match attempt count"
        );
        let input = self.known_file(agent, text(&job["parameters"], "input")?, false)?;
        ensure!(
            result["input"]["sha256"] == input["sha256"]
                && observations
                    .iter()
                    .all(|trial| trial["input_sha256"] == input["sha256"]),
            "verification input identity mismatch"
        );
        let passed = result["passed"]
            .as_bool()
            .context("worker passed flag missing")?;
        ensure!(
            !passed || (successes == attempts && attempts == requested),
            "worker success verdict contradicts trial observations"
        );
        let raw_evidence = self.store.artifact(
            &agent.run_id,
            &agent.id,
            call_id,
            "verification.raw_job_status",
            &serde_json::to_vec(&status)?,
            "application/json",
            json!({"job_id":job_id,"finding_id":finding.id,"target_sha256":run.target_sha256}),
        )?;
        let artifacts = self.import_job_artifacts(agent, call_id, job_id).await?;
        let mut evidence_ids = vec![raw_evidence.id];
        for artifact in artifacts["artifacts"]
            .as_array()
            .context("artifact import missing")?
        {
            evidence_ids.push(text(artifact, "evidence_id")?.into());
        }
        let observed_stage = result["observed_stage"].as_str().unwrap_or("unknown");
        let impact = match observed_stage {
            "crash" => "reproducible crash",
            _ => "observed behavior matches the recorded expectation",
        };
        // These exact labels are deliberately limited to what the worker
        // observed. Analyst claims cannot turn a crash into code execution.
        let verification = Verification {
            id: stable(
                "verification",
                &format!("{}:{}:{job_id}", agent.run_id, finding.id),
            ),
            run_id: agent.run_id.clone(),
            finding_id: finding.id.clone(),
            verifier_agent_id: agent.id.clone(),
            passed,
            attempts,
            successes,
            impact: impact.into(),
            evidence_ids,
            environment: result["environment"].clone(),
            conditions: json!({"worker_parameters":job["parameters"],"expected":result["expected"],"analyst_conditions":args.get("conditions"),"analyst_impact_claim":args.get("impact"),"observed_stage":observed_stage,"job_id":job_id}),
            result: status,
            created_at: now(),
        };
        self.effect(agent,call_id,|tx|{
            if let Some(existing)=tx.get::<Verification>("verification",&verification.id)?{return Ok(serde_json::to_value(existing)?);}
            tx.put("verification",&verification.id,&agent.run_id,&verification)?;
            tx.event(&agent.run_id,Some(&agent.id),"verification.recorded",json!({"verification_id":verification.id,"finding_id":finding.id,"passed":passed,"attempts":attempts,"successes":successes,"impact":verification.impact}))?;
            Ok(serde_json::to_value(verification)?)
        })
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
        let run = coordinator.create_run(&target, "fixture", Budget::default(), json!({}))?;
        let owner: AgentMetadata = store.require("agent", &run.root_id)?;
        let mut config = AppConfig::default();
        config.runtime.backend = RuntimeBackend::TrustedLocal;
        config.runtime.state_dir = dir.path().join("worker");
        let tools = ToolService::new(config, store.clone()).await?;
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
        let manual = tools.observe_job(&owner, "manual", job_id).await?;
        assert_eq!(manual, automatic["status"]);
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
