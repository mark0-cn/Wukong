//! Business tools systems for the Rig adapter.
use super::*;

pub(super) fn prepare_calls(
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
                .require_metadata("agent", &id.0)
                .ok()
                .map(|agent| business_call(&agent, &request.call.id.to_string())),
        }));
    }
}

pub(super) fn business_call(agent: &AgentMetadata, wire_call: &str) -> String {
    crate::effects::DurableEffects::call_id(agent, wire_call)
}

/// Re-enter only idempotent framework operations or the application ledger.
/// ToolService reconciles a running mutation by its receipt; an unknown MCP
/// mutation stays uncertain. Rig still supplies its settled results verbatim.
pub(super) fn reconcile_calls(
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

#[derive(Clone)]
pub(super) struct BusinessTool {
    pub(super) application: Application,
    pub(super) definition: ToolDefinition,
    pub(super) agent_id: Option<String>,
    pub(super) call_id: Option<String>,
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
                let agent: AgentMetadata = application
                    .runner
                    .store
                    .require_metadata("agent", &agent_id)?;
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
                .require_metadata("agent", &agent_id)?
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
