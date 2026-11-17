//! Coordination tool handlers.
use super::*;

pub(in crate::application::tools) fn handle_create_agent<'a>(
    tools: &'a super::super::coordination::CoordinationService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.create_agent(agent, call_id, &args) })
}

pub(in crate::application::tools) fn handle_view_agent_graph<'a>(
    tools: &'a super::super::coordination::CoordinationService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let agents = tools.store.records::<AgentMetadata>(&agent.run_id)?;
        if let Some(reference) = args["agent_id"].as_str() {
            let target = TaskGraph::resolve(&agents, reference)?;
            return Ok(json!({"caller_id":agent.id,"agent":target}));
        }
        let offset = crate::application::tools::host_usize(
            number(&args, "offset", 0).min(agents.len() as u64),
        );
        let limit = crate::application::tools::host_usize(number(&args, "limit", 8).clamp(1, 16));
        let page: Vec<_> = agents.iter().skip(offset).take(limit).collect();
        let brief = |text: &str| text.chars().take(160).collect::<String>();
        let mut sessions = Vec::new();
        for kind in ["debug_session", "interact_session"] {
            sessions.extend(tools.store.list::<Value>(kind, &agent.run_id)?.iter()
                        .filter(|session| !matches!(session["state"].as_str(), Some("closed" | "lost")) && page.iter().any(|a| session["agent_id"].as_str() == Some(a.id.as_str())))
                        .map(|session| json!({"kind":kind,"id":session["id"],"agent_id":session["agent_id"],"state":session["state"]})));
        }
        Ok(
            json!({"caller_id":agent.id,"parent_id":agent.parent_id,"total":agents.len(),"next_offset":(offset+page.len()<agents.len()).then_some(offset+page.len()),"agents":page.iter().map(|a|json!({"id":a.id,"parent_id":a.parent_id,"name":brief(&a.name),"role":a.role,"scope":brief(&a.scope),"status":a.status,"generation":a.generation,"error":a.error.as_deref().map(brief),"can_follow_up":a.status==Status::Completed || (a.status==Status::Failed && a.error.as_deref().is_some_and(|e|e.contains("budget")))})).collect::<Vec<_>>(),"open_sessions":sessions}),
        )
    })
}

pub(in crate::application::tools) fn handle_send_message_to_agent<'a>(
    tools: &'a super::super::coordination::CoordinationService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.send_message(agent, call_id, &args, name == "follow_up_agent") })
}

pub(in crate::application::tools) fn handle_message_status<'a>(
    tools: &'a super::super::coordination::CoordinationService,
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

pub(in crate::application::tools) fn handle_wait_for_agents<'a>(
    tools: &'a super::super::coordination::CoordinationService,
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

pub(in crate::application::tools) fn handle_stop_agent<'a>(
    tools: &'a super::super::coordination::CoordinationService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let target_id = {
            let agents = tools.store.records::<AgentMetadata>(&agent.run_id)?;
            let target = TaskGraph::resolve(&agents, text(&args, "agent_id")?)?;
            tools.authorize_control(agent, target)?;
            target.id.clone()
        };
        tools.effect(agent, call_id, |tx| {
            Ok(json!({"stopped":Coordinator::tx_stop(tx, &target_id, text(&args, "reason")?)?}))
        })
    })
}

pub(in crate::application::tools) fn handle_agent_finish<'a>(
    tools: &'a super::super::coordination::CoordinationService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let result: TaskResult = serde_json::from_value(args["result"].clone())?;
        ensure!(
            !flag_claims(&result.answer).placeholder,
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

pub(in crate::application::tools) fn handle_load_skill<'a>(
    tools: &'a super::super::coordination::CoordinationService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.activate_skill(agent, call_id, text(&args, "name")?) })
}

pub(in crate::application::tools) fn handle_tool_selection<'a>(
    tools: &'a super::super::coordination::CoordinationService,
    agent: &'a AgentMetadata,
    call: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        if name == "tool_select" {
            tools.select_tools(agent, call, args)
        } else {
            tools.tool_catalog(agent, &args)
        }
    })
}
