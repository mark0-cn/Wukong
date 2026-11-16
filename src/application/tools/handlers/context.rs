//! Context tool handlers.
use super::*;

pub(in crate::application::tools) fn handle_threat_model_save<'a>(
    tools: &'a documents::DocumentService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.save_document(agent, call_id, name, &args) })
}

pub(in crate::application::tools) fn handle_threat_model_amend<'a>(
    tools: &'a documents::DocumentService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.amend_document(agent, call_id, name, &args) })
}

pub(in crate::application::tools) fn handle_context_search<'a>(
    tools: &'a documents::DocumentService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        crate::application::context::search(&tools.store, agent, &args, tools.output_limit)
    })
}

pub(in crate::application::tools) fn handle_context_read<'a>(
    tools: &'a documents::DocumentService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        crate::application::context::read(&tools.store, agent, &args, tools.output_limit)
    })
}

pub(in crate::application::tools) fn handle_coverage_record<'a>(
    tools: &'a evidence::EvidenceService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.coverage(agent, call_id, name, &args) })
}

pub(in crate::application::tools) fn handle_coverage_list<'a>(
    tools: &'a evidence::EvidenceService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { super::super::inventory::coverage(&tools.store, agent, call_id, &args) })
}
