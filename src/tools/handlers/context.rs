//! Context tool handlers.
use super::*;

pub(in crate::tools) fn handle_threat_model_get<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        if let Some(id) = args["id"].as_str() {
            Ok(serde_json::to_value(require_document(
                &tools.store,
                DocumentKind::ThreatModel,
                id,
                &agent.run_id,
            )?)?)
        } else {
            Ok(json!({"documents":tools.store.documents(DocumentKind::ThreatModel,&agent.run_id)?}))
        }
    })
}

pub(in crate::tools) fn handle_threat_model_save<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.save_document(agent, call_id, name, &args) })
}

pub(in crate::tools) fn handle_threat_model_amend<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.amend_document(agent, call_id, name, &args) })
}

pub(in crate::tools) fn handle_notes_list<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        Ok(json!({"notes":tools.store.documents(DocumentKind::Note,&agent.run_id)?}))
    })
}

pub(in crate::tools) fn handle_context_search<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        crate::context::search(&tools.store, agent, &args, tools.limits.max_output_bytes)
    })
}

pub(in crate::tools) fn handle_context_read<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        crate::context::read(&tools.store, agent, &args, tools.limits.max_output_bytes)
    })
}

pub(in crate::tools) fn handle_todos_list<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        Ok(json!({"todos":tools.store.documents(DocumentKind::Todo,&agent.run_id)?}))
    })
}

pub(in crate::tools) fn handle_coverage_record<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.coverage(agent, call_id, name, &args) })
}

pub(in crate::tools) fn handle_coverage_list<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { Ok(json!({"coverage":tools.store.records::<Coverage>(&agent.run_id)?})) })
}
