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
            json!({"agents":tools.store.records::<AgentMetadata>(&agent.run_id)?.iter().map(|a|json!({"id":a.id,"parent_id":a.parent_id,"name":a.name,"role":a.role,"task":a.task,"scope":a.scope,"contract":a.contract,"result":a.task_result,"status":a.status,"generation":a.generation,"dependency_policy":a.dependency_policy,"predecessors":a.wait_for_predecessors,"predecessors_released":a.predecessors_released,"wait_policy":a.wait_policy,"wait_for":a.wait_for,"summary":a.summary,"error":a.error,"turns":a.turns,"total_tokens":a.total_tokens})).collect::<Vec<_>>(),"open_sessions":sessions}),
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
            let target: AgentMetadata =
                require_in_run(&tools.store, text(&args, "agent_id")?, &agent.run_id)?;
            tools.authorize_control(agent, &target)?;
            target.id.clone()
        };
        tools.effect(agent, call_id, |tx| {
            Ok(json!({"stopped":Coordinator::tx_stop(tx, &target_id, text(&args, "reason")?)?}))
        })
    })
}

/// Literal placeholder flags — `flag{...}`, `Flag{.}`, `flag{AAAA...}`,
/// `flag{test}` and escaped variants — must never pass as a submitted result.
fn placeholder_flag_claim(answer: &str) -> bool {
    let lower = answer.to_lowercase();
    for (_, rest) in lower.match_indices("flag") {
        let Some(after) = rest.strip_prefix("{") else {
            continue;
        };
        let Some(close) = after.find('}') else {
            continue;
        };
        let core: String = after[..close]
            .chars()
            .filter(|c| !matches!(c, '\\' | '_'))
            .collect();
        let placeholder = core.is_empty()
            || core
                .chars()
                .all(|c| matches!(c, '.' | '…' | '+' | '-' | ' ' | '*' | 'x' | 'a'))
            || core == "test"
            || (core.len() >= 3
                && core
                    .chars()
                    .next()
                    .is_some_and(|first| core.chars().all(|c| c == first)));
        if placeholder {
            return true;
        }
    }
    false
}

pub(super) fn handle_emulate_run<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        tools
            .execution
            .files
            .known_file(agent, text(&args, "target")?, true)?;
        tools
            .execution
            .gateway
            .worker_call(agent, call_id, "emu.run", args)
            .await
    })
}

pub(super) fn handle_libc_identify<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        tools
            .execution
            .gateway
            .worker_call(agent, call_id, "libc.identify", args)
            .await
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
        ensure!(
            !placeholder_flag_claim(&result.answer),
            "the answer contains a placeholder flag (flag{{...}}, Flag{{.}} and similar). Submit only a value observed in the target's own output, with its evidence id in evidence_ids; a bounded negative result is 'not found in scope X'."
        );
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
        Ok(json!({"notes":tools.store.documents(DocumentKind::Note,&agent.run_id)?}))
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
        Ok(json!({"todos":tools.store.documents(DocumentKind::Todo,&agent.run_id)?}))
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
    Box::pin(async move { Ok(json!({"coverage":tools.store.records::<Coverage>(&agent.run_id)?})) })
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
    Box::pin(async move { Ok(json!({"findings":tools.store.records::<Finding>(&agent.run_id)?})) })
}

pub(super) fn handle_finding_get<'a>(
    tools: &'a ToolService,
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
            .records::<crate::domain::Thought>(&agent.run_id)?
            .into_iter()
            .filter(|t| t.agent_id == agent.id)
            .count();
        tools.store.save_record(&thought)?;
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
            "full_bytes": std::fs::metadata(&run.target_path)
                .map(|metadata| metadata.len())
                .unwrap_or(0),
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

pub(super) fn handle_analysis_verify<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        ensure!(
            agent.role == Role::Verifier,
            "independent verifier required"
        );
        let input: args::AnalysisVerifyArgs = serde_json::from_value(args.clone())?;
        for field in [&input.claim, &input.scope, &input.expected, &input.observed] {
            ensure!(
                !field.trim().is_empty() && field.len() <= 16384,
                "invalid verification text"
            );
        }
        ensure!(
            input.counterexamples.len() <= 32
                && input.counterexamples.iter().all(|s| s.len() <= 4096),
            "too many counterexamples"
        );
        ensure!(
            !input.evidence_ids.is_empty() && input.evidence_ids.len() <= 128,
            "verification requires bounded evidence"
        );
        tools
            .store
            .verify_evidence_links(&agent.run_id, &input.evidence_ids)?;
        tools.effect(agent, call_id, |tx| {
            let producer: AgentMetadata = tx
                .get("agent", &input.producer_id)?
                .context("claim producer missing")?;
            ensure!(
                producer.run_id == agent.run_id
                    && producer.id != agent.id
                    && producer.role != Role::Verifier,
                "invalid independent producer"
            );
            let mut own = false;
            for id in &input.evidence_ids {
                let evidence: Evidence = tx.get_record(id)?.context("evidence missing")?;
                ensure!(evidence.run_id == agent.run_id, "cross-run evidence");
                own |= evidence.agent_id == agent.id;
            }
            ensure!(own, "verification requires verifier-owned observations");
            let run: RunRecord = tx.get("run", &agent.run_id)?.context("run missing")?;
            let id = stable("analysis-verification", call_id);
            let record = json!({"id":id,"run_id":agent.run_id,"verifier_id":agent.id,
                "target_sha256":run.target_sha256,"created_at":now(),"result":args});
            ensure!(
                tx.get::<Value>("analysis_verification", &id)?.is_none(),
                "verification is immutable"
            );
            tx.put("analysis_verification", &id, &agent.run_id, &record)?;
            let note = SharedDocument {
                id: id.clone(),
                run_id: agent.run_id.clone(),
                agent_id: agent.id.clone(),
                kind: "note".into(),
                title: format!("Independent verification: {}", input.scope),
                body: serde_json::to_string_pretty(&record)?,
                amendments: vec![],
                created_at: now(),
                updated_at: now(),
            };
            tx.save_document(&note)?;
            tx.event(
                &agent.run_id,
                Some(&agent.id),
                "analysis.verified",
                record.clone(),
            )?;
            Ok(record)
        })
    })
}

pub(super) fn handle_differential_test<'a>(
    tools: &'a ToolService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::DifferentialTestArgs = serde_json::from_value(args)?;
        ensure!(
            (1..=64).contains(&input.inputs_base64.len())
                && (1..=10).contains(&input.case_timeout_seconds),
            "invalid differential budget"
        );
        for encoded in &input.inputs_base64 {
            ensure!(
                BASE64.decode(encoded)?.len() <= 65536,
                "input vector too large"
            );
        }
        tools
            .execution
            .files
            .known_file(agent, &input.target, true)?;
        tools
            .execution
            .files
            .known_file(agent, &input.reference, false)?;
        let config = serde_json::to_string(
            &json!({"inputs":input.inputs_base64,"timeout":input.case_timeout_seconds,"python":input.reference_python}),
        )?;
        tools
            .execution
            .runtime_tool(
                agent,
                call_id,
                "py_run",
                json!({
                    "source":include_str!("../../workers/runtime/differential_runner.py"),
                    "argv":[input.reference,config], "target":input.target,
                    "timeout_secs":(input.case_timeout_seconds*128+20).min(3600)
                }),
            )
            .await
    })
}
