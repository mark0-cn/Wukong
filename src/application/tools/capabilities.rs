//! Task grants authorize advanced tools; selection only controls model context size.
use super::*;

pub(super) fn group(name: &str) -> &'static str {
    match name {
        "debug_command" => "raw_debugger",
        name if name.starts_with("lab_") => "windows_lab",
        name if name.starts_with("debug_") || name == "crash_snapshot" => "debugger",
        "fuzz_start" | "input_minimize" | "cyclic_offset" => "fuzzing",
        "angr_solve" | "emulate_run" => "symbolic",
        "function_decompile" | "functions_search" | "function_instructions" | "xrefs_query" => {
            "static_analysis"
        }
        _ => "core",
    }
}

pub(super) fn authorized(agent: &AgentMetadata, name: &str) -> bool {
    let grant = match name {
        "debug_command" => ToolGrant::RawDebugger,
        "lab_prepare" | "lab_run" | "lab_install" | "lab_restore" => ToolGrant::WindowsLab,
        _ => return true,
    };
    agent.contract.tool_grants.contains(&grant)
}

pub(super) fn selected(store: &Store, agent: &AgentMetadata, name: &str) -> bool {
    // Lifecycle observation/cleanup must remain available after switching groups.
    if matches!(
        name,
        "debug_close"
            | "debug_inspect"
            | "debug_control"
            | "lab_status"
            | "lab_operation_status"
            | "lab_operation_cancel"
            | "lab_restore"
    ) {
        return true;
    }
    selected_group(store, agent, group(name))
}

pub(super) fn selected_group(store: &Store, agent: &AgentMetadata, group: &str) -> bool {
    if group == "core" {
        return true;
    }
    store
        .get::<Value>("tool_selection", &agent.id)
        .ok()
        .flatten()
        .map(|selection| {
            selection["groups"]
                .as_array()
                .is_some_and(|groups| groups.iter().any(|g| g == group))
        })
        .unwrap_or(matches!(group, "static_analysis" | "debugger"))
}

impl coordination::CoordinationService {
    pub(super) fn select_tools(
        &self,
        agent: &AgentMetadata,
        call: &str,
        args: Value,
    ) -> Result<Value> {
        let groups = args["groups"].as_array().context("groups required")?;
        for group in groups {
            let required = match group.as_str() {
                Some("raw_debugger") => Some(ToolGrant::RawDebugger),
                Some("windows_lab") => Some(ToolGrant::WindowsLab),
                _ => None,
            };
            ensure!(
                required.is_none_or(|grant| agent.contract.tool_grants.contains(&grant)),
                "group requires an explicit task grant from an authorized ancestor; selection cannot grant permissions"
            );
        }
        self.effect(agent,call,|tx| {
            tx.put("tool_selection", &agent.id, &agent.run_id, &args)?;
            Ok(json!({"selected_groups":groups,"always_available":"core and owned-resource lifecycle tools","effective":"next model request; backend readiness and task grants still apply"}))
        })
    }
    pub(super) fn tool_catalog(&self, agent: &AgentMetadata, args: &Value) -> Result<Value> {
        let snapshot = self
            .store
            .get::<Value>("tool_availability", &agent.id)?
            .unwrap_or(json!({}));
        let mut rows: Vec<Value> = builtins().iter().filter(|spec|spec.model_visible)
            .filter(|spec|args["group"].as_str().is_none_or(|g|g==group(spec.definition.name.as_str())))
            .map(|spec| {
                let name = spec.definition.name.as_str();
                let reason = if !spec.allows(agent.role) { "role denied" }
                    else if !authorized(agent,name) { "task grant required" }
                    else if !selected(&self.store,agent,name) { "group not selected" }
                    else if snapshot["available"].as_array().is_some_and(|names|names.iter().any(|n|n==name)) { "available at last preflight; rechecked on invocation" }
                    else { "backend unavailable or not yet observed" };
                json!({"id":name,"group":group(name),"status":reason,"requires":spec.requirement.label()})
            }).collect();
        if args["group"].as_str().is_none_or(|g| g == "external") {
            rows.extend(self.external_tools.iter().map(|name| {
                let status = if agent.role == Role::Root { "role denied" }
                    else if !selected_group(&self.store,agent,"external") { "group not selected" }
                    else { "configured; rechecked on invocation" };
                json!({"id":name,"group":"external","status":status,"requires":"configured MCP server"})
            }));
        }
        let mut result = inventory::page(rows, args, "tools")?;
        result["observed_at"] = snapshot["observed_at"].clone();
        Ok(result)
    }
}
