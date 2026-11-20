//! Project the existing worker job lifecycle onto its originating tool row.
//!
//! Execution, resource cleanup and the reason a finished job is not settled are
//! read from the durable `JobRecord` only: the desktop never infers cleanup from
//! the tool call result and never opens a second storage path.
use super::*;
use crate::adapters::worker::protocol::{CleanupState, OutcomeCertainty};

#[expect(
    clippy::cast_possible_truncation,
    reason = "a finite seconds value in chrono's range yields an i64 millisecond count; anything else returns None"
)]
fn timestamp(value: Option<&Value>) -> Option<chrono::DateTime<chrono::Utc>> {
    let seconds = value?.as_f64()?;
    if !seconds.is_finite() {
        return None;
    }
    chrono::DateTime::from_timestamp_millis((seconds * 1000.0) as i64)
}

/// Bounded worker diagnostics that explain an unresolved resource.
fn worker_detail(job: &JobRecord) -> Option<String> {
    ["error", "reconciliation"]
        .into_iter()
        .find_map(|key| job.status.extra.get(key).and_then(Value::as_str))
        .map(|detail| detail.trim())
        .filter(|detail| !detail.is_empty())
        .map(|detail| detail.chars().take(512).collect())
}

/// A finished execution does not imply cleanup; both must be shown separately.
fn blocking(job: &JobRecord, stale: bool) -> Option<JobBlockingDisplay> {
    if stale {
        return Some(JobBlockingDisplay {
            reason: UiMessage::new("progress-job-stale").into(),
            detail: None,
        });
    }
    let observation = job.status.observation();
    if !observation.terminal {
        return None;
    }
    if observation.outcome == OutcomeCertainty::Unknown {
        return Some(JobBlockingDisplay {
            reason: UiMessage::new("progress-job-blocked-outcome-unknown").into(),
            detail: worker_detail(job),
        });
    }
    if observation.cleanup != CleanupState::Confirmed {
        return Some(JobBlockingDisplay {
            reason: UiMessage::new("progress-job-blocked-cleanup").into(),
            detail: worker_detail(job),
        });
    }
    if !job.evidence_ready() {
        let reason = if job.evidence_sync.state == SyncState::Failed {
            "progress-job-blocked-evidence-failed"
        } else {
            "progress-job-blocked-evidence-pending"
        };
        return Some(JobBlockingDisplay {
            reason: UiMessage::new(reason).into(),
            detail: job
                .evidence_sync
                .error
                .clone()
                .or_else(|| worker_detail(job)),
        });
    }
    None
}

/// Link the evidence records the controller already references from the job.
fn evidence(job: &JobRecord) -> Vec<JobEvidenceDisplay> {
    let mut evidence = Vec::new();
    let execution = job
        .execution_status_evidence_id
        .clone()
        .or_else(|| job.status_evidence_id.clone());
    if let Some(evidence_id) = execution {
        evidence.push(JobEvidenceDisplay {
            role: JobEvidenceRole::Execution,
            evidence_id,
        });
    }
    if let Some(evidence_id) = job.status_evidence_id.clone() {
        evidence.push(JobEvidenceDisplay {
            role: JobEvidenceRole::Cleanup,
            evidence_id,
        });
    }
    let artifacts = job
        .imported_artifacts
        .as_ref()
        .and_then(|value| value["artifacts"].as_array())
        .map(|artifacts| {
            artifacts
                .iter()
                .filter_map(|file| file["evidence_id"].as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            job.evidence_sync
                .files
                .iter()
                .filter_map(|file| file.evidence_id.as_deref())
                .collect()
        });
    evidence.extend(
        artifacts
            .into_iter()
            .take(4)
            .map(|evidence_id| JobEvidenceDisplay {
                role: JobEvidenceRole::Artifact,
                evidence_id: evidence_id.to_owned(),
            }),
    );
    evidence
}

pub fn with_job(
    mut display: ToolDisplay,
    call: &ToolCallRecord,
    agent: Option<&AgentSummary>,
    jobs: &[JobRecord],
) -> ToolDisplay {
    let Some(job) = jobs.iter().find(|job| {
        job.call_id == call.id
            && job.agent_id == call.agent_id
            && job.run_id == call.run_id
            && job.tool_name == call.name
    }) else {
        return display;
    };
    let stale = agent.is_none_or(|agent| agent.generation != job.owner_generation);
    display.state = match job.status.state {
        JobState::Queued | JobState::Running if stale => "interrupted",
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Completed
            if job.status.result.as_ref().is_some_and(|result| {
                result.passed == Some(false)
                    || result.passed.is_none() && result.exit_code.is_some_and(|code| code != 0)
            }) =>
        {
            "failed"
        }
        JobState::Completed => "completed",
        JobState::Cancelled => "stopped",
        JobState::Failed | JobState::TimedOut => "failed",
        JobState::Lost => "uncertain",
    }
    .into();
    let started_at = timestamp(job.status.extra.get("started_at")).unwrap_or(job.created_at);
    let finished_at = timestamp(job.status.extra.get("finished_at"));
    display.duration_ms = finished_at.map(|end| (end - started_at).num_milliseconds().max(0));
    display.summary = if job.status.state.terminal() {
        let summary = result_summary(&serde_json::to_value(&job.status).unwrap_or_default());
        if summary.is_empty() {
            UiMessage::new(format!("result-{}", display.state)).into()
        } else {
            summary
        }
    } else {
        UiMessage::new(if stale {
            "progress-job-stale"
        } else {
            "progress-job-background"
        })
        .into()
    };
    display.evidence_id = job.status_evidence_id.clone().or(display.evidence_id);
    let count = |key| {
        job.status
            .result
            .as_ref()
            .and_then(|result| result.extra.get(key))
            .and_then(Value::as_u64)
    };
    let observation = job.status.observation();
    let result = job.status.result.as_ref();
    let execution = JobExecutionDisplay {
        finished: observation.terminal,
        outcome: match observation.outcome {
            OutcomeCertainty::Known => JobOutcome::Known,
            OutcomeCertainty::Unknown => JobOutcome::Unknown,
        },
        exit_code: result.and_then(|result| result.exit_code),
        passed: result.and_then(|result| result.passed),
        evidence_id: job
            .execution_status_evidence_id
            .clone()
            .or_else(|| job.status_evidence_id.clone()),
    };
    let cleanup = JobCleanupDisplay {
        status: match observation.cleanup {
            CleanupState::Pending => JobCleanupStatus::Pending,
            CleanupState::Unconfirmed => JobCleanupStatus::Unconfirmed,
            CleanupState::Confirmed => JobCleanupStatus::Confirmed,
        },
        // Only the settled final status is cleanup evidence; a finished
        // execution receipt must not be presented as cleanup confirmation.
        evidence_id: job.status_evidence_id.clone(),
    };
    display.job = Some(JobDisplay {
        id: job.id.clone(),
        state: job.status.state,
        stale,
        started_at,
        finished_at,
        observed_at: job.updated_at,
        attempts: count("attempts"),
        successes: count("successes"),
        execution,
        cleanup,
        blocking: blocking(job, stale),
        evidence: evidence(job),
    });
    display
}
