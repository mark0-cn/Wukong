//! Load only visible calls and unfinished jobs; never query workers from a UI read.
use super::*;

pub(super) struct Tools {
    pub calls: Vec<ToolCallRecord>,
    pub jobs: Vec<JobRecord>,
    pub active: Vec<presentation::ToolDisplay>,
}

pub(super) fn read(
    tx: &Connection,
    run: &RunSummary,
    selected_agent: Option<&str>,
    events: &[Event],
    agents: &[AgentSummary],
) -> Result<Tools> {
    let ids: BTreeSet<_> = events
        .iter()
        .filter_map(|event| event.data["call_id"].as_str())
        .collect();
    let ids = serde_json::to_string(&ids)?;
    let mut query = tx.prepare("SELECT data FROM entities WHERE kind='job' AND run_id=?1 AND (?2 IS NULL OR json_extract(data,'$.agent_id')=?2) AND (json_extract(data,'$.status.state') IN ('queued','running','lost') OR json_extract(data,'$.call_id') IN (SELECT value FROM json_each(?3))) ORDER BY rowid")?;
    let jobs = query
        .query_map(params![run.id, selected_agent, ids], |row| {
            row.get::<_, String>(0)
        })?
        .map(|raw| Ok(serde_json::from_str::<JobRecord>(&raw?)?))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|job| job.target_sha256 == run.target_sha256)
        .collect::<Vec<_>>();
    let mut query = tx.prepare("SELECT data FROM entities WHERE kind='call' AND run_id=?1 AND (?2 IS NULL OR json_extract(data,'$.agent_id')=?2) AND (json_extract(data,'$.state') IN ('prepared','running','uncertain') OR id IN (SELECT value FROM json_each(?3)) OR id IN (SELECT value FROM json_each(?4))) ORDER BY rowid")?;
    let calls = query
        .query_map(
            params![
                run.id,
                selected_agent,
                ids,
                serde_json::to_string(&jobs.iter().map(|job| &job.call_id).collect::<Vec<_>>())?
            ],
            |row| row.get::<_, String>(0),
        )?
        .map(|raw| Ok(serde_json::from_str::<ToolCallRecord>(&raw?)?))
        .collect::<Result<Vec<_>>>()?;
    let target_name = run
        .target_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let active = calls
        .iter()
        .filter_map(|call| {
            let agent = agents.iter().find(|agent| agent.id == call.agent_id);
            let display = presentation::with_job(
                presentation::tool_display(call, agent, &target_name),
                call,
                agent,
                &jobs,
            );
            (matches!(display.state.as_str(), "queued" | "running" | "uncertain")
                && display.job.as_ref().is_none_or(|job| !job.stale))
            .then_some(display)
        })
        .collect();
    Ok(Tools {
        calls,
        jobs,
        active,
    })
}
