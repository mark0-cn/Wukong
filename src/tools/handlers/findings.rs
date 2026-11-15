//! Findings tool handlers.
use super::*;

pub(in crate::tools) fn handle_finding_file<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.finding(agent, call_id, name, &args) })
}

pub(in crate::tools) fn handle_findings_list<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { Ok(json!({"findings":tools.store.records::<Finding>(&agent.run_id)?})) })
}

pub(in crate::tools) fn handle_finding_get<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let finding: Finding = require_in_run(&tools.store, text(&args, "id")?, &agent.run_id)?;
        let verification = finding
            .verification_id
            .as_ref()
            .map(|id| require_in_run::<Verification>(&tools.store, id, &agent.run_id))
            .transpose()?;
        Ok(json!({"finding":finding,"verification":verification}))
    })
}

pub(in crate::tools) fn handle_verification_record<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        tools
            .execution
            .verification
            .record(agent, call_id, &args)
            .await
    })
}

pub(in crate::tools) fn handle_evidence_read<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let evidence_id = text(&args, "evidence_id")?.to_owned();
        let record: Evidence = require_in_run(&tools.store, &evidence_id, &agent.run_id)?;
        let total = usize::try_from(record.size)?;
        let offset = usize::try_from(number(&args, "offset", 0))?;
        ensure!(offset <= total, "evidence offset is past end of file");
        let length = usize::try_from(number(&args, "length", 8192))?
            .min(tools.limits.max_evidence_read_bytes);
        let part = if offset == total || length == 0 {
            Vec::new()
        } else {
            tools
                .store
                .read_evidence_range(&agent.run_id, &evidence_id, offset as u64, length)?
        };
        let end = offset + part.len();
        let content = match std::str::from_utf8(&part) {
            Ok(text) => json!({"encoding":"utf8","data":text}),
            Err(_) => json!({"encoding":"base64","data":BASE64.encode(&part)}),
        };
        Ok(
            json!({"evidence_id":evidence_id,"offset":offset,"length":part.len(),"total_bytes":total,"next_offset":if end<total{Some(end)}else{None},"content":content}),
        )
    })
}
