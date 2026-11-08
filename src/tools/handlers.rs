//! Registered builtin handlers. The catalog owns routing and recovery metadata.
use super::*;
use futures::future::BoxFuture;

pub(super) fn handle_create_agent<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.create_agent(agent, call_id, &args) })
}

pub(super) fn handle_view_agent_graph<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let mut sessions = Vec::new();
        for kind in ["debug_session", "interact_session"] {
            sessions.extend(tools.store.list::<Value>(kind, &agent.run_id)?.iter()
                        .filter(|session| !matches!(session["state"].as_str(), Some("closed" | "lost")))
                        .map(|session| json!({"kind":kind,"id":session["id"],"agent_id":session["agent_id"],"state":session["state"]})));
        }
        Ok(
            json!({"agents":tools.store.list_metadata("agent",&agent.run_id)?.iter().map(|a|json!({"id":a.id,"parent_id":a.parent_id,"name":a.name,"role":a.role,"task":a.task,"scope":a.scope,"contract":a.contract,"result":a.task_result,"status":a.status,"generation":a.generation,"dependency_policy":a.dependency_policy,"predecessors":a.wait_for_predecessors,"predecessors_released":a.predecessors_released,"wait_policy":a.wait_policy,"wait_for":a.wait_for,"summary":a.summary,"error":a.error,"turns":a.turns,"total_tokens":a.total_tokens})).collect::<Vec<_>>(),"open_sessions":sessions}),
        )
    })
}

pub(super) fn handle_send_message_to_agent<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.send_message(agent, call_id, &args, name == "follow_up_agent") })
}

pub(super) fn handle_message_status<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let mid = number(&args, "message_id", 0) as i64;
        let message = tools
            .store
            .write(|tx| {
                Ok(tx
                    .messages(&agent.run_id)?
                    .into_iter()
                    .find(|message| message.id == mid))
            })?
            .context("message not in this run")?;
        ensure!(
            agent.role == Role::Root || message.sender == agent.id || message.recipient == agent.id,
            "message is not addressed to or sent by this agent"
        );
        Ok(serde_json::to_value(message)?)
    })
}

pub(super) fn handle_wait_for_agents<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let policy =
            serde_json::from_value(args.get("policy").cloned().unwrap_or(json!("all_terminal")))?;
        tools.effect(agent, call_id, |tx| {
            Coordinator::tx_wait(
                tx,
                &agent.id,
                &string_list(&args, "agent_ids")?,
                number(&args, "timeout_seconds", 300),
                policy,
            )
        })
    })
}

pub(super) fn handle_stop_agent<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let target_id = {
            let target: AgentMetadata = require_in_run(
                &tools.store,
                "agent",
                text(&args, "agent_id")?,
                &agent.run_id,
            )?;
            tools.authorize_control(agent, &target)?;
            target.id.clone()
        };
        tools.effect(agent, call_id, |tx| {
            Ok(json!({"stopped":Coordinator::tx_stop(tx, &target_id, text(&args, "reason")?)?}))
        })
    })
}

pub(super) fn handle_agent_finish<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let result: TaskResult = if let Some(result) = args.get("result") {
            // result.answer is the canonical summary; a free-text
            // summary argument is accepted but never cross-checked
            // (models reliably produce both, and equality checks
            // just burn retry turns).
            serde_json::from_value(result.clone())?
        } else {
            TaskResult {
                answer: text(&args, "summary")?.into(),
                ..Default::default()
            }
        };
        tools
            .store
            .verify_evidence_links(&agent.run_id, &result.evidence_links())?;
        tools.effect(agent, call_id, |tx| {
            Coordinator::tx_finish_result(tx, &agent.id, result)?;
            Ok(json!({"finished":true,"agent_id":agent.id}))
        })
    })
}

pub(super) fn handle_threat_model_get<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        if let Some(id) = args["id"].as_str() {
            Ok(serde_json::to_value(require_in_run::<SharedDocument>(
                &tools.store,
                "threat_model",
                id,
                &agent.run_id,
            )?)?)
        } else {
            Ok(
                json!({"documents":tools.store.list::<SharedDocument>("threat_model",&agent.run_id)?}),
            )
        }
    })
}

pub(super) fn handle_threat_model_save<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.save_document(agent, call_id, name, &args) })
}

pub(super) fn handle_threat_model_amend<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.amend_document(agent, call_id, name, &args) })
}

pub(super) fn handle_notes_list<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        Ok(json!({"notes":tools.store.list::<SharedDocument>("note",&agent.run_id)?}))
    })
}

pub(super) fn handle_context_search<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        crate::context::search(&tools.store, agent, &args, tools.limits.max_output_bytes)
    })
}

pub(super) fn handle_context_read<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        crate::context::read(&tools.store, agent, &args, tools.limits.max_output_bytes)
    })
}

pub(super) fn handle_todos_list<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        Ok(json!({"todos":tools.store.list::<SharedDocument>("todo",&agent.run_id)?}))
    })
}

pub(super) fn handle_coverage_record<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.coverage(agent, call_id, name, &args) })
}

pub(super) fn handle_coverage_list<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        Ok(json!({"coverage":tools.store.list::<Coverage>("coverage",&agent.run_id)?}))
    })
}

pub(super) fn handle_finding_file<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.finding(agent, call_id, name, &args) })
}

pub(super) fn handle_findings_list<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        Ok(json!({"findings":tools.store.list::<Finding>("finding",&agent.run_id)?}))
    })
}

pub(super) fn handle_finding_get<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let finding: Finding =
            require_in_run(&tools.store, "finding", text(&args, "id")?, &agent.run_id)?;
        let verification = finding
            .verification_id
            .as_ref()
            .map(|id| {
                require_in_run::<Verification>(&tools.store, "verification", id, &agent.run_id)
            })
            .transpose()?;
        Ok(json!({"finding":finding,"verification":verification}))
    })
}

pub(super) fn handle_verification_record<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        tools
            .execution
            .jobs
            .verification(agent, call_id, &args)
            .await
    })
}

pub(super) fn handle_evidence_read<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let evidence_id = text(&args, "evidence_id")?.to_owned();
        let record: Evidence =
            require_in_run(&tools.store, "evidence", &evidence_id, &agent.run_id)?;
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

pub(super) fn handle_load_skill<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.activate_skill(agent, call_id, text(&args, "name")?) })
}

pub(super) fn handle_think<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    __name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let content = text(&args, "content")?;
        ensure!(
            content.chars().count() <= 8000,
            "think content exceeds 8000 characters"
        );
        let thought = crate::domain::Thought {
            id: stable("thought", call_id),
            run_id: agent.run_id.clone(),
            agent_id: agent.id.clone(),
            content: content.to_owned(),
            created_at: now(),
        };
        let thoughts = tools
            .store
            .list::<crate::domain::Thought>("thought", &agent.run_id)?
            .into_iter()
            .filter(|t| t.agent_id == agent.id)
            .count();
        tools
            .store
            .put("thought", &thought.id, &agent.run_id, &thought)?;
        Ok(json!({"recorded":true,"chars":content.chars().count(),"thoughts":thoughts + 1}))
    })
}

pub(super) fn handle_binary_inspect<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    __name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let run = target(&tools.store, agent)?;
        let full = serde_json::to_value(binary::inspect(&run.target_path)?)?;
        let bytes = serde_json::to_vec(&full)?;
        let evidence = tools.store.artifact(
                    &agent.run_id,
                    &agent.id,
                    call_id,
                    "binary_inspect.full",
                    &bytes,
                    "application/json",
                    json!({"raw_tool_output":true,"tool":"binary_inspect","target_sha256":run.target_sha256}),
                )?;
        let pick = |key: &str| full.get(key).cloned().unwrap_or(Value::Null);
        let count = |key: &str| {
            full.get(key)
                .and_then(Value::as_array)
                .map_or(0, |a| a.len())
        };
        Ok(json!({
            "format": pick("format"),
            "architecture": pick("architecture"),
            "bits": pick("bits"),
            "endianness": pick("endianness"),
            "kind": pick("kind"),
            "image_base": pick("image_base"),
            "entry": pick("entry"),
            "mitigations": pick("mitigations"),
            "libraries": pick("libraries"),
            "has_debug_symbols": pick("has_debug_symbols"),
            "warnings": pick("warnings"),
            "sha256": pick("sha256"),
            "size": pick("size"),
            "imports_count": count("imports"),
            "sections_count": count("sections"),
            "symbols_count": count("symbols"),
            "full_evidence_id": evidence.id,
            "full_bytes": bytes.len(),
        }))
    })
}

pub(super) fn handle_binary_strings<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let run = target(&tools.store, agent)?;
        let query: StringsQuery = serde_json::from_value(args)?;
        Ok(serde_json::to_value(binary::strings(
            &run.target_path,
            &query,
        )?)?)
    })
}

pub(super) fn handle_functions_search<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.analysis.ghidra(agent, call_id, name, &args).await })
}

pub(super) fn handle_runtime_doctor<'a>(
    tools: &'a ToolService,
    _agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.doctor().await })
}

pub(super) fn handle_stage_target<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.execution.files.stage_target(agent, call_id).await })
}

pub(super) fn handle_file_put<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.execution.files.file_put(agent, call_id, &args).await })
}

pub(super) fn handle_file_get<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.execution.files.file_get(agent, call_id, &args).await })
}

pub(super) fn handle_job_start<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        tools
            .execution
            .runtime_tool(agent, call_id, name, args)
            .await
    })
}
