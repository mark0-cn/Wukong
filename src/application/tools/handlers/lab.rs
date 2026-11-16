//! Windows lab tool handlers.
use super::*;
use crate::application::tools::lab::LabService;
use crate::application::windows_lab::WindowsLab;

fn open(tools: &LabService, vm: &str, agent: &AgentMetadata) -> Result<WindowsLab> {
    tools.open(vm, agent)
}

pub(in crate::application::tools) fn handle_lab_status<'a>(
    tools: &'a LabService,
    agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::LabStatusArgs = serde_json::from_value(args)?;
        let lab = open(tools, &input.vm, agent)?;
        lab.status().await
    })
}

pub(in crate::application::tools) fn handle_lab_prepare<'a>(
    service: &'a LabService,
    agent: &'a AgentMetadata,
    call: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { service.start_operation(agent, call, name, args).await })
}

pub(in crate::application::tools) fn handle_lab_run<'a>(
    service: &'a LabService,
    agent: &'a AgentMetadata,
    call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        service
            .run(agent, call, serde_json::from_value(args)?)
            .await
    })
}

pub(in crate::application::tools) fn handle_lab_install<'a>(
    service: &'a LabService,
    agent: &'a AgentMetadata,
    call: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { service.start_operation(agent, call, name, args).await })
}

pub(in crate::application::tools) fn handle_lab_restore<'a>(
    service: &'a LabService,
    agent: &'a AgentMetadata,
    call: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { service.start_operation(agent, call, name, args).await })
}

pub(in crate::application::tools) fn handle_lab_operation<'a>(
    service: &'a LabService,
    agent: &'a AgentMetadata,
    _call: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::LabOperationArgs = serde_json::from_value(args)?;
        service
            .operation(agent, &input.operation_id, name == "lab_operation_cancel")
            .await
    })
}

pub(in crate::application::tools) fn handle_lab_list<'a>(
    service: &'a LabService,
    _agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { Ok(json!({"configured_vms":service.list()})) })
}
