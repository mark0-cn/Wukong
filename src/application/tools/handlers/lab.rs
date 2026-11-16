//! Windows lab tool handlers.
use super::*;
use crate::application::tools::context::ToolContext;
use crate::application::windows_lab::{ExperimentSpec, WindowsLab};

fn open<'a>(tools: &'a ToolContext, vm: &'a str) -> Result<WindowsLab> {
    tools.lab.open(vm)
}

pub(in crate::application::tools) fn handle_lab_status<'a>(
    tools: &'a ToolContext,
    agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::LabStatusArgs = serde_json::from_value(args)?;
        let Some(vm) = input.vm else {
            return Ok(json!({"configured_vms": tools.lab.list()}));
        };
        if agent.role != Role::Root {
            let config = tools.lab.config.vms.get(&vm).context("VM not configured")?;
            ensure!(
                agent.environment_id.is_some() && agent.environment_id == config.environment,
                "VM is outside this task environment"
            );
        }
        let lab = open(tools, &vm)?;
        let result = lab.status().await;
        lab.finish().await;
        result
    })
}

pub(in crate::application::tools) fn handle_lab_prepare<'a>(
    tools: &'a ToolContext,
    agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::LabPrepareArgs = serde_json::from_value(args)?;
        let spec: ExperimentSpec = serde_json::from_value(input.spec)
            .context("lab spec must be an ExperimentSpec object")?;
        let lab = open(tools, &input.vm)?;
        tools.lab.authorize_vm(agent, &input.vm)?;
        let result = lab.prepare(&agent.id, &input.purpose, spec).await;
        lab.finish().await;
        let lease = result?;
        let value = serde_json::to_value(&lease)?;
        let reference = tools.lab.bind_reference(
            agent,
            &input.vm,
            text(&value, "lab_id")?,
            number(&value, "generation", 0),
        )?;
        Ok(json!({"lab_ref":reference,"lease":value}))
    })
}

pub(in crate::application::tools) fn handle_lab_run<'a>(
    tools: &'a ToolContext,
    agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::LabRunArgs = serde_json::from_value(args)?;
        let reference = tools.lab.resolve_reference(agent, &input.lab_ref)?;
        let stdin = match input.stdin_evidence_id.as_deref() {
            Some(id) => {
                let bytes = tools.analysis.store.read_evidence(&agent.run_id, id)?;
                ensure!(bytes.len() <= 1024 * 1024, "lab stdin exceeds 1 MiB");
                Some(bytes)
            }
            None => None,
        };
        let lab = open(tools, &reference.vm)?;
        let result = lab
            .run(
                &reference.lab_id,
                reference.generation,
                &input.artifact,
                stdin,
                input.timeout_secs.unwrap_or(120),
            )
            .await;
        lab.finish().await;
        result
    })
}

pub(in crate::application::tools) fn handle_lab_install<'a>(
    tools: &'a ToolContext,
    agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::LabInstallArgs = serde_json::from_value(args)?;
        let reference = tools.lab.resolve_reference(agent, &input.lab_ref)?;
        let lab = open(tools, &reference.vm)?;
        let result = lab
            .install(
                &reference.lab_id,
                reference.generation,
                input.timeout_secs.unwrap_or(300),
            )
            .await;
        lab.finish().await;
        result
    })
}

pub(in crate::application::tools) fn handle_lab_restore<'a>(
    tools: &'a ToolContext,
    agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::LabRestoreArgs = serde_json::from_value(args)?;
        let reference = tools.lab.resolve_reference(agent, &input.lab_ref)?;
        let lab = open(tools, &reference.vm)?;
        let result = lab.restore(&reference.lab_id, reference.generation).await;
        lab.finish().await;
        result
    })
}
