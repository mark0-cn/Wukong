//! Claim-level provenance and reproduction admission for general analysis.
use super::*;

pub(super) fn validate(
    tx: &crate::storage::StateSession<'_>,
    agent: &AgentMetadata,
    run: &RunRecord,
    input: &args::AnalysisVerifyArgs,
) -> Result<Value> {
    ensure!(
        (1..=64).contains(&input.checks.len()),
        "verification requires 1..64 explicit checks"
    );
    ensure!(
        input.limitations.len() <= 32
            && input
                .limitations
                .iter()
                .all(|s| !s.trim().is_empty() && s.len() <= 4096),
        "invalid verification limitations"
    );
    let mut bindings = Vec::new();
    let mut errors = Vec::new();
    for (index, check) in input.checks.iter().enumerate() {
        let binding = (|| -> Result<Value> {
            for text in [&check.claim, &check.expected, &check.observed] {
                ensure!(
                    !text.trim().is_empty() && text.len() <= 16384,
                    "invalid check text"
                );
            }
            ensure!(
                !check.evidence_ids.is_empty() && check.evidence_ids.len() <= 128,
                "each check requires evidence"
            );
            let mut owned = false;
            for id in &check.evidence_ids {
                ensure!(
                    input.evidence_ids.contains(id),
                    "check evidence must appear in verification evidence_ids"
                );
                let evidence: Evidence = tx.get_record(id)?.context("check evidence missing")?;
                ensure!(evidence.run_id == agent.run_id, "cross-run check evidence");
                owned |= evidence.agent_id == agent.id;
            }
            ensure!(owned, "each check requires verifier-owned observations");
            if matches!(input.verdict, args::AnalysisVerdict::Supported) {
                ensure!(
                    matches!(check.verdict, args::AnalysisVerdict::Supported),
                    "supported verdict cannot hide a refuted or inconclusive check; narrow the claim or change verdict"
                );
            }
            let artifact = if let Some(id) = &check.artifact_evidence_id {
                ensure!(
                    input.evidence_ids.contains(id),
                    "checked artifact must appear in evidence_ids"
                );
                let evidence: Evidence = tx.get_record(id)?.context("checked artifact missing")?;
                ensure!(
                    evidence.run_id == agent.run_id,
                    "cross-run checked artifact"
                );
                Some(evidence)
            } else {
                None
            };
            if matches!(
                check.kind,
                args::AnalysisCheckKind::Reproduction
                    | args::AnalysisCheckKind::ValidatorIntegration
            ) {
                ensure!(
                    artifact.is_some()
                        && check.reproduction_job_id.is_some()
                        && check.expected_exit_code.is_some(),
                    "reproduction requires the exact artifact, verifier job and expected_exit_code"
                );
            }
            let job = if let Some(id) = &check.reproduction_job_id {
                let job: JobRecord = tx.get_record(id)?.context("reproduction job missing")?;
                ensure!(
                    job.run_id == agent.run_id
                        && job.agent_id == agent.id
                        && job.target_sha256 == run.target_sha256,
                    "reproduction must use the verifier's same-target job"
                );
                ensure!(
                    !matches!(job.status.state, JobState::Queued | JobState::Running),
                    "reproduction job is still active"
                );
                let evidence_id = job
                    .status_evidence_id
                    .as_ref()
                    .context("import reproduction status with job_status first")?;
                ensure!(
                    check.evidence_ids.contains(evidence_id),
                    "check must link original reproduction status evidence"
                );
                if matches!(check.verdict, args::AnalysisVerdict::Supported) {
                    ensure!(
                        job.status.settled() && job.evidence_ready(),
                        "supported reproduction requires ready evidence and confirmed cleanup"
                    );
                    ensure!(
                        job.status.state == JobState::Completed
                            || (job.status.state == JobState::Failed
                                && check.expected_exit_code.is_some_and(|code| code > 0)
                                && job
                                    .status
                                    .result
                                    .as_ref()
                                    .is_some_and(
                                        |r| r.extra.get("state") == Some(&json!("completed"))
                                    )),
                        "supported reproduction requires a normal process completion, including an explicitly expected nonzero exit"
                    );
                    if matches!(
                        check.kind,
                        args::AnalysisCheckKind::Reproduction
                            | args::AnalysisCheckKind::ValidatorIntegration
                    ) {
                        ensure!(
                            check.expected_exit_code.is_some_and(|code| code >= 0)
                                && job.status.result.as_ref().is_some_and(|r| r
                                    .extra
                                    .get("signal")
                                    .is_none_or(Value::is_null)),
                            "signals cannot satisfy a normal reproduction exit expectation"
                        );
                        ensure!(
                            job.status
                                .result
                                .as_ref()
                                .and_then(|result| result.exit_code)
                                == check.expected_exit_code,
                            "reproduction exit code differs from the declared expectation"
                        );
                    }
                }
                Some(
                    json!({"id":job.id,"parameters":job.parameters,"status_evidence_id":evidence_id}),
                )
            } else {
                None
            };
            Ok(json!({"claim":check.claim,"artifact":artifact,"reproduction":job}))
        })();
        match binding {
            Ok(value) => bindings.push(value),
            Err(error) => errors.push(format!("checks[{index}]: {error:#}")),
        }
    }
    ensure!(
        errors.is_empty(),
        "verification validation errors: {}",
        errors.join("; ")
    );
    Ok(
        json!({"applies_only_to":"result.checks", "whole_report_verified":false, "bindings":bindings}),
    )
}

pub(super) fn resolve_status_evidence(
    tx: &crate::storage::StateSession<'_>,
    agent: &AgentMetadata,
    input: &mut args::AnalysisVerifyArgs,
) -> Result<()> {
    for check in &mut input.checks {
        if let Some(id) = &check.reproduction_job_id
            && let Some(job) = tx.get_record::<JobRecord>(id)?
            && job.run_id == agent.run_id
            && job.agent_id == agent.id
            && let Some(evidence) = job.status_evidence_id
        {
            if !check.evidence_ids.contains(&evidence) {
                check.evidence_ids.push(evidence.clone());
            }
            if !input.evidence_ids.contains(&evidence) {
                input.evidence_ids.push(evidence);
            }
        }
    }
    Ok(())
}
