//! Admission and evidence certification for independent verification.
use super::verification_protocol::{VerificationPlan, VerificationResult};
use super::*;

#[derive(Clone)]
pub(super) struct VerificationService {
    pub(super) jobs: jobs::JobService,
}

impl VerificationService {
    pub(super) async fn admit(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        run: &RunRecord,
        finding_id: Option<&str>,
        args: &mut Value,
    ) -> Result<()> {
        let finding: Finding = require_in_run(
            &self.jobs.store,
            finding_id.context("finding_id missing")?,
            &agent.run_id,
        )?;
        ensure!(
            finding.agent_id != agent.id,
            "the verifier must differ from the finding producer"
        );
        // Structural closure budget: at most two replay jobs per
        // (verifier, finding). The verdict then belongs in
        // verification_record, passed=true or a diagnosed false.
        let used = self
            .jobs
            .store
            .records::<JobRecord>(&agent.run_id)?
            .into_iter()
            .filter(|job| {
                job.tool_name == "poc_verify"
                    && job.agent_id == agent.id
                    && job.call_id != call_id
                    && job.finding_id.as_deref() == Some(finding.id.as_str())
            })
            .count();
        ensure!(
            used < 2,
            "poc_verify budget for this finding is exhausted ({used}/2 jobs used); record your verdict with verification_record now — passed=false with a diagnosis is a complete outcome"
        );
        args.as_object_mut()
            .context("arguments not an object")?
            .remove("finding_id");
        args["producer_agent_id"] = json!(finding.agent_id);
        args["verifier_agent_id"] = json!(agent.id);
        if args["network"].is_object() {
            self.jobs
                .files
                .known_file(agent, text(args, "target")?, true)?;
            ensure!(
                ["args", "env", "expected", "input_mode"]
                    .iter()
                    .all(|key| args[key].is_null()),
                "network replay cannot override local replay parameters"
            );
            let binding = self
                .jobs
                .gateway
                .worker_call(
                    agent,
                    &format!("{call_id}:network-profile"),
                    Request::NetworkProfile(serde_json::from_value(
                        json!({"target":args["target"],"launch_spec":args["launch_spec"]}),
                    )?),
                )
                .await?;
            ensure!(
                binding["target_sha256"] == run.target_sha256,
                "network profile target differs from this run"
            );
            args["network_binding"] = binding;
        }
        Ok(())
    }
    pub(super) async fn record(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        args: &Value,
    ) -> Result<Value> {
        let finding: Finding =
            require_in_run(&self.jobs.store, text(args, "finding_id")?, &agent.run_id)?;
        ensure!(
            finding.agent_id != agent.id,
            "verification must be independent of the finding producer"
        );
        let job_id = text(args, "job_id")?;
        let job = self.jobs.known_job(agent, job_id, true)?;
        ensure!(
            job.tool_name == "poc_verify" && job.finding_id.as_deref() == Some(finding.id.as_str()),
            "job is not an independent verification of this finding"
        );
        let status = self.jobs.observe(agent, call_id, job_id).await?;
        ensure!(
            status["state"] == "completed" && status["kind"] == "verify",
            "verification job has not completed successfully"
        );
        let result = &status["result"];
        let observed: VerificationResult =
            serde_json::from_value(result.clone()).context("invalid verification result")?;
        let run: RunRecord = self.jobs.store.load(&agent.run_id)?;
        ensure!(
            observed.target.sha256 == run.target_sha256 && job.target_sha256 == run.target_sha256,
            "verification target SHA256 differs from finding target"
        );
        ensure!(
            observed.verifier_agent_id == agent.id
                && observed.producer_agent_id == finding.agent_id
                && observed.call_id == job.worker_request_id,
            "worker verification identity differs from its durable request"
        );
        ensure!(
            observed.fresh_working_directory_per_trial,
            "verification did not use clean trial directories"
        );
        let attempts = observed.attempts;
        let trials = observed.trials;
        let successes = observed.successes;
        let requested = observed.requested_trials;
        ensure!(
            attempts > 0
                && attempts == trials
                && attempts <= 16
                && successes <= attempts
                && requested >= attempts,
            "worker trial counts are inconsistent"
        );
        let observations = &observed.results;
        ensure!(
            observations.len() as u64 == attempts,
            "worker trial observations do not match attempt count"
        );
        let input = self
            .jobs
            .files
            .known_file(agent, text(&job.parameters, "input")?, false)?;
        ensure!(
            observed.input.sha256 == input.sha256
                && observations
                    .iter()
                    .all(|trial| trial.input_sha256 == input.sha256),
            "verification input identity mismatch"
        );
        let passed = observed.passed;
        let plan = VerificationPlan::parse(&job.parameters)?;
        let network = matches!(plan, VerificationPlan::Network { .. });
        if let VerificationPlan::Network { steps, binding } = &plan {
            ensure!(
                observed.network_binding.as_ref() == Some(binding)
                    && binding.target_sha256 == run.target_sha256
                    && observed.expected == serde_json::to_value(steps)?
                    && observed.server_reset_per_trial == Some(false)
                    && observations
                        .iter()
                        .all(|trial| trial.network_binding.as_ref() == Some(binding))
                    && observations
                        .iter()
                        .filter(|trial| trial.network_success == Some(true))
                        .count() as u64
                        == successes,
                "network verification binding or observations differ from the admitted plan"
            );
            ensure!(
                !passed || observed.fresh_connection_per_trial == Some(true),
                "successful network trials must use fresh connections"
            );
        }
        ensure!(
            !passed || (successes == attempts && attempts == requested),
            "worker success verdict contradicts trial observations"
        );
        let raw_evidence = self.jobs.store.artifact(
            &agent.run_id,
            &agent.id,
            call_id,
            "verification.raw_job_status",
            &serde_json::to_vec(&status)?,
            "application/json",
            json!({"job_id":job_id,"finding_id":finding.id,"target_sha256":run.target_sha256}),
        )?;
        let artifacts = self
            .jobs
            .import_job_artifacts(agent, call_id, job_id)
            .await?;
        if network {
            let imported = artifacts["artifacts"]
                .as_array()
                .context("network artifacts missing")?;
            ensure!(
                observations.iter().all(|trial| {
                    trial.transcript_path.is_some()
                        && trial.transcript_sha256.is_some()
                        && imported.iter().any(|item| {
                            item["artifact"]["path"].as_str() == trial.transcript_path.as_deref()
                                && item["artifact"]["sha256"].as_str()
                                    == trial.transcript_sha256.as_deref()
                        })
                }),
                "network transcript identity was not preserved in imported evidence"
            );
        }
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
            "network_behavior" => {
                "observed network response matches the configured service plan; deployed runtime identity is unconfirmed"
            }
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
            conditions: json!({"worker_parameters":job.parameters,"expected":result["expected"],"analyst_conditions":args.get("conditions"),"analyst_impact_claim":args.get("impact"),"observed_stage":observed_stage,"job_id":job_id}),
            result: status,
            created_at: now(),
        };
        DurableEffects::new(&self.jobs.store,self.jobs.gateway.output_limit).commit(agent,call_id,|tx|{
            if let Some(existing)=tx.get_record::<Verification>(&verification.id)?{return Ok(serde_json::to_value(existing)?);}
            tx.save_record(&verification)?;
            tx.event(&agent.run_id,Some(&agent.id),"verification.recorded",json!({"verification_id":verification.id,"finding_id":finding.id,"passed":passed,"attempts":attempts,"successes":successes,"impact":verification.impact}))?;
            Ok(serde_json::to_value(verification)?)
        })
    }
}
