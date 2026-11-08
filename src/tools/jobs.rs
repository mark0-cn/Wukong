//! Jobs tool operations.
use super::*;

impl ToolService {
    pub(super) fn known_job(
        &self,
        agent: &AgentRecord,
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
        agent: &AgentRecord,
        call_id: &str,
        job_id: &str,
    ) -> Result<Value> {
        let mut job = self.known_job(agent, job_id, false)?;
        let status = self
            .runtime()?
            .call("job.status", json!({"job_id":job_id}))
            .await?;
        ensure!(status["job_id"] == job_id, "worker returned the wrong job");
        job["status"] = status.clone();
        job["updated_at"] = json!(now());
        self.store.put("job", job_id, &agent.run_id, &job)?;
        if let Some(crashes) = status["crashes"].as_array() {
            for artifact in crashes {
                self.register_file(agent, artifact, None)?;
                if let Some(path) = artifact["path"].as_str() {
                    let existing = self.known_file(agent, path, false)?;
                    if existing["evidence_id"].is_null() {
                        let raw = self
                            .runtime()?
                            .call("file.get", json!({"path":path,"max_bytes":32*1024*1024}))
                            .await?;
                        self.import_file(agent, call_id, &raw)?;
                    }
                }
            }
        }
        if ["completed", "failed", "cancelled", "timed_out", "lost"]
            .contains(&status["state"].as_str().unwrap_or_default())
        {
            self.import_job_artifacts(agent, call_id, job_id).await?;
        }
        Ok(status)
    }

    pub(super) async fn import_job_artifacts(
        &self,
        agent: &AgentRecord,
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
        if ["completed", "failed", "cancelled", "timed_out", "lost"]
            .contains(&job["status"]["state"].as_str().unwrap_or_default())
        {
            job["imported_artifacts"] = result.clone();
            self.store.put("job", job_id, &agent.run_id, &job)?;
        }
        Ok(result)
    }

    pub(super) async fn verification(
        &self,
        agent: &AgentRecord,
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
            if let Some(existing)=Store::tx_get::<Verification>(tx,"verification",&verification.id)?{return Ok(serde_json::to_value(existing)?);}
            Store::tx_put(tx,"verification",&verification.id,&agent.run_id,&verification)?;
            Store::tx_event(tx,&agent.run_id,Some(&agent.id),"verification.recorded",json!({"verification_id":verification.id,"finding_id":finding.id,"passed":passed,"attempts":attempts,"successes":successes,"impact":verification.impact}))?;
            Ok(serde_json::to_value(verification)?)
        })
    }
}
