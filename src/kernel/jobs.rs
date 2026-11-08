//! Jobs systems for the Rig adapter.
use super::*;

/// Poll each in-run job once per drive tick and record terminal results in
/// the store, so the next model request injects them as a section instead of
/// the agent spending a full turn on job_status.
pub(super) async fn refresh_job_statuses(runner: &AssessmentRunner, run_id: &str) -> Result<()> {
    let Ok(runtime) = runner.tools.runtime() else {
        return Ok(());
    };
    let jobs = runner.store.list::<Value>("job", run_id)?;
    for job in jobs {
        if job["delivered_to_model"].as_bool() == Some(true) {
            continue;
        }
        let state = job["status"]["state"]
            .as_str()
            .unwrap_or("queued")
            .to_owned();
        if matches!(
            state.as_str(),
            "completed" | "failed" | "cancelled" | "timed_out"
        ) {
            // Already terminal from a manual poll; just mark it deliverable.
            let mut record = job;
            record["delivered_to_model"] = json!(true);
            if let Some(job_id) = record["id"].as_str().map(str::to_owned) {
                runner.store.put("job", &job_id, run_id, &record)?;
            }
            continue;
        }
        let Some(job_id) = job["id"].as_str().map(str::to_owned) else {
            continue;
        };
        let Ok(status) = runtime.call("job.status", json!({"job_id": job_id})).await else {
            continue;
        };
        let state = status["state"].as_str().unwrap_or("queued").to_owned();
        if !matches!(
            state.as_str(),
            "completed" | "failed" | "cancelled" | "timed_out"
        ) {
            continue;
        }
        let mut record = job;
        record["status"] = status;
        record["delivered_to_model"] = json!(true);
        runner.store.put("job", &job_id, run_id, &record)?;
    }
    Ok(())
}

/// Terminal, delivered-but-not-yet-injected job results for one agent, each
/// marked injected once its brief enters the prompt.
pub(super) fn collect_job_briefs(store: &Store, agent: &AgentRecord) -> Result<Vec<Value>> {
    let mut briefs = Vec::new();
    for job in store.list::<Value>("job", &agent.run_id)? {
        if job["agent_id"].as_str() != Some(agent.id.as_str())
            || job["delivered_to_model"].as_bool() != Some(true)
            || job["injected"].as_bool() == Some(true)
        {
            continue;
        }
        let Some(job_id) = job["id"].as_str().map(str::to_owned) else {
            continue;
        };
        let status = &job["status"];
        briefs.push(json!({
            "job_id": job_id,
            "tool": job["tool_name"],
            "state": status["state"],
            "exit_code": status["exit_code"],
            "passed": status["passed"],
            "stdout": status["stdout"].as_str().unwrap_or("").chars().take(400).collect::<String>(),
            "stderr": status["stderr"].as_str().unwrap_or("").chars().take(200).collect::<String>(),
            "note": "This job result arrived automatically; do not call job_status for it.",
        }));
        let mut record = job;
        record["injected"] = json!(true);
        store.put("job", &job_id, &agent.run_id, &record)?;
    }
    Ok(briefs)
}
