//! Rig owns execution, conversations, async subagents and effect cancellation.
//! This plugin supplies only application admission, evidence and completion gates.
use crate::{
    agents,
    domain::{self, AgentRecord, Role, RunRecord, Status},
    runner::AssessmentRunner,
    store::Store,
};
use anyhow::{Result, ensure};
use rig_basics::subagents::{
    OpenRequests, Report, Request, SpawnSubagent, SpawnedAgent, SubagentPolicy, SubagentsPlugin,
};
use rig_core::{
    completion::{AssistantContent, CompletionResponse, ToolDefinition, Usage},
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

mod admission;
use admission::*;
mod models;
use models::*;
mod coordination;
use coordination::*;
mod jobs;
use jobs::*;
mod business_tools;
use business_tools::*;

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
                definition.description = "Start an asynchronous subagent: role=analyst performs analysis, role=verifier independently checks another agent's claim. Supply a short description, full prompt and scope, with known evidence, constraints and outputs. Select capabilities through skills (recon, reverse, hunter, fuzz, crash, exploit, reporter or task methods), never through role. Changing capabilities alone does not justify another agent. Optional inherit_context, contract, wait_for_predecessors and dependency_policy refine the assignment. Creation returns before work finishes; predecessor gates hold the child's model requests. Model choice and tool permissions come from controller configuration and the role. Inspect existing work first. Use follow_up_agent for completed work, wait for one report or wait_for_agents for a group/success policy.".into();
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
    for role in [Role::Root, Role::Analyst, Role::Verifier] {
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
            tool_access(
                &runner,
                &runner.store.require_metadata("agent", &run.root_id)?,
            )?,
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
        // Push terminal job results into the store so the next model request
        // injects them as a section instead of the agent polling job_status.
        let handle = app.world().resource::<Application>().runtime.clone();
        handle.block_on(refresh_job_statuses(&runner, run_id))?;
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
        let agents = runner.store.list_metadata::<AgentRecord>("agent", run_id)?;
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

#[derive(Component)]
struct ModelSlot;
