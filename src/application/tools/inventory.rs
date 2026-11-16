//! Bounded controller inventories. Listing never grants ownership or performs cleanup.
use super::*;

fn short(value: &str) -> String {
    value.chars().take(240).collect()
}

pub(super) fn page(mut rows: Vec<Value>, args: &Value, key: &str) -> Result<Value> {
    rows.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let after = args["after"].as_str();
    if let Some(after) = after {
        rows.retain(|row| row["id"].as_str().is_some_and(|id| id > after));
    }
    let limit = number(args, "limit", 10) as usize;
    ensure!((1..=20).contains(&limit), "limit must be between 1 and 20");
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next = more.then(|| rows.last().unwrap()["id"].clone());
    Ok(json!({key:rows,"next_cursor":next}))
}

fn matches(args: &Value, field: &str, value: &str) -> bool {
    args[field].as_str().is_none_or(|wanted| value == wanted)
}

pub(super) fn findings(store: &Store, agent: &AgentMetadata, args: &Value) -> Result<Value> {
    let rows = store.records::<Finding>(&agent.run_id)?.into_iter().filter_map(|row| {
        let value = serde_json::to_value(&row).ok()?;
        (matches(args,"agent_id",&row.agent_id)
            && matches(args,"stage",value["stage"].as_str().unwrap_or_default())
            && matches(args,"severity",value["severity"].as_str().unwrap_or_default()))
            .then(|| json!({"id":row.id,"agent_id":row.agent_id,"title":short(&row.title),"stage":row.stage,"severity":row.severity,"verification_id":row.verification_id}))
    }).collect();
    page(rows, args, "findings")
}

pub(super) fn coverage(
    store: &Store,
    agent: &AgentMetadata,
    call: &str,
    args: &Value,
) -> Result<Value> {
    let full = store
        .records::<Coverage>(&agent.run_id)?
        .into_iter()
        .filter(|row| {
            matches(args, "agent_id", &row.agent_id)
                && matches(args, "scope", &row.scope)
                && matches(args, "outcome", &row.outcome)
        })
        .map(serde_json::to_value)
        .collect::<serde_json::Result<Vec<_>>>()?;
    let mut result = page(full, args, "coverage")?;
    let evidence = store.artifact(
        &agent.run_id,
        &agent.id,
        call,
        "coverage.page",
        &serde_json::to_vec(&result)?,
        "application/json",
        json!({"raw_tool_output":true}),
    )?;
    if let Some(rows) = result["coverage"].as_array_mut() {
        for row in rows {
            for key in ["scope", "kind", "notes"] {
                row[key] = json!(short(row[key].as_str().unwrap_or_default()));
            }
            row["evidence_count"] = json!(row["evidence_ids"].as_array().map_or(0, Vec::len));
            row.as_object_mut().unwrap().remove("evidence_ids");
        }
    }
    result["full_evidence_id"] = json!(evidence.id);
    Ok(result)
}

pub(super) fn evidence(store: &Store, agent: &AgentMetadata, args: &Value) -> Result<Value> {
    let rows = store.records::<Evidence>(&agent.run_id)?.into_iter()
        .filter(|row| matches(args,"agent_id",&row.agent_id) && matches(args,"sha256",&row.sha256)
            && matches(args,"media_type",&row.media_type) && matches(args,"source",&row.source)
            && matches(args,"target_sha256",row.metadata["target_sha256"].as_str().unwrap_or_default())
            && matches(args,"job_id",row.metadata["job_id"].as_str().unwrap_or_default()))
        .map(|row|json!({"id":row.id,"agent_id":row.agent_id,"sha256":row.sha256,"size":row.size,"source":short(&row.source),"media_type":row.media_type,"created_at":row.created_at})).collect();
    page(rows, args, "evidence")
}

pub(super) fn resources(store: &Store, agent: &AgentMetadata, args: &Value) -> Result<Value> {
    let mut rows = Vec::new();
    if matches(args, "kind", "job") {
        rows.extend(store.records::<JobRecord>(&agent.run_id)?.into_iter()
            .filter(|row| row.agent_id == agent.id)
            .filter(|row| args["active_only"] != true || !row.status.settled())
            .map(|row| json!({"id":row.id,"kind":"job","state":row.status.state,"tool":row.tool_name,"worker_id":row.worker_id,"updated_at":row.updated_at,"status_evidence_id":row.status_evidence_id})));
    }
    for (kind, label) in [
        (SessionKind::Debugger, "debugger"),
        (SessionKind::Interactive, "interactive"),
    ] {
        if matches(args, "kind", label) {
            rows.extend(store.sessions(kind,&agent.run_id)?.into_iter()
                .filter(|row| row.agent_id == agent.id)
                .filter(|row| args["active_only"] != true || !matches!(row.state,SessionState::Closed))
                .map(|row| json!({"id":row.id,"kind":label,"state":row.state,"worker_id":row.worker_id,"updated_at":row.updated_at,"transcript_evidence_id":row.transcript_evidence_id})));
        }
    }
    let mut result = page(rows, args, "resources")?;
    result["source"] =
        json!("controller-owned records; state is last observed, use status/inspect to refresh");
    Ok(result)
}
