//! Jobs systems for the Rig adapter.
use super::*;

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
