//! Windows lab tool handlers.
use super::*;
use crate::application::tools::context::ToolContext;
use crate::application::windows_lab::{ExperimentSpec, WindowsLab};

fn open<'a>(tools: &'a ToolContext, vm: &'a str) -> Result<WindowsLab> {
    tools.lab.open(vm)
}

pub(in crate::application::tools) fn handle_lab_status<'a>(
    tools: &'a ToolContext,
    _agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::LabStatusArgs = serde_json::from_value(args)?;
        let Some(vm) = input.vm else {
            return Ok(json!({"configured_vms": tools.lab.list()}));
        };
        let lab = open(tools, &vm)?;
        let result = lab.status().await;
        lab.finish().await;
        result
    })
}

pub(in crate::application::tools) fn handle_lab_prepare<'a>(
    tools: &'a ToolContext,
    _agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::LabPrepareArgs = serde_json::from_value(args)?;
        let spec: ExperimentSpec = serde_json::from_value(input.spec)
            .context("lab spec must be an ExperimentSpec object")?;
        let lab = open(tools, &input.vm)?;
        let result = lab.prepare(&input.owner, &input.purpose, spec).await;
        lab.finish().await;
        result.map(|lease| serde_json::to_value(lease).unwrap_or(Value::Null))
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
        let stdin = match input.stdin_evidence_id.as_deref() {
            Some(id) => {
                let bytes = tools.analysis.store.read_evidence(&agent.run_id, id)?;
                ensure!(bytes.len() <= 1024 * 1024, "lab stdin exceeds 1 MiB");
                Some(bytes)
            }
            None => None,
        };
        let lab = open(tools, &input.vm)?;
        let result = lab
            .run(
                &input.lab_id,
                input.generation,
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
    _agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::LabInstallArgs = serde_json::from_value(args)?;
        let lab = open(tools, &input.vm)?;
        let result = lab
            .install(
                &input.lab_id,
                input.generation,
                input.timeout_secs.unwrap_or(300),
            )
            .await;
        lab.finish().await;
        result
    })
}

pub(in crate::application::tools) fn handle_lab_restore<'a>(
    tools: &'a ToolContext,
    _agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::LabRestoreArgs = serde_json::from_value(args)?;
        let lab = open(tools, &input.vm)?;
        let result = lab.restore(&input.lab_id, input.generation).await;
        lab.finish().await;
        result
    })
}

pub(in crate::application::tools) fn handle_lab_prune<'a>(
    tools: &'a ToolContext,
    _agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::LabPruneArgs = serde_json::from_value(args)?;
        let lab = open(tools, &input.vm)?;
        let result = lab.prune().await;
        lab.finish().await;
        result
    })
}
