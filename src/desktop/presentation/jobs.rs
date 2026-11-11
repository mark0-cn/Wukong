//! Project the existing worker job lifecycle onto its originating tool row.
use super::*;

fn timestamp(value: Option<&Value>) -> Option<chrono::DateTime<chrono::Utc>> {
    let seconds = value?.as_f64()?;
    if !seconds.is_finite() {
        return None;
    }
    chrono::DateTime::from_timestamp_millis((seconds * 1000.0) as i64)
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
    display.job = Some(JobDisplay {
        id: job.id.clone(),
        state: job.status.state,
        stale,
        started_at,
        finished_at,
        observed_at: job.updated_at,
        attempts: count("attempts"),
        successes: count("successes"),
    });
    display
}
