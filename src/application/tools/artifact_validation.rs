//! Independent, hash-bound acceptance of generated script predicates.
use super::*;

const DRIVER: &str = include_str!("../../../workers/runtime/wukong_worker/execution/validator.py");

pub(super) async fn start(
    tools: &ExecutionService,
    agent: &AgentMetadata,
    call: &str,
    input: args::ArtifactValidateArgs,
) -> Result<Value> {
    ensure!(
        agent.role == Role::Verifier,
        "artifact_validate requires an independent verifier"
    );
    ensure!(
        tools.gateway.runtime()?.doctor().await?["os"] == "linux",
        "self_check_v1 execution currently requires a configured Linux worker; report other platforms as an explicit gap"
    );
    ensure!(
        ["python3", "bash"].contains(&input.interpreter.as_str()),
        "self_check_v1 currently supports python3 and bash interpreters"
    );
    let artifact: Evidence = tools.store.load(&input.artifact_evidence_id)?;
    let baseline: Evidence = tools.store.load(&input.baseline_evidence_id)?;
    let job = tools.jobs.known_job(agent, &input.baseline_job_id, true)?;
    let run: RunRecord = tools.store.load(&agent.run_id)?;
    ensure!(
        artifact.size <= 1024 * 1024 && baseline.size <= 1024 * 1024,
        "self_check_v1 script and baseline are bounded to 1 MiB each"
    );
    let verifications: Vec<Value> = tools.store.list("analysis_verification", &agent.run_id)?;
    ensure!(
        verifications.iter().any(|v| v["verifier_id"] == agent.id
            && v["target_sha256"] == run.target_sha256
            && v["result"]["checks"]
                .as_array()
                .is_some_and(|checks| checks.iter().any(|check| check["kind"]
                    == "validator_integration"
                    && check["verdict"] == "supported"
                    && check["artifact_evidence_id"] == artifact.id
                    && check["reproduction_job_id"] == job.id))),
        "record a supported analysis_verify kind=validator_integration for this artifact and baseline job after inspecting collector/evaluator wiring"
    );
    ensure!(
        artifact.run_id == agent.run_id
            && baseline.run_id == agent.run_id
            && baseline.agent_id == agent.id
            && artifact.agent_id != agent.id,
        "use a same-run producer artifact and verifier-owned baseline"
    );
    ensure!(
        job.target_sha256 == run.target_sha256
            && job.status.settled()
            && job.status.state == JobState::Completed
            && job.status.result.as_ref().and_then(|r| r.exit_code) == Some(0),
        "baseline must come from a successful verifier same-target job"
    );
    ensure!(
        job.imported_artifacts
            .as_ref()
            .and_then(|v| v["artifacts"].as_array())
            .is_some_and(|items| items.iter().any(|item| item["evidence_id"] == baseline.id)),
        "baseline_evidence_id must be an imported artifact of baseline_job_id"
    );
    let execution = job
        .status
        .result
        .as_ref()
        .context("baseline job result missing")?;
    ensure!(
        execution
            .extra
            .get("execution")
            .and_then(|v| v["inputs"].as_array())
            .is_some_and(|items| items.iter().any(|v| v["source_sha256"] == artifact.sha256)),
        "baseline job must execute the exact submitted artifact bytes"
    );
    ensure!(
        input.arguments.len() <= 32 && input.arguments.iter().all(|v| v.len() <= 4096),
        "too many or oversized script arguments"
    );
    let original_argv = job.parameters["argv"]
        .as_array()
        .context("baseline must be the documented raw argv reproduction")?;
    ensure!(
        original_argv.len() >= 2
            && original_argv[0] == input.interpreter.as_str()
            && original_argv[2..]
                == input
                    .arguments
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect::<Vec<_>>(),
        "interpreter and arguments must match baseline_job_id's recorded invocation"
    );
    let original_script = original_argv[1]
        .as_str()
        .context("baseline script path missing")?;
    ensure!(
        tools
            .files
            .known_file(agent, original_script, false)?
            .sha256
            == artifact.sha256,
        "baseline argv[1] must be the submitted script"
    );
    for argument in &input.arguments {
        if argument.starts_with("files/") {
            tools.files.known_file(agent, argument, false)?;
        }
    }
    let script = tools
        .files
        .file_put(
            agent,
            &format!("{call}:script"),
            &json!({"path":"validator-script","path_policy":"versioned","evidence_id":artifact.id}),
        )
        .await?;
    let staged = tools
        .files
        .file_put(
            agent,
            &format!("{call}:baseline"),
            &json!({"path":"baseline.json","path_policy":"versioned","evidence_id":baseline.id}),
        )
        .await?;
    let mut argv = vec![
        json!(input.interpreter),
        script["file"]["path"].clone(),
        staged["file"]["path"].clone(),
    ];
    argv.extend(input.arguments.into_iter().map(Value::String));
    let result = tools
        .runtime_tool(
            agent,
            &format!("{call}:driver"),
            "py_run",
            json!({"source":DRIVER,"argv":argv,"timeout_secs":900,"output_limit":1048576}),
        )
        .await?;
    let job_id = text(&result, "job_id")?.to_owned();
    let record = ArtifactValidation {
        id: job_id.clone(),
        job_id,
        run_id: agent.run_id.clone(),
        verifier_id: agent.id.clone(),
        worker_id: job.worker_id,
        target_sha256: run.target_sha256,
        artifact_evidence_id: artifact.id,
        artifact_sha256: artifact.sha256,
        baseline_evidence_id: baseline.id,
        baseline_sha256: baseline.sha256,
        baseline_job_id: job.id,
        passed: false,
        status_evidence_id: None,
        result: None,
    };
    tools.store.write(|tx| {
        if tx.get_record::<ArtifactValidation>(&record.id)?.is_none() {
            tx.save_record(&record)?;
        }
        Ok(())
    })?;
    Ok(
        json!({"job":result,"validation_id":record.id,"note":"Pending. Observe the job; only the trusted driver result can satisfy self_check_v1."}),
    )
}

pub(super) fn observe(store: &Store, job: &JobRecord) -> Result<()> {
    let Some(mut record) = store.get_record::<ArtifactValidation>(&job.id)? else {
        return Ok(());
    };
    if !job.status.settled() || record.status_evidence_id.is_some() {
        return Ok(());
    }
    let result = job.status.result.as_ref();
    let report_evidence = job
        .imported_artifacts
        .as_ref()
        .and_then(|v| v["artifacts"].as_array())
        .and_then(|items| {
            items.iter().find(|v| {
                v["artifact"]["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("/validation-report.json"))
            })
        })
        .and_then(|v| v["evidence_id"].as_str());
    let report = report_evidence
        .map(|id| store.read_evidence(&job.run_id, id))
        .transpose()?
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    record.passed = job.status.state == JobState::Completed
        && result.and_then(|r| r.exit_code) == Some(0)
        && report.as_ref().is_some_and(|v| {
            v["passed"] == true
                && v["profile"] == "self_check_v1"
                && v["artifact_sha256"] == record.artifact_sha256
                && v["baseline_sha256"] == record.baseline_sha256
                && v["cases"].as_array().is_some_and(|cases| {
                    !cases.is_empty() && cases.iter().all(|c| c["passed"] == true)
                })
        });
    record.result = report.as_ref().map(|report|json!({"report_evidence_id":report_evidence,
        "cases_count":report["cases"].as_array().map_or(0,Vec::len),"error":report["error"],
        "failed_scenarios":report["cases"].as_array().map(|cases|cases.iter().filter(|v|v["passed"] != true).take(8).map(|v|v["scenario"].clone()).collect::<Vec<_>>())}));
    record.status_evidence_id = job.status_evidence_id.clone();
    store.write(|tx| {
        tx.save_record(&record)?;
        tx.event(
            &job.run_id,
            Some(&job.agent_id),
            "artifact.validated",
            json!(record),
        )?;
        Ok(())
    })
}
