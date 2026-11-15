//! Coordination tool handlers.
use super::*;

pub(in crate::tools) fn handle_create_agent<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.create_agent(agent, call_id, &args) })
}

pub(in crate::tools) fn handle_view_agent_graph<'a>(
    tools: &'a ToolContext<'_>,
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

pub(in crate::tools) fn handle_send_message_to_agent<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.send_message(agent, call_id, &args, name == "follow_up_agent") })
}

pub(in crate::tools) fn handle_message_status<'a>(
    tools: &'a ToolContext<'_>,
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

pub(in crate::tools) fn handle_wait_for_agents<'a>(
    tools: &'a ToolContext<'_>,
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

pub(in crate::tools) fn handle_stop_agent<'a>(
    tools: &'a ToolContext<'_>,
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

pub(in crate::tools) fn handle_agent_finish<'a>(
    tools: &'a ToolContext<'_>,
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

pub(in crate::tools) fn handle_load_skill<'a>(
    tools: &'a ToolContext<'_>,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.activate_skill(agent, call_id, text(&args, "name")?) })
}

pub(in crate::tools) fn handle_think<'a>(
    tools: &'a ToolContext<'_>,
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
