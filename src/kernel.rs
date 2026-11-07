//! Rig owns execution, conversations, async subagents and effect cancellation.
//! This plugin supplies only application admission, evidence and completion gates.
use crate::{
    agents,
    domain::{self, AgentRecord, Role, RunRecord, Status},
    runner::AssessmentRunner,
};
use anyhow::{Result, ensure};
use rig_basics::subagents::{
    OpenRequests, Report, Request, SpawnSubagent, SpawnedAgent, SubagentPolicy, SubagentsPlugin,
};
use rig_core::{
    completion::{CompletionResponse, ToolDefinition, Usage},
    effect::{self, Answered, EffectKind, FamilyDescriptor, HandlerDescriptor},
    serve::{Dispatch, ErasedHandler, Reply, Serve},
    tool::{ToolExecutionError, ToolOutput},
};
use rig_ecs::session_log::SessionStore;
use rig_harness::prelude::*;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

#[derive(Resource, Clone)]
struct Application {
    runner: AssessmentRunner,
    run_id: String,
    runtime: tokio::runtime::Handle,
}
#[derive(Resource, Default)]
struct Fatal(Option<anyhow::Error>);
#[derive(Component)]
struct BusinessTask;
#[derive(Component)]
struct SpawnAdmission {
    id: String,
    fresh: bool,
}
#[derive(Component)]
struct AdmissionHold;
#[derive(Component)]
struct MailDelivery {
    id: i64,
    generation: u64,
}
#[derive(Component)]
struct HostFinal;
#[derive(Component)]
struct BusinessPrepared;
#[derive(Component)]
struct PredecessorHold;
#[derive(Component, Default)]
struct Nudged(bool);

fn failed(app: &mut Fatal, result: Result<()>) {
    if let Err(error) = result {
        app.0.get_or_insert(error);
    }
}

/// A windowless Rig app. Wakes come from its effects; the one-second heartbeat
/// serves leases, wall-clock limits and externally submitted operator messages.
pub(crate) fn drive(
    runner: AssessmentRunner,
    run_id: &str,
    owner: &str,
    cancel: CancellationToken,
    runtime: tokio::runtime::Handle,
) -> Result<RunRecord> {
    let mut run: RunRecord = runner.store.require("run", run_id)?;
    ensure!(!run.status.terminal(), "cannot drive a terminal assessment");
    runner.coordinator.resume(run_id, None)?;
    let directory = runner.config().state_dir.join("rig").join(run_id);
    let root: AgentRecord = runner.store.require("agent", &run.root_id)?;
    ensure!(
        root.checkpoint.is_none()
            || directory
                .join("logs")
                .join(format!("{}.jsonl", run.root_id))
                .is_file(),
        "this run has no native Rig journal; start a new assessment"
    );
    if root.checkpoint.is_none() {
        // Freeze the resolved endpoint, including environment fallback, before
        // any model IO. Credentials remain environment-variable names only.
        run.config = serde_json::to_value(runner.config())?;
        runner.store.put("run", run_id, run_id, &run)?;
    } else {
        let saved: crate::config::AppConfig = serde_json::from_value(run.config.clone())?;
        ensure!(
            serde_json::to_value(&saved.model)? == serde_json::to_value(&runner.config().model)?,
            "native session model configuration changed; use the saved provider/endpoint configuration"
        );
    }
    let journal = rig::memory::DirStore::open(directory)?;
    let (wake_tx, wakes) = mpsc::channel();
    let store_wake = wake_tx.clone();
    let mut changes = runner.store.subscribe_changes();
    let watcher = runtime.spawn(async move {
        while changes.changed().await.is_ok() {
            if store_wake.send(()).is_err() {
                break;
            }
        }
    });
    struct Watcher(tokio::task::AbortHandle);
    impl Drop for Watcher {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _watcher = Watcher(watcher.abort_handle());
    let mut app = App::new();
    app.add_plugins(TaskPoolPlugin::default())
        .insert_resource(Wake::new(move || {
            let _ = wake_tx.send(());
        }))
        .insert_resource(SessionStore::new(journal))
        .insert_resource(Application {
            runner: runner.clone(),
            run_id: run_id.into(),
            runtime,
        })
        .init_resource::<Fatal>()
        .add_plugins((AgentPlugin, SessionLogPlugin))
        .insert_resource(SubagentPolicy {
            max_depth: run.budget.max_depth as usize,
        })
        .add_plugins((
            SubagentsPlugin,
            rig_basics::telemetry::effect_log::EffectLogPlugin,
        ))
        .add_systems(
            AgentPass,
            (
                prepare_spawns.in_set(PrepareSystems::Edit),
                inform_terminal.in_set(PrepareSystems::Edit),
                bind_reports.in_set(PrepareSystems::Edit),
                acknowledge_mail.in_set(Stage::Gate).before(admit_spawns),
                configure_agents.in_set(Stage::Admit),
                carry_native_turns.in_set(Stage::Prepare),
                ensure_owner_requests
                    .in_set(Stage::Prepare)
                    .before(prepare_models),
                release_model_slots.in_set(Stage::Prepare),
                release_predecessors.in_set(Stage::Prepare),
                prepare_calls.in_set(PrepareSystems::Edit),
                reconcile_calls.in_set(PrepareSystems::Edit),
                prepare_models
                    .in_set(PrepareSystems::Edit)
                    .after(prepare_spawns),
                (admit_spawns, gate_models, gate_tools)
                    .chain()
                    .in_set(Stage::Gate),
                (account_replies, configure_spawned, enforce_completion)
                    .chain()
                    .in_set(ReviewSystems::Edit),
                retry_policy.in_set(ReviewSystems::Edit),
            ),
        )
        .add_observer(record_turn_end)
        .add_systems(PostUpdate, stream_progress)
        .add_systems(Last, (project_conversations, acknowledge_mail));

    // Native task spawning is extended with our typed business contract. Its
    // generic spawn, reports and cancellation remain SubagentsPlugin's.
    let task_schema = task_schema();
    for mut def in app
        .world_mut()
        .query::<&mut ToolDef>()
        .iter_mut(app.world_mut())
    {
        let definition = Arc::make_mut(&mut def.0);
        match definition.name.as_str() {
            "task" => {
                definition.parameters = task_schema.clone();
                definition.description = "Start an asynchronous analysis subagent. Supply a short description, a full prompt, role and scope; include known evidence, constraints and expected outputs. Optional skills, inherit_context, contract, wait_for_predecessors and dependency_policy refine the assignment. The call returns before work finishes; predecessor gates hold the child's model requests until satisfied. Model choice and tool permissions come from controller configuration and the role. Inspect the agent graph before duplicating work. Use follow_up_agent for new work on a completed child, wait for one agent's report, or wait_for_agents for a group or success policy.".into();
            }
            "message" => {
                definition.description = "Send a request, correction or reply to a reachable active agent using agent and text. Use send_message_to_agent with kind=information for progress-only updates, which must not release waits. New work on a completed agent requires follow_up_agent.".into();
            }
            "wait" => {
                definition.description = "Wait for a requested reply or report from one reachable agent. Receiving a reply does not establish task success. Use wait_for_agents to collect multiple tasks or require all_success, all_terminal or any_success; inspect their structured outcomes.".into();
            }
            _ => {}
        }
    }
    let mut definitions = HashMap::new();
    for role in [
        Role::Root,
        Role::Recon,
        Role::Reverse,
        Role::Hunter,
        Role::Fuzz,
        Role::Crash,
        Role::Exploit,
        Role::Verifier,
        Role::Reporter,
    ] {
        for definition in runner.tools.definitions(role) {
            if definition.name.as_str() != "create_agent" {
                definitions.insert(definition.name.to_string(), definition);
            }
        }
    }
    let application = app.world().resource::<Application>().clone();
    for definition in definitions.into_values() {
        let options = if definition.name.as_str().starts_with("mcp_") {
            ToolOptions::default()
        } else {
            ToolOptions {
                footprint: Some(Touches::from_annotations(&definition.annotations)),
            }
        };
        app.world_mut().spawn_tool(
            definition.clone(),
            ErasedHandler::new(BusinessTool {
                application: application.clone(),
                definition,
                agent_id: None,
                call_id: None,
            }),
            options,
        );
    }
    app.finish();
    // Rig restores its journal before creating the default root.
    app.update();
    let roots: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, (With<Agent>, Without<SpawnedBy>)>()
        .iter(app.world())
        .collect();
    let root = *roots
        .first()
        .ok_or_else(|| anyhow::anyhow!("Rig did not create a root agent"))?;
    if app
        .world()
        .get::<AgentId>(root)
        .is_none_or(|id| id.0 != run.root_id)
    {
        let connection = runner
            .engine_for(Role::Root)
            .connection(runner.config().execution.stream);
        app.world_mut().entity_mut(root).insert((
            AgentId(run.root_id.clone()),
            connection.clone(),
            BusinessTask,
            ToolAccess::All,
        ));
        app.world_mut()
            .entity_mut(root)
            .insert(ModelChoice(connection.spec.reference()));
        app.world_mut()
            .trigger(Deliver::new(root, run.task.clone(), DeliveryMode::Steer));
    }
    runner.emit(
        run_id,
        None,
        "controller.started",
        json!({"owner":owner,"runtime":"rig-ecs","subagents":"rig-basics"}),
    )?;
    let started = Instant::now();
    let mut renewed = Instant::now();
    let mut included = HashSet::new();
    loop {
        if renewed.elapsed() >= Duration::from_secs(5) {
            ensure!(
                runner.store.acquire_lease(run_id, owner, 20)?,
                "controller lease lost"
            );
            renewed = Instant::now();
        }
        if cancel.is_cancelled() {
            runner
                .coordinator
                .pause_budget(run_id, "controller interrupted")?;
        }
        let run: RunRecord = runner.store.require("run", run_id)?;
        if !run.status.terminal()
            && run.status != Status::BudgetPaused
            && started.elapsed().as_secs() >= run.budget.max_wall_seconds
        {
            runner
                .coordinator
                .pause_budget(run_id, "wall-clock budget reached")?;
        }
        runner.coordinator.release_dependencies(run_id)?;
        runner.coordinator.wake_waiters(run_id)?;
        deliver_mail(&mut app, &runner, run_id, &mut included)?;
        sync_statuses(&mut app, &runner, run_id)?;
        app.update();
        if let Some(error) = app.world_mut().resource_mut::<Fatal>().0.take() {
            return Err(error);
        }
        if let Some(failure) = app
            .world()
            .resource::<rig_ecs::session_log::SessionLog>()
            .failure()
        {
            anyhow::bail!("Rig journal write failed: {failure}");
        }
        let run: RunRecord = runner.store.require("run", run_id)?;
        let active = app
            .world_mut()
            .query_filtered::<(), With<ActiveTurn>>()
            .iter(app.world())
            .count();
        if run.status == Status::BudgetPaused || cancel.is_cancelled() {
            // A subagent's initial brief is sent on the next harness frame.
            // Flush those messages before dropping the app. Gates hold model
            // and business tool IO while this run is paused.
            app.update();
            app.update();
            // Drop cancels in-flight futures. Leave Rig's saved turn open so
            // CarryOn can restore the exact pending effect on a later drive.
            runner.emit(
                run_id,
                None,
                "controller.finished",
                json!({"status":run.status}),
            )?;
            return runner.store.require("run", run_id);
        }
        if run.status.terminal() && active == 0 {
            app.world_mut().write_message(AppExit::Success);
            app.update();
            runner.emit(
                run_id,
                None,
                "controller.finished",
                json!({"status":run.status}),
            )?;
            return runner.store.require("run", run_id);
        }
        let agents = runner.store.list::<AgentRecord>("agent", run_id)?;
        if !run.status.terminal() && active == 0 && agents.iter().all(|a| a.status.terminal()) {
            let mut run = run;
            run.status = Status::Failed;
            run.summary = Some("All agents terminated without a successful finish_scan. Inspect errors and partial evidence.".into());
            run.updated_at = domain::now();
            runner.store.put("run", run_id, run_id, &run)?;
            continue;
        }
        // Complete all locally queued stages before waiting for async IO.
        if wakes.try_recv().is_err() {
            let _ = wakes.recv_timeout(Duration::from_secs(1));
        }
    }
}

fn task_schema() -> Value {
    let mut schema = crate::tools::child_schema();
    let properties = schema["properties"].as_object_mut().expect("typed object");
    let name = properties.remove("name").unwrap();
    let task = properties.remove("task").unwrap();
    properties.insert("description".into(), name);
    properties.insert("prompt".into(), task);
    schema["required"] = json!(["description", "prompt", "role", "scope"]);
    schema
}

fn allowed(runner: &AssessmentRunner, role: Role, name: &str) -> bool {
    matches!(name, "task" | "message" | "wait")
        || (name != "create_agent" && runner.tools.allows(role, name))
}

fn prepare_spawns(
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
            let agent: AgentRecord = application.runner.store.require("agent", &parent.0)?;
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

fn admit_spawns(
    requests: Query<(Entity, &SpawnAdmission), (With<SpawnSubagent>, ReadyToStart)>,
    application: Res<Application>,
    mut commands: Commands,
    mut fatal: ResMut<Fatal>,
) {
    for (entity, admission) in &requests {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let child: AgentRecord =
                    application.runner.store.require("agent", &admission.id)?;
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

fn configure_spawned(
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
                let child: AgentRecord =
                    application.runner.store.require("agent", &admission.id)?;
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
            .require::<AgentRecord>("agent", &admission.id)
        {
            let connection = application
                .runner
                .engine_for(child.role)
                .connection(application.runner.config().execution.stream);
            let choice = ModelChoice(connection.spec.reference());
            commands
                .entity(spawned.0)
                .insert((BusinessTask, ToolAccess::All, connection));
            commands
                .entity(spawned.0)
                .insert(choice)
                .remove::<ModelRefused>();
        }
    }
}

fn configure_agents(
    agents: Query<(Entity, &AgentId), (With<Agent>, Without<BusinessTask>)>,
    application: Res<Application>,
    mut commands: Commands,
) {
    for (entity, id) in &agents {
        if let Ok(Some(agent)) = application.runner.store.get::<AgentRecord>("agent", &id.0) {
            let connection = application
                .runner
                .engine_for(agent.role)
                .connection(application.runner.config().execution.stream);
            let choice = ModelChoice(connection.spec.reference());
            commands
                .entity(entity)
                .insert((BusinessTask, ToolAccess::All, connection));
            commands
                .entity(entity)
                .insert(choice)
                .remove::<ModelRefused>();
        }
    }
}

fn prepare_calls(
    mut calls: Query<(&ForAgent, &mut ToolRequest, &mut CallHandler), Added<Admitted>>,
    ids: Query<&AgentId>,
    defs: Query<&ToolDef>,
    application: Res<Application>,
) {
    for (from, mut request, mut handler) in &mut calls {
        let Ok(id) = ids.get(from.0) else {
            continue;
        };
        let name = request.call.function.name.as_str();
        let Some(def) = defs.iter().find(|def| def.0.name.as_str() == name) else {
            continue;
        };
        let mut args = Value::Object(request.call.function.arguments.clone());
        crate::tools::normalize_integer_arguments(&mut args, &def.0.parameters);
        request.call.function.arguments = args.as_object().unwrap().clone();
        handler.0 = rig_ecs::effects::Handler(ErasedHandler::new(BusinessTool {
            application: application.clone(),
            definition: (*def.0).clone(),
            agent_id: Some(id.0.clone()),
            call_id: application
                .runner
                .store
                .require::<AgentRecord>("agent", &id.0)
                .ok()
                .map(|agent| business_call(&agent, &request.call.id.to_string())),
        }));
    }
}

fn prepare_models(
    mut calls: Query<
        (Entity, &ForAgent, &mut ModelRequest),
        (
            With<Admitted>,
            Without<Started>,
            Without<Outcome>,
            Without<BusinessPrepared>,
        ),
    >,
    mut owners: Query<(&AgentId, &mut Conversation)>,
    application: Res<Application>,
    mut fatal: ResMut<Fatal>,
    mut commands: Commands,
) {
    for (effect, from, mut call) in &mut calls {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let (id, mut conversation) = owners.get_mut(from.0)?;
                let agent: AgentRecord = application.runner.store.require("agent", &id.0)?;
                if !agent.predecessors_released {
                    return Ok(());
                }
                let run: RunRecord = application.runner.store.require("run", &agent.run_id)?;
                call.history = Some(conversation.request_view());
                let config = application.runner.config();
                let mut preamble = agents::preamble(
                    &agent,
                    &run,
                    &application.runner.store,
                    config.skills_dir.as_deref(),
                    config.demos_dir.as_deref(),
                )?;
                for skill in &agent.active_skills {
                    ensure!(
                        crate::binary::sha256_bytes(skill.content.as_bytes()) == skill.sha256,
                        "active skill {} failed its content hash check",
                        skill.name
                    );
                    preamble.push_str(&format!(
                        "\n\n# Active controller skill: {} (SHA256 {})\n{}",
                        skill.name, skill.sha256, skill.content
                    ));
                }
                if let Some(prompt) = &mut call.prompt {
                    prompt.role = preamble;
                    if !agent.wait_for_predecessors.is_empty() {
                        let results = agent.wait_for_predecessors.iter().map(|id| application.runner.store.require::<AgentRecord>("agent",id).map(|predecessor| json!({"id":id,"status":predecessor.status,"result":predecessor.task_result,"summary":predecessor.summary,"error":predecessor.error}))).collect::<Result<Vec<_>>>()?;
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
                        .connection(application.runner.config().execution.stream),
                );
                let model_config = config.model.role_config_for(agent.role);
                let request = call.request_mut();
                request.max_tokens = Some(model_config.max_tokens);
                request.temperature = model_config.temperature;
                request.additional_params = model_config.additional_params;
                request
                    .tools
                    .retain(|def| allowed(&application.runner, agent.role, def.name.as_str()));
                // Rig's retention and summary splice preserve tool pairing, author
                // attribution and undo data. Full transcripts remain in its journal
                // and in integrity-checked evidence.
                let chars = serde_json::to_vec(conversation.messages())?.len();
                if chars > config.execution.compact_after_chars {
                    let retained = rig::memory::recent::view(
                        conversation.messages(),
                        &rig::memory::recent::LastMessages(config.execution.keep_recent_messages),
                    );
                    let start = conversation.messages().len() - retained.len();
                    if start > 0 && !retained.is_empty() {
                        let evidence = application.runner.store.artifact(
                            &run.id,
                            &agent.id,
                            &format!("rig-context:{effect:?}"),
                            "context.checkpoint",
                            &serde_json::to_vec(&conversation.0)?,
                            "application/json",
                            json!({"native_rig_conversation":true}),
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

fn gate_tools(
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
            let agent: AgentRecord = application.runner.store.require("agent", &id.0)?;
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
                    .list::<AgentRecord>("agent", &agent.run_id)?;
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

fn gate_models(
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
                let agent: AgentRecord = application.runner.store.require("agent", &id.0)?;
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

fn account_replies(
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
                    application
                        .runner
                        .account_usage(&ids.get(from.0)?.0, response.usage, true)
                })(),
            );
        }
    }
}

fn retry_policy(
    mut calls: Query<(&Outcome, &CallOf, &mut RetryVerdict), ReadyToSettle>,
    turns: Query<&Recovery>,
    application: Res<Application>,
) {
    let policy = rig_ecs::retry::RetryPolicy {
        max_retries: application
            .runner
            .config()
            .execution
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

fn enforce_completion(
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
                let agent: AgentRecord = application.runner.store.require("agent", &id.0)?;
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

fn record_turn_end(
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
                let record: AgentRecord = application.runner.store.require("agent", &id.0)?;
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

fn project_conversations(
    agents: Query<(&AgentId, &Conversation), Changed<Conversation>>,
    application: Res<Application>,
    mut fatal: ResMut<Fatal>,
) {
    for (id, conversation) in &agents {
        if application
            .runner
            .store
            .get::<AgentRecord>("agent", &id.0)
            .ok()
            .flatten()
            .is_some()
        {
            failed(
                &mut fatal,
                application.runner.store.checkpoint_and_ack(
                    &id.0,
                    serde_json::to_value(&conversation.0).unwrap(),
                    &[],
                ),
            );
        }
    }
}

fn deliver_mail(
    app: &mut App,
    runner: &AssessmentRunner,
    run_id: &str,
    included: &mut HashSet<i64>,
) -> Result<()> {
    let agents: HashMap<String, Entity> = app
        .world_mut()
        .query::<(Entity, &AgentId)>()
        .iter(app.world())
        .map(|(entity, id)| (id.0.clone(), entity))
        .collect();
    for agent in runner.store.list::<AgentRecord>("agent", run_id)? {
        let Some(&entity) = agents.get(&agent.id) else {
            continue;
        };
        // Completed work keeps queued mail until an explicit follow-up. The
        // coordinator then marks older-generation mail stale before delivery.
        if agent.status.terminal() {
            continue;
        }
        for mail in runner
            .store
            .inbox(&agent.id)?
            .into_iter()
            .filter(|mail| !included.contains(&mail.id))
            .collect::<Vec<_>>()
        {
            let followup = mail.envelope.options.kind == domain::MessageKind::FollowUp;
            let native_report = serde_json::from_str::<Value>(&mail.content)
                .ok()
                .is_some_and(|value| value["type"] == "agent_finished")
                && agents.contains_key(&mail.sender)
                && runner
                    .store
                    .require::<AgentRecord>("agent", &mail.sender)
                    .is_ok_and(|sender| Some(sender.generation) == mail.envelope.sender_generation);
            // The native Report carries this ledger notification. Do not send
            // a second message that can race its parent's next model call.
            if native_report {
                continue;
            }
            let informational = mail.envelope.options.kind == domain::MessageKind::Information;
            // Rig's wait observes messages attributed to the awaited agent,
            // including Inform messages. A typed information notice is host
            // data carrying its sender in the envelope, never a request/report.
            let author = if informational {
                Author::plugin("coordination-information")
            } else if mail.sender == "operator" {
                Author::user()
            } else {
                Author::agent(
                    AgentId(mail.sender.clone()),
                    followup.then(|| RequestId(format!("message-{}", mail.id))),
                )
            };
            if followup && let Some(&sender) = agents.get(&mail.sender) {
                let request = Request {
                    id: RequestId(format!("message-{}", mail.id)),
                    asker: AgentId(mail.sender.clone()),
                };
                let mut current = app
                    .world()
                    .get::<OpenRequests>(entity)
                    .cloned()
                    .unwrap_or_default();
                current.0.push(request);
                app.world_mut().entity_mut(entity).insert(current);
                let _ = sender;
            }
            let mode = if !informational
                && !agent.status.terminal()
                && (followup
                    || mail.sender == "operator"
                    || mail.envelope.options.kind == domain::MessageKind::Result)
            {
                DeliveryMode::Steer
            } else {
                DeliveryMode::Inform
            };
            app.world_mut().spawn((
                Incoming::new(serde_json::to_string(&mail)?, mode).with_author(author),
                ForAgent(entity),
                MailDelivery {
                    id: mail.id,
                    generation: agent.generation,
                },
            ));
            included.insert(mail.id);
        }
    }
    Ok(())
}

fn sync_statuses(app: &mut App, runner: &AssessmentRunner, run_id: &str) -> Result<()> {
    let records: HashMap<_, _> = runner
        .store
        .list::<AgentRecord>("agent", run_id)?
        .into_iter()
        .map(|agent| (agent.id.clone(), agent))
        .collect();
    let stopped: Vec<_> = app
        .world_mut()
        .query::<(Entity, &AgentId)>()
        .iter(app.world())
        .filter(|(_, id)| {
            records
                .get(&id.0)
                .is_some_and(|agent| matches!(agent.status, Status::Stopped | Status::Failed))
        })
        .map(|(entity, _)| entity)
        .collect();
    for entity in stopped {
        if app.world().get::<ActiveTurn>(entity).is_some() {
            app.world_mut().trigger(StopTurn::new(entity));
        }
    }
    Ok(())
}

#[derive(Clone)]
struct BusinessTool {
    application: Application,
    definition: ToolDefinition,
    agent_id: Option<String>,
    call_id: Option<String>,
}
impl Serve for BusinessTool {
    type Family = effect::family::Tool;
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            key: effect::tool_key(self.definition.name.as_str()),
            family: FamilyDescriptor::Tool {
                name: self.definition.name.to_string(),
                description: self.definition.description.clone(),
                parameters: self.definition.parameters.clone(),
                embedding: None,
            },
            layers: vec![],
        }
    }
    async fn serve(&self, kind: EffectKind, dispatch: Dispatch) -> Reply {
        let Some(agent_id) = self.agent_id.clone() else {
            return Reply::Outcome(Err(rig_core::error::ErrorReport::new(
                rig_core::error::ErrorKind::Internal,
                "tool has no business agent",
            )));
        };
        let EffectKind::ToolCall { name, args } = kind else {
            unreachable!("tool family")
        };
        let application = self.application.clone();
        let runtime = application.runtime.clone();
        let call_id = self
            .call_id
            .clone()
            .unwrap_or_else(|| format!("{}:rig:{}", agent_id, dispatch.id()));
        let mut task = runtime.spawn(async move {
            let mut output = application
                .runner
                .tools
                .execute(
                    &agent_id,
                    &call_id,
                    name.as_str(),
                    serde_json::from_str(&args)?,
                )
                .await?;
            if !output.is_error
                && !matches!(
                    name.as_str(),
                    "load_skill" | "context_search" | "context_read"
                )
                && application.runner.config().execution.summarize_tool_outputs
                && output.value.to_string().len()
                    > application
                        .runner
                        .config()
                        .execution
                        .summarize_threshold_chars
            {
                let agent: AgentRecord = application.runner.store.require("agent", &agent_id)?;
                let (value, usage) = application
                    .runner
                    .engine_for(agent.role)
                    .summarize_tool_output(
                        name.as_str(),
                        &output.value,
                        application.runner.config().execution.summarize_max_tokens,
                    )
                    .await;
                application.runner.account_usage(&agent_id, usage, false)?;
                output.value = value;
                application.runner.tools.replace_call_result(
                    &call_id,
                    &output.value,
                    output.is_error,
                )?;
            }
            // Conditional task waits add domain predicates. Their open tool
            // effect belongs to Rig, so its cancellation drops this future.
            let mut changes = application.runner.store.subscribe_changes();
            while application
                .runner
                .store
                .require::<AgentRecord>("agent", &agent_id)?
                .status
                == Status::Waiting
            {
                changes.changed().await?;
            }
            let value = ToolOutput::json(output.value);
            Ok::<_, anyhow::Error>(if output.is_error {
                ToolResult::failed(ToolExecutionError::other("tool failed")).with_output(value)
            } else {
                ToolResult::success(value)
            })
        });
        struct Abort(tokio::task::AbortHandle);
        impl Drop for Abort {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let _abort = Abort(task.abort_handle());
        let result = match (&mut task).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => ToolResult::failed(ToolExecutionError::other(format!("{error:#}"))),
            Err(error) => ToolResult::failed(ToolExecutionError::other(format!(
                "tool task stopped: {error}"
            ))),
        };
        Reply::Outcome(Ok(Answered::ToolResult { result }))
    }
}

fn carry_native_turns(
    turns: Query<Entity, (With<Restored>, With<TurnOf>)>,
    mut commands: Commands,
) {
    for turn in &turns {
        commands.entity(turn).insert(CarryOn);
    }
}

fn acknowledge_mail(
    deliveries: Query<(&MailDelivery, &ForAgent), Or<(With<DeliveredIn>, With<Settled>)>>,
    agents: Query<(&AgentId, &Conversation)>,
    application: Res<Application>,
    mut fatal: ResMut<Fatal>,
) {
    for (mail, from) in &deliveries {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let (id, conversation) = agents.get(from.0)?;
                application.runner.store.checkpoint_and_ack_generation(
                    &id.0,
                    serde_json::to_value(&conversation.0)?,
                    &[mail.id],
                    Some(mail.generation),
                )
            })(),
        );
    }
}

#[derive(Component)]
struct ModelSlot;
fn release_model_slots(
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

fn inform_terminal(
    mut incoming: Query<(&mut Incoming, &ForAgent), Added<Admitted>>,
    ids: Query<&AgentId>,
    application: Res<Application>,
) {
    for (mut message, from) in &mut incoming {
        if let Ok(id) = ids.get(from.0)
            && application
                .runner
                .store
                .require::<AgentRecord>("agent", &id.0)
                .is_ok_and(|agent| agent.status.terminal())
        {
            message.mode = DeliveryMode::Inform;
        }
    }
}

fn business_call(agent: &AgentRecord, wire_call: &str) -> String {
    format!(
        "{}:g{}:t{}:{}",
        agent.id, agent.generation, agent.turns, wire_call
    )
}

/// Re-enter only idempotent framework operations or the application ledger.
/// ToolService reconciles a running mutation by its receipt; an unknown MCP
/// mutation stays uncertain. Rig still supplies its settled results verbatim.
fn reconcile_calls(
    calls: Query<(Entity, &ToolRequest, &Interrupted), Added<Admitted>>,
    application: Res<Application>,
    mut commands: Commands,
) {
    for (entity, call, interrupted) in &calls {
        if !matches!(interrupted, Interrupted::Started) {
            continue;
        }
        let name = call.call.function.name.as_str();
        if name == "task"
            || name == "wait"
            || application.runner.tools.allows(Role::Root, name)
            || application.runner.tools.allows(Role::Verifier, name)
        {
            commands.entity(entity).remove::<Interrupted>();
        }
    }
}

fn bind_reports(
    mut reports: Query<(Entity, &ForAgent, &mut Incoming), (With<Report>, Added<Admitted>)>,
    ids: Query<&AgentId>,
    application: Res<Application>,
    mut commands: Commands,
    mut fatal: ResMut<Fatal>,
) {
    for (entity, from, mut report) in &mut reports {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let recipient = ids.get(from.0)?;
                let Some(sender) = report.author.from.as_deref() else {
                    return Ok(());
                };
                let owner: AgentRecord = application.runner.store.require("agent", &recipient.0)?;
                let source: AgentRecord = application.runner.store.require("agent", sender)?;
                if let Some(mail) = application
                    .runner
                    .store
                    .inbox(&recipient.0)?
                    .into_iter()
                    .find(|mail| {
                        mail.sender == sender
                            && mail.envelope.sender_generation == Some(source.generation)
                            && serde_json::from_str::<Value>(&mail.content)
                                .is_ok_and(|value| value["type"] == "agent_finished")
                    })
                {
                    *report.text_mut() = mail.content;
                    commands.entity(entity).insert(MailDelivery {
                        id: mail.id,
                        generation: owner.generation,
                    });
                }
                Ok(())
            })(),
        );
    }
}

fn stream_progress(
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

fn release_predecessors(
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
                .require::<AgentRecord>("agent", &id.0)
                .is_ok_and(|agent| agent.predecessors_released || agent.status.terminal())
        {
            commands.entity(hold).despawn();
        }
    }
}

/// A reactivated business task still owes its tree owner a result, even
/// when an ancestor assigned the follow-up directly to a deeper descendant.
fn ensure_owner_requests(
    mut agents: Query<(&AgentId, &mut OpenRequests), With<BusinessTask>>,
    application: Res<Application>,
    mut fatal: ResMut<Fatal>,
) {
    for (id, mut open) in &mut agents {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let task: AgentRecord = application.runner.store.require("agent", &id.0)?;
                if !task.status.terminal()
                    && let Some(owner) = task.parent_id
                {
                    if !open.0.iter().any(|request| request.asker.0 == owner) {
                        open.0.push(Request {
                            id: RequestId(format!("owner:{}:{}", task.id, task.generation)),
                            asker: AgentId(owner),
                        });
                    }
                }
                Ok(())
            })(),
        );
    }
}
