//! Bounded working state rebuilt from durable observations, not prior summaries.
use super::*;

fn ends(text: &str, limit: usize) -> Value {
    let count = text.chars().count();
    if count <= limit {
        return range(text, 0, limit);
    }
    json!({"head":range(text,0,limit/2),"tail":range(text,count-limit/2,limit/2)})
}

fn bookends<T>(items: &[T], first: usize, last: usize) -> impl Iterator<Item = &T> {
    items.iter().take(first).chain(
        items
            .iter()
            .skip(first.max(items.len().saturating_sub(last))),
    )
}

pub(crate) fn snapshot(tx: &StateSession<'_>, agent: &AgentMetadata) -> Result<Value> {
    let mut jobs = tx.records::<JobRecord>(&agent.run_id)?;
    jobs.retain(|job| job.agent_id == agent.id);
    jobs.sort_by_key(|job| (job.created_at, job.id.clone()));
    let completed = jobs
        .iter()
        .filter(|job| job.status.state == JobState::Completed)
        .collect::<Vec<_>>();
    let other = jobs
        .iter()
        .filter(|job| job.status.state != JobState::Completed)
        .collect::<Vec<_>>();
    let selected_jobs = bookends(&completed, 3, 5).chain(bookends(&other, 0, 4)).map(|job| {
        json!({"id":job.id,"tool":job.tool_name,"state":job.status.state,
            "target_sha256":job.target_sha256,"generation":job.owner_generation,
            "status_evidence_id":job.status_evidence_id,
            "evidence_sync":job.evidence_sync.progress(job.evidence_ready()),
            "stdout_observation":ends(&job.status.stdout,1600),
            "stderr_observation":ends(&job.status.stderr,400),
            "result_preview":ends(&json!(job.status.result).to_string(),800),
            "artifacts_preview":ends(&json!(job.imported_artifacts).to_string(),800),
            "read_with":"job_status/job_artifacts or evidence_read; reuse artifacts before rerunning"})
    }).collect::<Vec<_>>();
    let mut files = tx.records::<WorkerFileRecord>(&agent.run_id)?;
    files.retain(|file| file.agent_id == agent.id);
    files.sort_by_key(|file| (file.created_at, file.id.clone()));
    let selected_files = bookends(&files, 2, 10).map(|file| json!({
        "path":range(&file.path,0,512),"sha256":file.sha256,"size":file.size,"evidence_id":file.evidence_id,
        "worker_id":file.worker_id
    })).collect::<Vec<_>>();
    let mut calls = tx.records::<ToolCallRecord>(&agent.run_id)?;
    calls.retain(|call| call.agent_id == agent.id);
    calls.sort_by_key(|call| (call.created_at, call.id.clone()));
    let mut errors = BTreeMap::new();
    for call in &calls {
        if call.state == CallState::Failed {
            errors.insert(&call.name, call);
        }
    }
    let errors = errors
        .values()
        .rev()
        .take(8)
        .map(|call| {
            json!({
                "call_id":call.id,"tool":call.name,"evidence_id":call.evidence_id,
                "arguments_preview":range(&call.arguments.to_string(),0,500),
                "error_preview":range(&json!(call.result).to_string(),0,900)
            })
        })
        .collect::<Vec<_>>();
    let observations = calls
        .iter()
        .rev()
        .filter(|call| {
            call.state == CallState::Completed
                && matches!(
                    call.name.as_str(),
                    "binary_inspect"
                        | "function_decompile"
                        | "analysis_verify"
                        | "verification_record"
                )
        })
        .take(6)
        .map(|call| {
            json!({"call_id":call.id,"tool":call.name,"evidence_id":call.evidence_id,
            "result_preview":range(&json!(call.result).to_string(),0,1200)})
        })
        .collect::<Vec<_>>();
    let mut notes = documents(tx, &agent.run_id)?;
    notes.retain(|note| note.agent_id == agent.id);
    notes.sort_by_key(|note| (note.updated_at, note.id.clone()));
    let notes = notes.iter().rev().take(4).map(|note| json!({
        "id":note.id,"kind":note.kind,"title":range(&note.title,0,160),
        "body_preview":range(&note.body,0,2000),"amendments_preview":range(&json!(note.amendments).to_string(),0,800),
        "read_with":"context_read"
    })).collect::<Vec<_>>();
    Ok(json!({
        "notice":"Durable working state: observations and author claims, NOT independently verified facts. Completed jobs can contain failed checks. Earliest and latest completed jobs preserve initial discoveries across repeated compactions; previews omit material. Retrieve original evidence before relying on a claim. Continue the remaining gap rather than rediscovering results. Save concise notes incrementally with facts, evidence, rejected hypotheses and next actions.",
        "jobs":selected_jobs,"jobs_omitted":jobs.len().saturating_sub(selected_jobs.len()),
        "files":selected_files,"files_omitted":files.len().saturating_sub(selected_files.len()),
        "tool_errors":errors,"static_and_verification_observations":observations,"notes":notes
    }))
}
