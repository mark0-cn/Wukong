//! Models systems for the Rig adapter.
use super::*;

fn public_text<'a>(parts: impl IntoIterator<Item = &'a AssistantContent>) -> String {
    parts
        .into_iter()
        .filter_map(|part| match part {
            AssistantContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[expect(
    clippy::type_complexity,
    reason = "Keep Bevy query components and filters visible in system signatures"
)]
pub(super) fn prepare_models(
    mut calls: Query<
        (Entity, &ForAgent, &mut ModelRequest),
        (
            With<Admitted>,
            Without<Started>,
            Without<Outcome>,
            Without<BusinessPrepared>,
        ),
    >,
    mut owners: Query<(&AgentId, &mut Conversation, &mut ToolAccess)>,
    definitions: Query<&ToolDef>,
    application: Res<Application>,
    mut fatal: ResMut<Fatal>,
    mut commands: Commands,
) {
    for (effect, from, mut call) in &mut calls {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let (id, mut conversation, mut access) = owners.get_mut(from.0)?;
                let agent: AgentMetadata = application.runner.agent(&id.0)?;
                if !agent.predecessors_released {
                    return Ok(());
                }
                let run: RunRecord = application.runner.run(&agent.run_id)?;
                call.history = Some(conversation.request_view());
                let config = application.runner.execution();
                let preamble = application.runner.preamble(&agent, &run)?;
                if let Some(prompt) = &mut call.prompt {
                    prompt.role = preamble;
                    if !agent.wait_for_predecessors.is_empty() {
                        let results = agent.wait_for_predecessors.iter().map(|id| application.runner.agent(id).map(|predecessor| json!({"id":id,"status":predecessor.status,"result":predecessor.task_result,"summary":predecessor.summary,"error":predecessor.error}))).collect::<Result<Vec<_>>>()?;
                        prompt.sections.push(rig_core::completion::Section::new(
                            "dependency_results",
                            serde_json::to_string(&results)?,
                        ));
                    }
                    if let Some(section) = prompt
                        .sections
                        .iter_mut()
                        .find(|section| section.tag == "subagent_role")
                    {
                        section.text = "You are an asynchronous analysis subagent. Continue your assigned task with tools. Finish with agent_finish and an accepted TaskResult; Rig will deliver that result as your report. Use send_message_to_agent for information/questions and follow_up_agent for new work; wait for an agent with wait, or use wait_for_agents when a dependency policy is required. The parent may send corrections while you work.".into();
                    }
                }
                call.connection = Some(
                    application
                        .runner
                        .engine_for(agent.role)
                        .connection(application.runner.execution().stream),
                );
                let model_config = application.runner.role_config(agent.role);
                let request = call.request_mut();
                request.max_tokens = Some(model_config.max_tokens);
                request.temperature = model_config.temperature;
                request.additional_params = model_config.additional_params;
                let mut names = application.runner.tool_names(&agent)?;
                names.extend(["task", "message", "wait"].map(str::to_owned));
                // Rebuild from Rig's registry: its initial request was built
                // with the previous selection, which can hide newly loaded tools.
                request.tools = definitions
                    .iter()
                    .filter(|definition| {
                        names.contains(definition.0.name.as_str())
                            && allowed(&application.runner, agent.role, definition.0.name.as_str())
                    })
                    .map(|definition| definition.0.clone())
                    .collect();
                request
                    .tools
                    .sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
                *access = ToolAccess::Only(
                    request
                        .tools
                        .iter()
                        .map(|definition| definition.name.to_string())
                        .collect(),
                );
                // Rig's retention and summary splice preserve tool pairing, author
                // attribution and undo data. Full transcripts remain in its journal
                // and in integrity-checked evidence.
                let chars = serde_json::to_vec(conversation.messages())?.len();
                if chars > config.compact_after_chars {
                    let retained = rig::memory::recent::view(
                        conversation.messages(),
                        &rig::memory::recent::LastMessages(config.keep_recent_messages),
                    );
                    let start = conversation.messages().len() - retained.len();
                    if start > 0 && !retained.is_empty() {
                        let evidence = application.runner.checkpoint_evidence(
                            &run.id,
                            &agent.id,
                            &format!("rig-context:{effect:?}"),
                            &serde_json::to_vec(&conversation.0)?,
                        )?;
                        let keep_from = conversation.id(start);
                        let mut state = rig::memory::summary_state(&conversation.0);
                        state.summary =
                            application
                                .runner
                                .structured_summary(&agent, &run, &evidence.id)?;
                        rig::memory::apply_summary(&mut conversation.0, keep_from, &state);
                        call.history = Some(conversation.request_view());
                        application.runner.emit(&run.id,Some(&agent.id),"context.compacted",json!({"checkpoint_evidence_id":evidence.id,"removed_messages":start,"runtime":"rig-memory"}))?;
                    }
                }
                commands.entity(effect).insert(BusinessPrepared);
                Ok(())
            })(),
        );
    }
}

pub(super) fn account_replies(
    calls: Query<(&ForAgent, &Outcome), (ReadyToSettle, ModelCalls, Without<HostFinal>)>,
    ids: Query<&AgentId>,
    application: Res<Application>,
    mut fatal: ResMut<Fatal>,
) {
    for (from, outcome) in &calls {
        if let Some(response) = outcome.completion() {
            failed(
                &mut fatal,
                (|| -> Result<()> {
                    let agent_id = ids.get(from.0)?.0.clone();
                    let agent: AgentMetadata = application.runner.agent(&agent_id)?;
                    application
                        .runner
                        .account_usage(&agent_id, response.usage, true)?;
                    // Only public assistant text is operator-facing progress.
                    let text = public_text(response.choice.iter());
                    if !text.trim().is_empty() {
                        let excerpt: String = text.chars().take(64 * 1024).collect();
                        application.runner.emit(&agent.run_id, Some(&agent_id), "model.text",
                            json!({"text":excerpt,"truncated":text.chars().count()>64*1024,"public":true}))?;
                    }
                    Ok(())
                })(),
            );
        }
    }
}

pub(super) fn retry_policy(
    mut calls: Query<(&Outcome, &CallOf, &mut RetryVerdict), ReadyToSettle>,
    turns: Query<&Recovery>,
    application: Res<Application>,
) {
    let policy = rig_ecs::retry::RetryPolicy {
        max_retries: application
            .runner
            .execution()
            .model_retries
            .min(u32::MAX as usize) as u32,
        ..rig_ecs::retry::RetryPolicy::DEFAULT
    };
    for (outcome, of, mut verdict) in &mut calls {
        if let Err(error) = &outcome.0 {
            verdict.0 = policy.verdict(
                error,
                turns.get(of.0).map_or(0, |recovery| recovery.retries),
                std::time::SystemTime::now(),
            );
        }
    }
}

pub(super) fn enforce_completion(
    mut turns: Query<(Entity, &TurnOf, &mut TurnOutcome)>,
    ids: Query<&AgentId>,
    nudges: Query<&Nudged>,
    application: Res<Application>,
    mut commands: Commands,
    mut fatal: ResMut<Fatal>,
) {
    for (turn, of, mut outcome) in &mut turns {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let id = ids.get(of.0)?;
                let agent: AgentMetadata = application.runner.agent(&id.0)?;
                if !agent.status.terminal() && matches!(*outcome, TurnOutcome::Answered(_)) {
                    if nudges.get(of.0).is_ok_and(|nudge| nudge.0) {
                        application.runner.fail_agent(
                            &id.0,
                            "agent ended without an accepted lifecycle tool".into(),
                            match &*outcome {
                                TurnOutcome::Answered(message) => message.final_answer(),
                                _ => None,
                            },
                        )?;
                        *outcome = TurnOutcome::Failed("task result not accepted".into());
                    } else {
                        commands.entity(of.0).insert(Nudged(true));
                        commands.entity(turn).remove::<TurnOutcome>();
                        commands.trigger(Deliver::new(of.0, "Call agent_finish (child) or finish_scan (root) with your accepted structured task result, or continue using tools. A plain-text answer does not complete this task.", DeliveryMode::Inform));
                        commands.spawn((TurnCall, CallOf(turn), ForAgent(of.0)));
                    }
                }
                Ok(())
            })(),
        );
    }
}

pub(super) fn record_turn_end(
    settled: On<Add<Settled>>,
    turns: Query<(&ForAgent, &TurnOutcome)>,
    ids: Query<&AgentId>,
    application: Res<Application>,
    mut fatal: ResMut<Fatal>,
) {
    if let Ok((from, outcome)) = turns.get(settled.entity) {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let id = ids.get(from.0)?;
                let record: AgentMetadata = application.runner.agent(&id.0)?;
                if !record.status.terminal()
                    && let TurnOutcome::Failed(reason) = outcome
                {
                    application.runner.fail_agent(&id.0, reason.clone(), None)?;
                }
                Ok(())
            })(),
        );
    }
}

pub(super) fn stream_progress(
    mut events: MessageReader<StreamDelta>,
    ids: Query<&AgentId>,
    application: Res<Application>,
) {
    for event in events.read() {
        if let Some(agent) = event.agent
            && let Ok(id) = ids.get(agent)
        {
            application
                .runner
                .stream_progress(&application.run_id, &id.0);
        }
    }
}

#[cfg(test)]
mod public_progress_tests {
    use super::*;
    #[test]
    fn narration_uses_public_text_and_never_reasoning() {
        let parts = [
            AssistantContent::reasoning("private reasoning"),
            AssistantContent::text("公开进展"),
        ];
        assert_eq!(public_text(&parts), "公开进展");
        assert!(public_text(&parts[..1]).is_empty());
    }
}
