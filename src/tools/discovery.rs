//! Progressive tool disclosure. Rig remains responsible for schemas and calls;
//! selection only narrows the existing role permissions.
use super::*;
use std::collections::BTreeSet;

const MAX_SELECTED: usize = 12;
const CORE: &[&str] = &[
    "view_agent_graph",
    "send_message_to_agent",
    "follow_up_agent",
    "wait_for_agents",
    "agent_finish",
    "finish_scan",
    "note_save",
    "context_search",
    "context_read",
    "evidence_read",
    "load_skill",
    "think",
    "tool_search",
    "tools_load",
];

#[derive(Default, Serialize, Deserialize)]
struct Selection {
    generation: u64,
    names: Vec<String>,
}

fn group(name: &str) -> &'static str {
    match name {
        name if CORE.contains(&name) => "core",
        "message_status" | "stop_agent" => "coordination",
        "threat_model_get" | "threat_model_save" | "threat_model_amend" => "security_context",
        "notes_list" | "todo_save" | "todo_update" | "todos_list" => "shared",
        "coverage_record" | "coverage_update" | "coverage_list" => "coverage",
        "finding_file"
        | "finding_update"
        | "findings_list"
        | "finding_get"
        | "verification_record" => "findings",
        "binary_inspect" | "binary_strings" => "binary",
        "functions_search" | "function_decompile" | "xrefs_query" => "ghidra",
        "runtime_doctor" | "stage_target" | "file_put" | "file_get" => "runtime",
        "job_start" | "job_status" | "job_cancel" | "job_artifacts" => "jobs",
        "debug_start" | "debug_command" | "debug_close" => "debugger",
        "interactive_start" | "interactive_send" | "interactive_close" => "interactive",
        "fuzz_start" | "fuzz_status" => "fuzz",
        "crash_replay" | "input_minimize" | "poc_verify" => "reproduction",
        "angr_solve" => "symbolic",
        name if name.starts_with("mcp_") => "mcp",
        _ => "other",
    }
}

fn baseline(role: Role, name: &str) -> bool {
    CORE.contains(&name) || (role == Role::Analyst && name == "binary_inspect")
}

impl ToolService {
    fn discoverable(&self, role: Role) -> Vec<ToolDefinition> {
        self.definitions(role)
            .into_iter()
            .filter(|definition| definition.name.as_str() != "create_agent")
            .collect()
    }

    /// Native task/message/wait are supplied separately by SubagentsPlugin.
    /// This is also used to rebuild request definitions after a selection has
    /// changed, since the previous ToolAccess may have hidden the new tools.
    pub(crate) fn model_tool_names(&self, agent: &AgentRecord) -> Result<BTreeSet<String>> {
        let selection = self
            .store
            .get::<Selection>("tool_selection", &agent.id)?
            .unwrap_or_default();
        Ok(self
            .discoverable(agent.role)
            .into_iter()
            .filter(|definition| {
                let name = definition.name.as_str();
                baseline(agent.role, name)
                    || (selection.generation == agent.generation
                        && selection.names.iter().any(|selected| selected == name))
            })
            .map(|definition| definition.name.to_string())
            .collect())
    }

    pub(crate) fn discovery_context(&self, agent: &AgentRecord) -> Result<Value> {
        let mut groups = BTreeMap::<&str, usize>::new();
        for definition in self.discoverable(agent.role) {
            if !baseline(agent.role, definition.name.as_str()) {
                *groups.entry(group(definition.name.as_str())).or_default() += 1;
            }
        }
        let active = self
            .model_tool_names(agent)?
            .into_iter()
            .filter(|name| !baseline(agent.role, name))
            .collect::<Vec<_>>();
        Ok(
            json!({"deferred_groups":groups,"selected":active,"max_selected":MAX_SELECTED,
            "notice":"Use tool_search by group or keywords to discover names. tools_load replaces the selected optional tools; their full schemas appear on the next fresh model request. Core tools remain available. Discovery and selection never grant permissions or prove a worker capability is installed."}),
        )
    }

    pub(super) fn search_tools(&self, agent: &AgentRecord, args: &Value) -> Result<Value> {
        let query = args["query"].as_str().unwrap_or_default().to_lowercase();
        let words = query.split_whitespace().collect::<Vec<_>>();
        let selected = self.model_tool_names(agent)?;
        let mut definitions = self
            .discoverable(agent.role)
            .into_iter()
            .filter(|definition| {
                if args["group"]
                    .as_str()
                    .is_some_and(|name| group(definition.name.as_str()) != name)
                {
                    return false;
                }
                let text = format!(
                    "{} {} {}",
                    definition.name,
                    group(definition.name.as_str()),
                    definition.description
                )
                .to_lowercase();
                words.iter().all(|word| text.contains(word))
            })
            .collect::<Vec<_>>();
        definitions.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        let offset = args["offset"].as_u64().unwrap_or(0) as usize;
        let limit = args["limit"].as_u64().unwrap_or(6).clamp(1, 10) as usize;
        let mut page = definitions
            .iter()
            .skip(offset)
            .take(limit)
            .map(|definition| {
                json!({
                    "name":definition.name,"group":group(definition.name.as_str()),
                    "description":definition.description.chars().take(220).collect::<String>(),
                    "loaded":selected.contains(definition.name.as_str())
                })
            })
            .collect::<Vec<_>>();
        loop {
            let end = offset.saturating_add(page.len()).min(definitions.len());
            let result = json!({"tools":page,"total":definitions.len(),"offset":offset,
                "next_offset":(end < definitions.len()).then_some(end),"load_with":"tools_load; use exact names; schemas arrive next request"});
            if serde_json::to_vec(&result)?.len()
                <= self.config.tools.max_output_bytes.saturating_sub(512)
            {
                return Ok(result);
            }
            if page.len() > 1 {
                page.pop();
            } else if let Some(tool) = page.first_mut() {
                ensure!(
                    tool.get("description").is_some(),
                    "tool identity exceeds output limit"
                );
                tool.as_object_mut().unwrap().remove("description");
            } else {
                bail!("tool search page exceeds output limit");
            }
        }
    }

    pub(super) fn load_tools(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        args: &Value,
    ) -> Result<Value> {
        let requested = string_list(args, "names")?;
        ensure!(
            requested.len() <= MAX_SELECTED,
            "select at most {MAX_SELECTED} tools"
        );
        let catalog = self.discoverable(agent.role);
        for name in &requested {
            ensure!(
                catalog
                    .iter()
                    .any(|definition| definition.name.as_str() == name),
                "tool {name} is unavailable for role {}; use tool_search for permitted names",
                agent.role
            );
        }
        let names = requested
            .into_iter()
            .filter(|name| !baseline(agent.role, name))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        self.effect(agent, call_id, |tx| {
            let current: AgentRecord = Store::tx_get(tx,"agent",&agent.id)?.context("agent missing")?;
            ensure!(current.generation == agent.generation && !current.status.terminal(), "task generation changed or is terminal");
            Store::tx_put(tx,"tool_selection",&agent.id,&agent.run_id,&Selection { generation:agent.generation,names:names.clone() })?;
            Ok(json!({"selected":names,"effective":"next fresh model request","core_tools_retained":true,"generation":agent.generation}))
        })
    }
}
