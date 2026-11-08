//! Admission systems for the Rig adapter.
use super::*;

pub(super) fn allowed(runner: &AssessmentRunner, role: Role, name: &str) -> bool {
    matches!(name, "task" | "message" | "wait")
        || (name != "create_agent" && runner.tools.allows(role, name))
}

pub(super) fn tool_access(runner: &AssessmentRunner, agent: &AgentRecord) -> Result<ToolAccess> {
    let mut names = runner.tools.model_tool_names(agent)?;
    names.extend(["task", "message", "wait"].map(str::to_owned));
    Ok(ToolAccess::Only(names.into_iter().collect()))
}

#[expect(
    clippy::type_complexity,
    reason = "Keep Bevy query components and filters visible in system signatures"
)]
pub(super) fn prepare_spawns(
    mut requests: Query<
        (
            Entity,
            &Effect,
            &ForAgent,
            &mut SpawnSubagent,
            Option<&ChildOf>,
        ),
        (Added<Admitted>, Without<SpawnAdmission>),
    >,
    ids: Query<(Entity, &AgentId)>,
    calls: Query<&ToolRequest>,
    application: Res<Application>,
    mut commands: Commands,
) {
    for (entity, effect, from, mut request, parent_call) in &mut requests {
        let result = (|| -> Result<()> {
            let (_, parent) = ids.get(from.0)?;
            let agent: AgentRecord = application
                .runner
                .store
                .require_metadata("agent", &parent.0)?;
            let mut args = Value::Object(request.extra.clone());
            args["name"] = json!(request.title);
            args["task"] = json!(request.brief);
            let call_id = parent_call
                .and_then(|parent| calls.get(parent.parent()).ok())
                .map(|call| business_call(&agent, &call.call.id.to_string()))
                .unwrap_or_else(|| format!("rig-spawn:{}", effect.id()));
            let result = application
                .runner
                .tools
                .create_agent(&agent, &call_id, &args)?;
            let child: AgentRecord = serde_json::from_value(result["agent"].clone())?;
            let existing = ids.iter().find(|(_, id)| id.0 == child.id);
            if let Some((existing, _)) = existing {
                commands.entity(entity).insert((
                    SpawnedAgent(existing),
                    Outcome::custom(json!({"agent":child.id})),
                ));
            }
            request.id = Some(AgentId(child.id.clone()));
            request.model = None;
            request.tools = None;
            request.system_prompt = Some(agents::preamble(
                &child,
                &application.runner.store.require("run", &child.run_id)?,
                &application.runner.store,
                application.runner.config().skills_dir.as_deref(),
                application.runner.config().demos_dir.as_deref(),
            )?);
            commands.entity(entity).insert(SpawnAdmission {
                id: child.id,
                fresh: existing.is_none(),
            });
            Ok(())
        })();
        if let Err(error) = result {
            commands
                .entity(entity)
                .insert(Outcome::refused(format!("{error:#}")));
        }
    }
}

pub(super) fn admit_spawns(
    requests: Query<(Entity, &SpawnAdmission), (With<SpawnSubagent>, ReadyToStart)>,
    application: Res<Application>,
    mut commands: Commands,
    mut fatal: ResMut<Fatal>,
) {
    for (entity, admission) in &requests {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let child: AgentRecord = application
                    .runner
                    .store
                    .require_metadata("agent", &admission.id)?;
                if child.status.terminal() {
                    commands.entity(entity).insert(Outcome::refused(
                        child
                            .error
                            .unwrap_or_else(|| "task cannot be admitted".into()),
                    ));
                }
                Ok(())
            })(),
        );
    }
}

pub(super) fn configure_spawned(
    spawned: Query<(&SpawnedAgent, &SpawnAdmission, &ForAgent), ReadyToSettle>,
    histories: Query<&Conversation>,
    application: Res<Application>,
    mut fatal: ResMut<Fatal>,
    mut commands: Commands,
) {
    for (spawned, admission, from) in &spawned {
        if !admission.fresh {
            continue;
        }
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let child: AgentRecord = application
                    .runner
                    .store
                    .require_metadata("agent", &admission.id)?;
                if child.inherit_context {
                    let parent = histories.get(from.0)?;
                    let pending = parent.pending_calls();
                    // Never manufacture a result for the parent's in-flight tool calls.
                    let end = parent
                        .messages()
                        .iter()
                        .position(|message| match &**message {
                            rig_core::completion::Message::Assistant(assistant) => {
                                assistant.content.iter().any(|content| {
                                    if let AssistantContent::ToolCall(call) = content {
                                        pending.iter().any(|pending| pending.id == call.id)
                                    } else {
                                        false
                                    }
                                })
                            }
                            _ => false,
                        })
                        .unwrap_or(parent.messages().len());
                    let history = parent.messages()[..end].to_vec();
                    rig_core::conversation::pairing::validate(&history)?;
                    commands.entity(spawned.0).insert(Conversation(
                        rig_core::conversation::Conversation::from_messages(history),
                    ));
                }
                Ok(())
            })(),
        );
        if let Ok(child) = application
            .runner
            .store
            .require_metadata::<AgentRecord>("agent", &admission.id)
        {
            let connection = application
                .runner
                .engine_for(child.role)
                .connection(application.runner.config().execution.stream);
            let choice = ModelChoice(connection.spec.reference());
            match tool_access(&application.runner, &child) {
                Ok(access) => {
                    commands
                        .entity(spawned.0)
                        .insert((BusinessTask, access, connection));
                }
                Err(error) => {
                    fatal.0.get_or_insert(error);
                }
            }
            commands
                .entity(spawned.0)
                .insert(choice)
                .remove::<ModelRefused>();
        }
    }
}

#[expect(
    clippy::type_complexity,
    reason = "Keep Bevy query components and filters visible in system signatures"
)]
pub(super) fn configure_agents(
    agents: Query<(Entity, &AgentId), (With<Agent>, Without<BusinessTask>)>,
    application: Res<Application>,
    mut commands: Commands,
    mut fatal: ResMut<Fatal>,
) {
    for (entity, id) in &agents {
        if let Ok(Some(agent)) = application
            .runner
            .store
            .get_metadata::<AgentRecord>("agent", &id.0)
        {
            let connection = application
                .runner
                .engine_for(agent.role)
                .connection(application.runner.config().execution.stream);
            let choice = ModelChoice(connection.spec.reference());
            match tool_access(&application.runner, &agent) {
                Ok(access) => {
                    commands
                        .entity(entity)
                        .insert((BusinessTask, access, connection));
                }
                Err(error) => {
                    fatal.0.get_or_insert(error);
                }
            }
            commands
                .entity(entity)
                .insert(choice)
                .remove::<ModelRefused>();
        }
    }
}

#[expect(
    clippy::type_complexity,
    reason = "Keep Bevy query components and filters visible in system signatures"
)]
pub(super) fn gate_tools(
    mut calls: Query<
        (Entity, &ForAgent, &ToolRequest, &mut Decision),
        (ReadyToStart, Without<HostFinal>),
    >,
    ids: Query<&AgentId>,
    incoming: Query<&ForAgent, (With<Incoming>, Without<Settled>, Without<DeliveredIn>)>,
    application: Res<Application>,
    mut commands: Commands,
) {
    for (entity, from, request, mut decision) in &mut calls {
        let result = (|| -> Result<()> {
            let id = ids.get(from.0)?;
            let agent: AgentRecord = application.runner.store.require_metadata("agent", &id.0)?;
            ensure!(
                allowed(
                    &application.runner,
                    agent.role,
                    request.call.function.name.as_str()
                ),
                "tool unavailable to this role"
            );
            let name = request.call.function.name.as_str();
            if name == "message" {
                let wanted = request.call.function.arguments["agent"]
                    .as_str()
                    .unwrap_or_default();
                let targets = application
                    .runner
                    .store
                    .list_metadata::<AgentRecord>("agent", &agent.run_id)?;
                let matching: Vec<_> = targets
                    .iter()
                    .filter(|target| {
                        target.id == wanted || AgentId(target.id.clone()).short() == wanted
                    })
                    .collect();
                ensure!(matching.len() == 1, "use an unambiguous agent ID");
                ensure!(
                    !matching[0].status.terminal(),
                    "completed work requires follow_up_agent"
                );
            }
            if matches!(name, "agent_finish" | "finish_scan") {
                ensure!(
                    !incoming.iter().any(|message| message.0 == from.0),
                    "unread native messages remain; continue at the next model boundary before finishing"
                );
            }
            let run: RunRecord = application.runner.store.require("run", &agent.run_id)?;
            ensure!(!agent.status.terminal(), "task is not executable");
            if run.status == Status::BudgetPaused {
                commands.spawn((HoldOf(entity), AdmissionHold));
            }
            Ok(())
        })();
        if let Err(error) = result {
            decision.decide(
                Decider::plugin("task-policy"),
                Outcome::refused(format!("{error:#}")),
            );
        }
    }
}

pub(super) fn gate_models(
    mut calls: Query<(Entity, &ForAgent, &mut Decision), (ReadyToStart, ModelCalls)>,
    ids: Query<&AgentId>,
    active: Query<(), (ModelCalls, With<Started>, Without<Settled>)>,
    application: Res<Application>,
    mut commands: Commands,
    mut fatal: ResMut<Fatal>,
) {
    let mut slots = active.iter().count();
    for (entity, from, mut decision) in &mut calls {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let id = ids.get(from.0)?;
                let agent: AgentRecord =
                    application.runner.store.require_metadata("agent", &id.0)?;
                let run: RunRecord = application.runner.store.require("run", &agent.run_id)?;
                if agent.status == Status::Completed {
                    let response = CompletionResponse::new(
                        vec![AssistantContent::text(
                            agent
                                .task_result
                                .as_ref()
                                .map(|result| serde_json::to_string(result).unwrap())
                                .or(agent.summary)
                                .unwrap_or_default(),
                        )],
                        Usage::default(),
                        rig_core::message::Origin::new("host", "re-agent", "task-result"),
                        Value::Null,
                    );
                    decision.decide(
                        Decider::plugin("accepted-task-result"),
                        Outcome(Ok(Answered::Completion(response.into()))),
                    );
                    commands.entity(entity).insert(HostFinal);
                } else if agent.status.terminal() {
                    commands.trigger(StopTurn::new(from.0));
                } else if !agent.predecessors_released {
                    commands.spawn((HoldOf(entity), PredecessorHold));
                } else if run.status == Status::BudgetPaused {
                    commands.spawn((HoldOf(entity), AdmissionHold));
                } else if run.total_tokens >= run.budget.max_total_tokens
                    || run.budget.max_usd.is_some_and(|max| run.cost_usd >= max)
                {
                    application
                        .runner
                        .coordinator
                        .pause_budget(&run.id, "model token or USD budget reached")?;
                    commands.spawn((HoldOf(entity), AdmissionHold));
                } else if agent.token_cap.is_some_and(|cap| agent.total_tokens >= cap)
                    || agent.turns >= run.budget.max_turns_per_agent
                {
                    application.runner.fail_agent(
                        &agent.id,
                        "agent model budget reached".into(),
                        None,
                    )?;
                    commands.trigger(StopTurn::new(from.0));
                } else if slots >= run.budget.max_concurrent_agents {
                    commands.spawn((HoldOf(entity), ModelSlot));
                } else {
                    slots += 1;
                    application
                        .runner
                        .coordinator
                        .set_status(&agent.id, Status::Running, None)?;
                }
                Ok(())
            })(),
        );
    }
}

pub(super) fn release_model_slots(
    holds: Query<(Entity, &HoldOf), With<ModelSlot>>,
    active: Query<(), (ModelCalls, With<Started>, Without<Settled>)>,
    application: Res<Application>,
    mut commands: Commands,
) {
    let Ok(run) = application
        .runner
        .store
        .require::<RunRecord>("run", &application.run_id)
    else {
        return;
    };
    let available = run
        .budget
        .max_concurrent_agents
        .saturating_sub(active.iter().count());
    for (hold, _) in holds.iter().take(available) {
        commands.entity(hold).despawn();
    }
}

pub(super) fn release_predecessors(
    holds: Query<(Entity, &HoldOf), With<PredecessorHold>>,
    owners: Query<&ForAgent>,
    ids: Query<&AgentId>,
    application: Res<Application>,
    mut commands: Commands,
) {
    for (hold, of) in &holds {
        if let Ok(owner) = owners.get(of.0)
            && let Ok(id) = ids.get(owner.0)
            && application
                .runner
                .store
                .require_metadata::<AgentRecord>("agent", &id.0)
                .is_ok_and(|agent| agent.predecessors_released || agent.status.terminal())
        {
            commands.entity(hold).despawn();
        }
    }
}
