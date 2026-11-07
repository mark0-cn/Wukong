//! Role-aware tool dispatch with durable effect receipts and original evidence.
use crate::{
    agents,
    binary::{self, GhidraBackend, GhidraConfig, GhidraQuery, StringsQuery, XrefDirection},
    config::AppConfig,
    coordinator::Coordinator,
    domain::*,
    mcp::McpRegistry,
    runtime::{RuntimeBackend, RuntimeClient},
    store::Store,
};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rig::{
    completion::{ToolAnnotations, ToolDefinition},
    message::ToolName,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub value: Value,
    pub is_error: bool,
}
#[derive(Clone)]
pub struct ToolService {
    pub store: Store,
    pub coordinator: Coordinator,
    pub config: AppConfig,
    runtime: Option<RuntimeClient>,
    mcp: McpRegistry,
    locks: Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
}

mod args;

struct Spec {
    parse: fn(Value) -> Result<Value>,
    definition: ToolDefinition,
    side_effecting: bool,
    target: bool,
    verifier_only: bool,
    root_only: bool,
    child_only: bool,
}
fn spec<A: serde::de::DeserializeOwned + serde::Serialize + schemars::JsonSchema>(
    name: &str,
    description: &str,
    side_effecting: bool,
    target: bool,
) -> Spec {
    Spec {
        definition: ToolDefinition::new(
            ToolName::new(name).expect("built-in tool name"),
            description,
            rig_core::tool::args_schema::<A>(),
        )
        .with_annotations(
            ToolAnnotations::default()
                .with_read_only(!side_effecting)
                .with_open_world(target),
        ),
        parse: |value| Ok(serde_json::to_value(serde_json::from_value::<A>(value)?)?),
        side_effecting,
        target,
        verifier_only: false,
        root_only: false,
        child_only: false,
    }
}

fn builtins() -> &'static [Spec] {
    static CATALOG: std::sync::LazyLock<Vec<Spec>> = std::sync::LazyLock::new(build_catalog);
    &CATALOG
}

fn build_catalog() -> Vec<Spec> {
    let mut specs = vec![
        spec::<args::CreateAgentArgs>(
            "create_agent",
            "Create an asynchronous focused child and return immediately; inherited context is optional. wait_for_predecessors gates admission with dependency_policy (default all_success). Failed requirements block execution and notify the parent.",
            true,
            false,
        ),
        spec::<args::ViewAgentGraphArgs>(
            "view_agent_graph",
            "List this assessment's agent tree, scopes, states and results.",
            false,
            false,
        ),
        spec::<args::SendMessageToAgentArgs>(
            "send_message_to_agent",
            "Queue information, a question, reply, result or blocked notice. Information does not interrupt waits. Only follow_up_agent reopens completed work. Queued/included is not proof of processing.",
            true,
            false,
        ),
        spec::<args::FollowUpAgentArgs>(
            "follow_up_agent",
            "Explicitly assign a follow-up; reopens completed work in a new generation while preserving history. Use recipient_generation to guard against stale assignments.",
            true,
            false,
        ),
        spec::<args::MessageStatusArgs>(
            "message_status",
            "Inspect a sent/received message, its delivery state and correlated reply.",
            false,
            false,
        ),
        spec::<args::WaitForAgentsArgs>(
            "wait_for_agents",
            "Park on selected agents with policy all_terminal (default), all_success or any_success; return a satisfied/blocked outcome or timeout. Empty list means children.",
            true,
            false,
        ),
        spec::<args::StopAgentArgs>(
            "stop_agent",
            "Stop yourself or your descendants, with a recorded reason.",
            true,
            false,
        ),
        spec::<args::AgentFinishArgs>(
            "agent_finish",
            "Finish a child task with a structured result accounting for its contract, evidence and gaps. summary is shorthand for tasks without required outputs. Active descendants must finish first.",
            true,
            false,
        ),
        spec::<args::FinishScanArgs>(
            "finish_scan",
            "Root only: finish the assessment after children and coverage are resolved.",
            true,
            false,
        ),
        spec::<args::ThreatModelGetArgs>(
            "threat_model_get",
            "Read the shared threat model documents.",
            false,
            false,
        ),
        spec::<args::ThreatModelSaveArgs>(
            "threat_model_save",
            "Create a shared threat model with a stable document ID.",
            true,
            false,
        ),
        spec::<args::ThreatModelAmendArgs>(
            "threat_model_amend",
            "Append a durable amendment to a shared threat model.",
            true,
            false,
        ),
        spec::<args::NoteSaveArgs>(
            "note_save",
            "Save a shared note linked to this task.",
            true,
            false,
        ),
        spec::<args::NotesListArgs>(
            "notes_list",
            "List shared notes for this assessment.",
            false,
            false,
        ),
        spec::<args::ContextSearchArgs>(
            "context_search",
            "Search same-run shared notes, todos and optional threat models by text, kind, author scope or agent_id. Returns bounded previews and next_offset; use context_read for exact content including amendments.",
            false,
            false,
        ),
        spec::<args::ContextReadArgs>(
            "context_read",
            "Read a bounded character range of an exact same-run shared document as JSON, including amendments. Use kind and id from context_search; follow content.next_offset until the required content is complete.",
            false,
            false,
        ),
        spec::<args::TodoSaveArgs>(
            "todo_save",
            "Create a shared to-do item; status defaults to pending.",
            true,
            false,
        ),
        spec::<args::TodoUpdateArgs>(
            "todo_update",
            "Update the body or status of an existing shared to-do.",
            true,
            false,
        ),
        spec::<args::TodosListArgs>("todos_list", "List shared to-do items.", false, false),
        spec::<args::CoverageRecordArgs>(
            "coverage_record",
            "Record inspected scope, outcome and supporting evidence. needs_follow_up blocks root completion.",
            true,
            false,
        ),
        spec::<args::CoverageUpdateArgs>(
            "coverage_update",
            "Update a coverage record's outcome, notes and evidence.",
            true,
            false,
        ),
        spec::<args::CoverageListArgs>(
            "coverage_list",
            "List all recorded assessment coverage.",
            false,
            false,
        ),
        spec::<args::FindingFileArgs>(
            "finding_file",
            "File a suspected or reproduced finding with evidence. Verified impact requires a separate immutable verifier record. severity critical/high requires a reproduced-or-better stage.",
            true,
            false,
        ),
        spec::<args::FindingUpdateArgs>(
            "finding_update",
            "Update an existing finding. Impact promotion is enforced using independent execution evidence.",
            true,
            false,
        ),
        spec::<args::FindingsListArgs>(
            "findings_list",
            "List this assessment's findings and recorded stages.",
            false,
            false,
        ),
        spec::<args::FindingGetArgs>(
            "finding_get",
            "Read a finding and its independent verification record.",
            false,
            false,
        ),
        spec::<args::VerificationRecordArgs>(
            "verification_record",
            "Verifier only: ingest a completed poc_verify job, its raw evidence and real trial counts. A crash verifies only crash behavior.",
            true,
            true,
        ),
        spec::<args::EvidenceReadArgs>(
            "evidence_read",
            "Read a bounded byte range from an integrity-checked evidence artifact. Non-UTF8 is base64.",
            false,
            false,
        ),
        spec::<args::LoadSkillArgs>(
            "load_skill",
            "Activate a named local analysis skill (built-in or configured custom skill). The controller persists its exact content/hash and installs its instructions at the next fresh model boundary; survives compaction and resume.",
            true,
            false,
        ),
        spec::<args::ThinkArgs>(
            "think",
            "Record a concise audited scratch note for this agent. The original content remains in tool-call history and may be inherited by children; this is neither a private channel nor a context-saving mechanism. Use note_save for shared conclusions.",
            true,
            false,
        ),
        spec::<args::BinaryInspectArgs>(
            "binary_inspect",
            "Inspect the exact run target, architecture, sections, imports, symbols and mitigations.",
            false,
            true,
        ),
        spec::<args::BinaryStringsArgs>(
            "binary_strings",
            "Search printable strings with file-offset address references and pagination.",
            false,
            true,
        ),
        spec::<args::FunctionsSearchArgs>(
            "functions_search",
            "Ghidra: search analyzed functions, paginated.",
            false,
            true,
        ),
        spec::<args::FunctionDecompileArgs>(
            "function_decompile",
            "Ghidra: decompile a function by MODULE-RELATIVE offset.",
            false,
            true,
        ),
        spec::<args::XrefsQueryArgs>(
            "xrefs_query",
            "Ghidra: references to/from a MODULE-RELATIVE offset.",
            false,
            true,
        ),
        spec::<args::RuntimeDoctorArgs>(
            "runtime_doctor",
            "Inspect the configured worker's real platform, versions and available capabilities.",
            false,
            true,
        ),
        spec::<args::StageTargetArgs>(
            "stage_target",
            "Copy the run's SHA256-verified original target into the worker's immutable files area.",
            true,
            true,
        ),
        spec::<args::FilePutArgs>(
            "file_put",
            "Create an immutable worker file from base64 bytes or an existing evidence artifact.",
            true,
            true,
        ),
        spec::<args::FileGetArgs>(
            "file_get",
            "Retrieve a declared in-run worker file and persist original bytes as evidence.",
            false,
            true,
        ),
        spec::<args::JobStartArgs>(
            "job_start",
            "Start a persistent process job in the configured worker; returns a job_id immediately. argv entries may use {target} as a placeholder for the staged target path (resolved by the harness and staged in isolated jobs); worker paths like files/... resolve only as direct argv entries, never inside shell strings. Jobs may read RE_AGENT_STATE_DIR and RE_AGENT_JOB_DIR from the environment.",
            true,
            true,
        ),
        spec::<args::JobStatusArgs>(
            "job_status",
            "Observe an in-run job, refresh persisted status and import raw observations.",
            false,
            true,
        ),
        spec::<args::JobCancelArgs>(
            "job_cancel",
            "Request cancellation of an owned in-run job.",
            true,
            true,
        ),
        spec::<args::JobArtifactsArgs>(
            "job_artifacts",
            "List and import original artifacts produced by an in-run job.",
            false,
            true,
        ),
        spec::<args::FuzzStartArgs>(
            "fuzz_start",
            "Start AFL++ QEMU fuzzing after staging target and nonempty seed files.",
            true,
            true,
        ),
        spec::<args::FuzzStatusArgs>(
            "fuzz_status",
            "Observe a bound AFL++ job and its real statistics/crash files.",
            false,
            true,
        ),
        spec::<args::DebugStartArgs>(
            "debug_start",
            "Start an owned GDB/MI session on the staged target.",
            true,
            true,
        ),
        spec::<args::DebugCommandArgs>(
            "debug_command",
            "Execute exactly one GDB/MI command in your owned session; stable request IDs prevent blind replay.",
            true,
            true,
        ),
        spec::<args::DebugCloseArgs>(
            "debug_close",
            "Close your debugger session, or clean up an in-run session whose owner is terminal.",
            true,
            true,
        ),
        spec::<args::InteractiveStartArgs>(
            "interactive_start",
            "Start a persistent interactive process (REPL-style program) owned by this agent; returns a session_id. Worker paths resolve as direct argv entries.",
            true,
            true,
        ),
        spec::<args::InteractiveSendArgs>(
            "interactive_send",
            "Write bytes to an owned interactive session's stdin and return its newly produced output. The session stays alive between calls.",
            true,
            true,
        ),
        spec::<args::InteractiveCloseArgs>(
            "interactive_close",
            "Close your interactive session and terminate its process tree; also permits cleanup when its in-run owner is terminal.",
            true,
            true,
        ),
        spec::<args::AngrSolveArgs>(
            "angr_solve",
            "Symbolic execution: solve for a concrete stdin input that reaches a module-relative find address while avoiding others. Runs as a background job with a hard timeout; results (input_base64 + exploration stats) arrive via job_status.",
            true,
            true,
        ),
        spec::<args::CrashReplayArgs>(
            "crash_replay",
            "Start a persistent clean-directory crash reproduction job.",
            true,
            true,
        ),
        spec::<args::InputMinimizeArgs>(
            "input_minimize",
            "Start crash-preserving delta reduction using observed crash signatures.",
            true,
            true,
        ),
        spec::<args::PocVerifyArgs>(
            "poc_verify",
            "Verifier only: independently replay a finding's input in fresh directories. Worker observations determine trial success.",
            true,
            true,
        ),
    ];
    for item in &mut specs {
        match item.definition.name.as_str() {
            "finish_scan" => item.root_only = true,
            "agent_finish" => item.child_only = true,
            "poc_verify" | "verification_record" => item.verifier_only = true,
            _ => {}
        }
    }
    specs
}
pub(crate) fn child_schema() -> Value {
    rig_core::tool::args_schema::<args::CreateAgentArgs>()
}

impl ToolService {
    pub async fn new(config: AppConfig, store: Store) -> Result<Self> {
        config.validate()?;
        let runtime = if config.runtime.backend == RuntimeBackend::Disabled {
            None
        } else {
            Some(RuntimeClient::connect(config.runtime.clone()).await?)
        };
        let mcp = McpRegistry::connect(&config.mcp).await?;
        Ok(Self {
            coordinator: Coordinator::new(store.clone()),
            store,
            config,
            runtime,
            mcp,
            locks: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }
    pub(crate) fn allows(&self, role: Role, name: &str) -> bool {
        builtins()
            .iter()
            .find(|item| item.definition.name.as_str() == name)
            .map_or_else(
                || role != Role::Root && self.mcp.contains(name),
                |item| {
                    (!item.target || role != Role::Root)
                        && (!item.verifier_only || role == Role::Verifier)
                        && (!item.root_only || role == Role::Root)
                        && (!item.child_only || role != Role::Root)
                },
            )
    }

    pub fn definitions(&self, role: Role) -> Vec<ToolDefinition> {
        let mut definitions = builtins()
            .iter()
            .filter(|item| {
                (!item.target || role != Role::Root)
                    && (!item.verifier_only || role == Role::Verifier)
                    && (!item.root_only || role == Role::Root)
                    && (!item.child_only || role != Role::Root)
            })
            .map(|item| item.definition.clone())
            .collect::<Vec<_>>();
        if role != Role::Root {
            definitions.extend(self.mcp.definitions());
        }
        definitions
    }
    pub async fn close(&self) {
        self.mcp.close().await;
        if let Some(runtime) = &self.runtime {
            let _ = runtime.close().await;
        }
    }
    pub async fn doctor(&self) -> Result<Value> {
        if let Some(runtime) = &self.runtime {
            runtime.doctor().await
        } else {
            Ok(
                json!({"enabled":false,"capabilities":{},"reason":"runtime disabled in configuration"}),
            )
        }
    }

    /// Substitute a completed call's recorded result (e.g. an LM-summarized
    /// envelope). Evidence and the original bytes are untouched; every later
    /// durable replay returns the substituted result.
    pub fn replace_call_result(&self, call_id: &str, value: &Value, is_error: bool) -> Result<()> {
        let mut record: ToolCallRecord = self.store.require("call", call_id)?;
        ensure!(
            matches!(record.state, CallState::Completed | CallState::Failed),
            "only a completed call result can be replaced"
        );
        record.result = Some(json!({"value": value, "is_error": is_error}));
        record.updated_at = now();
        self.store.put("call", call_id, &record.run_id, &record)
    }

    pub async fn execute(
        &self,
        agent_id: &str,
        call_id: &str,
        name: &str,
        args: Value,
    ) -> Result<ToolOutcome> {
        ensure!(
            !call_id.is_empty() && call_id.len() <= 512,
            "invalid tool call ID"
        );
        let lock = {
            let mut locks = self.locks.lock().await;
            // Sweep entries nobody references anymore so a long-running
            // controller does not accumulate one mutex per historical call
            // id. Holders and waiters keep their Arc alive, so an entry that
            // is in flight (or queued behind it) is never swept.
            if locks.len() >= 256 {
                locks.retain(|_, held| Arc::strong_count(held) > 1);
            }
            locks
                .entry(call_id.into())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        let agent: AgentRecord = self.store.require("agent", agent_id)?;
        let specs = builtins();
        let builtin = specs.iter().find(|item| item.definition.name == name);
        let side_effecting = builtin
            .map(|item| item.side_effecting)
            .unwrap_or_else(|| self.mcp.side_effecting(name));
        let existing: Option<ToolCallRecord> = self.store.get("call", call_id)?;
        if let Some(existing) = &existing {
            ensure!(
                existing.agent_id == agent_id
                    && existing.run_id == agent.run_id
                    && existing.name == name
                    && existing.arguments == args,
                "tool call ID reused with a different request"
            );
            if matches!(existing.state, CallState::Completed | CallState::Failed) {
                return serde_json::from_value(
                    existing
                        .result
                        .clone()
                        .context("completed call result missing")?,
                )
                .context("decode recorded tool result");
            }
            // A builtin effect may have committed inside its transactional
            // receipt while its response envelope was lost. The durable retry
            // must surface the recorded effect, not re-run the transition
            // against an agent that is already terminal.
            if let Some(receipt) = self.store.get::<Value>("tool_receipt", call_id)? {
                return self.finish_raw(existing.clone(), receipt, false, CallState::Completed);
            }
            if self.mcp.contains(name) && side_effecting {
                if let Some(result) = &existing.result {
                    return serde_json::from_value(result.clone())
                        .context("decode uncertain tool result");
                }
                return self.finish_raw(existing.clone(),json!({"error":"prior MCP side effect outcome is unknown; operator reconciliation is required before any retry","call_id":call_id}),true,CallState::Uncertain);
            }
        }
        let mut record = existing.unwrap_or(ToolCallRecord {
            id: call_id.into(),
            run_id: agent.run_id.clone(),
            agent_id: agent_id.into(),
            name: name.into(),
            arguments: args.clone(),
            state: CallState::Prepared,
            side_effecting,
            result: None,
            evidence_id: None,
            created_at: now(),
            updated_at: now(),
        });
        record.state = CallState::Running;
        record.updated_at = now();
        self.store.put("call", call_id, &agent.run_id, &record)?;
        self.store.event(
            &agent.run_id,
            Some(agent_id),
            "tool.started",
            json!({"call_id":call_id,"name":name}),
        )?;
        // wait_for_agents parks the agent and returns immediately today, but
        // its schema allows waits up to 3600s. Size the protective outer
        // timeout to the requested wait so a future blocking implementation
        // cannot truncate a legitimate long wait into an uncertain outcome.
        let outer_timeout = if name == "wait_for_agents" {
            Duration::from_secs(
                number(&args, "timeout_seconds", 300)
                    .clamp(1, 3600)
                    .saturating_add(10),
            )
        } else {
            Duration::from_secs(self.config.tools.timeout_seconds)
        };
        let result = async {
            ensure!(args.is_object(), "tool arguments must be a JSON object");
            ensure!(
                !agent.status.terminal(),
                "a terminal agent cannot start a new tool effect"
            );
            let run: RunRecord = self.store.require("run", &agent.run_id)?;
            ensure!(
                !run.status.terminal(),
                "a terminal assessment cannot start a new tool effect"
            );
            ensure!(
                self.allows(agent.role, name),
                "tool {name} is unavailable for role {}",
                agent.role
            );
            let args = if let Some(spec) = builtin {
                (spec.parse)(validated_arguments(args, &spec.definition.parameters)?)?
            } else {
                args
            };
            self.dispatch(&agent, call_id, name, args).await
        };
        let result = tokio::time::timeout(outer_timeout, result).await;
        match result {
            Ok(Ok(value))=>{let is_error=self.mcp.contains(name)&&value["isError"].as_bool()==Some(true);self.finish_raw(record,value,is_error,if is_error{CallState::Failed}else{CallState::Completed})},
            Ok(Err(error))=>{
                let worker_unknown=side_effecting&&error.chain().any(|cause|cause.to_string().starts_with("worker dispatch"));
                let state=if (self.mcp.contains(name)&&side_effecting)||worker_unknown{CallState::Uncertain}else{CallState::Failed};
                self.finish_raw(record,json!({"error":format!("{error:#}")}),true,state)
            },
            Err(_)=>self.finish_raw(record,json!({"error":"tool timed out; reconcile the durable request before retrying","call_id":call_id}),true,if side_effecting{CallState::Uncertain}else{CallState::Failed}),
        }
    }

    fn finish_raw(
        &self,
        record: ToolCallRecord,
        raw: Value,
        is_error: bool,
        state: CallState,
    ) -> Result<ToolOutcome> {
        finish_record(
            &self.store,
            self.config.tools.max_output_bytes,
            record,
            raw,
            is_error,
            state,
        )
    }

    /// Operator API only; deliberately absent from model tool definitions.
    /// Preserve supporting evidence and an explicit resolution instead of
    /// repeating an external action whose outcome was lost.
    pub fn reconcile_call(
        &self,
        call_id: &str,
        resolution: Value,
        evidence_ids: &[String],
        is_error: bool,
    ) -> Result<ToolOutcome> {
        reconcile_record(&self.store, call_id, resolution, evidence_ids, is_error)
    }

    fn effect(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        action: impl FnOnce(&rusqlite::Connection) -> Result<Value>,
    ) -> Result<Value> {
        self.store.transaction(|tx| {
            if let Some(value) = Store::tx_get::<Value>(tx, "tool_receipt", call_id)? {
                return Ok(value);
            }
            let value = action(tx)?;
            Store::tx_put(tx, "tool_receipt", call_id, &agent.run_id, &value)?;
            Ok(value)
        })
    }

    async fn dispatch(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        args: Value,
    ) -> Result<Value> {
        match name {
            "create_agent" => self.create_agent(agent, call_id, &args),
            "view_agent_graph" => {
                let mut sessions = Vec::new();
                for kind in ["debug_session", "interact_session"] {
                    sessions.extend(self.store.list::<Value>(kind, &agent.run_id)?.iter()
                        .filter(|session| !matches!(session["state"].as_str(), Some("closed" | "lost")))
                        .map(|session| json!({"kind":kind,"id":session["id"],"agent_id":session["agent_id"],"state":session["state"]})));
                }
                Ok(
                    json!({"agents":self.store.list::<AgentRecord>("agent",&agent.run_id)?.iter().map(|a|json!({"id":a.id,"parent_id":a.parent_id,"name":a.name,"role":a.role,"task":a.task,"scope":a.scope,"contract":a.contract,"result":a.task_result,"status":a.status,"generation":a.generation,"dependency_policy":a.dependency_policy,"predecessors":a.wait_for_predecessors,"predecessors_released":a.predecessors_released,"wait_policy":a.wait_policy,"wait_for":a.wait_for,"summary":a.summary,"error":a.error,"turns":a.turns,"total_tokens":a.total_tokens})).collect::<Vec<_>>(),"open_sessions":sessions}),
                )
            }
            "send_message_to_agent" | "follow_up_agent" => {
                self.send_message(agent, call_id, &args, name == "follow_up_agent")
            }
            "message_status" => {
                let mid = number(&args, "message_id", 0) as i64;
                let message = self
                    .store
                    .transaction(|tx| {
                        Ok(Store::tx_messages(tx, &agent.run_id)?
                            .into_iter()
                            .find(|message| message.id == mid))
                    })?
                    .context("message not in this run")?;
                ensure!(
                    agent.role == Role::Root
                        || message.sender == agent.id
                        || message.recipient == agent.id,
                    "message is not addressed to or sent by this agent"
                );
                Ok(serde_json::to_value(message)?)
            }
            "wait_for_agents" => {
                let policy = serde_json::from_value(
                    args.get("policy").cloned().unwrap_or(json!("all_terminal")),
                )?;
                self.effect(agent, call_id, |tx| {
                    Coordinator::tx_wait(
                        tx,
                        &agent.id,
                        &string_list(&args, "agent_ids")?,
                        number(&args, "timeout_seconds", 300),
                        policy,
                    )
                })
            }
            "stop_agent" => {
                let target_id = {
                    let target: AgentRecord =
                        self.require_in_run("agent", text(&args, "agent_id")?, &agent.run_id)?;
                    self.authorize_control(agent, &target)?;
                    target.id.clone()
                };
                self.effect(agent, call_id, |tx| {
                    Ok(json!({"stopped":Coordinator::tx_stop(tx, &target_id, text(&args, "reason")?)?}))
                })
            }
            "agent_finish" | "finish_scan" => {
                let result: TaskResult = if let Some(result) = args.get("result") {
                    let result: TaskResult = serde_json::from_value(result.clone())?;
                    if let Some(summary) = args.get("summary").and_then(Value::as_str) {
                        ensure!(summary == result.answer, "summary must match result.answer");
                    }
                    result
                } else {
                    TaskResult {
                        answer: text(&args, "summary")?.into(),
                        ..Default::default()
                    }
                };
                self.store
                    .verify_evidence_links(&agent.run_id, &result.evidence_links())?;
                self.effect(agent, call_id, |tx| {
                    Coordinator::tx_finish_result(tx, &agent.id, result)?;
                    Ok(json!({"finished":true,"agent_id":agent.id}))
                })
            }
            "threat_model_get" => {
                if let Some(id) = args["id"].as_str() {
                    Ok(serde_json::to_value(
                        self.require_in_run::<SharedDocument>("threat_model", id, &agent.run_id)?,
                    )?)
                } else {
                    Ok(
                        json!({"documents":self.store.list::<SharedDocument>("threat_model",&agent.run_id)?}),
                    )
                }
            }
            "threat_model_save" | "note_save" | "todo_save" => {
                self.save_document(agent, call_id, name, &args)
            }
            "threat_model_amend" | "todo_update" => {
                self.amend_document(agent, call_id, name, &args)
            }
            "notes_list" => {
                Ok(json!({"notes":self.store.list::<SharedDocument>("note",&agent.run_id)?}))
            }
            "context_search" => crate::context::search(
                &self.store,
                agent,
                &args,
                self.config.tools.max_output_bytes,
            ),
            "context_read" => crate::context::read(
                &self.store,
                agent,
                &args,
                self.config.tools.max_output_bytes,
            ),
            "todos_list" => {
                Ok(json!({"todos":self.store.list::<SharedDocument>("todo",&agent.run_id)?}))
            }
            "coverage_record" | "coverage_update" => self.coverage(agent, call_id, name, &args),
            "coverage_list" => {
                Ok(json!({"coverage":self.store.list::<Coverage>("coverage",&agent.run_id)?}))
            }
            "finding_file" | "finding_update" => self.finding(agent, call_id, name, &args),
            "findings_list" => {
                Ok(json!({"findings":self.store.list::<Finding>("finding",&agent.run_id)?}))
            }
            "finding_get" => {
                let finding: Finding =
                    self.require_in_run("finding", text(&args, "id")?, &agent.run_id)?;
                let verification = finding
                    .verification_id
                    .as_ref()
                    .map(|id| {
                        self.require_in_run::<Verification>("verification", id, &agent.run_id)
                    })
                    .transpose()?;
                Ok(json!({"finding":finding,"verification":verification}))
            }
            "verification_record" => self.verification(agent, call_id, &args).await,
            "evidence_read" => {
                let evidence_id = text(&args, "evidence_id")?.to_owned();
                let record: Evidence =
                    self.require_in_run("evidence", &evidence_id, &agent.run_id)?;
                let total = usize::try_from(record.size)?;
                let offset = usize::try_from(number(&args, "offset", 0))?;
                ensure!(offset <= total, "evidence offset is past end of file");
                let length = usize::try_from(number(&args, "length", 8192))?
                    .min(self.config.tools.max_evidence_read_bytes);
                let part = if offset == total || length == 0 {
                    Vec::new()
                } else {
                    self.store.read_evidence_range(
                        &agent.run_id,
                        &evidence_id,
                        offset as u64,
                        length,
                    )?
                };
                let end = offset + part.len();
                let content = match std::str::from_utf8(&part) {
                    Ok(text) => json!({"encoding":"utf8","data":text}),
                    Err(_) => json!({"encoding":"base64","data":BASE64.encode(&part)}),
                };
                Ok(
                    json!({"evidence_id":evidence_id,"offset":offset,"length":part.len(),"total_bytes":total,"next_offset":if end<total{Some(end)}else{None},"content":content}),
                )
            }
            "load_skill" => self.activate_skill(agent, call_id, text(&args, "name")?),
            "think" => {
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
                let thoughts = self
                    .store
                    .list::<crate::domain::Thought>("thought", &agent.run_id)?
                    .into_iter()
                    .filter(|t| t.agent_id == agent.id)
                    .count();
                self.store
                    .put("thought", &thought.id, &agent.run_id, &thought)?;
                Ok(json!({"recorded":true,"chars":content.chars().count(),"thoughts":thoughts + 1}))
            }
            "binary_inspect" => {
                let run = self.target(agent)?;
                let full = serde_json::to_value(binary::inspect(&run.target_path)?)?;
                let bytes = serde_json::to_vec(&full)?;
                let evidence = self.store.artifact(
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
            }
            "binary_strings" => {
                let run = self.target(agent)?;
                let query: StringsQuery = serde_json::from_value(args)?;
                Ok(serde_json::to_value(binary::strings(
                    &run.target_path,
                    &query,
                )?)?)
            }
            "functions_search" | "function_decompile" | "xrefs_query" => {
                self.ghidra(agent, call_id, name, &args).await
            }
            "runtime_doctor" => self.doctor().await,
            "stage_target" => self.stage_target(agent, call_id).await,
            "file_put" => self.file_put(agent, call_id, &args).await,
            "file_get" => self.file_get(agent, call_id, &args).await,
            "job_start" | "job_status" | "job_cancel" | "job_artifacts" | "fuzz_start"
            | "fuzz_status" | "debug_start" | "debug_command" | "debug_close" | "crash_replay"
            | "input_minimize" | "poc_verify" | "angr_solve" | "interactive_start"
            | "interactive_send" | "interactive_close" => {
                self.runtime_tool(agent, call_id, name, args).await
            }
            _ if self.mcp.contains(name) => self.mcp.call(name, args).await,
            _ => bail!("unknown tool {name}"),
        }
    }

    fn require_in_run<T: serde::de::DeserializeOwned + Serialize>(
        &self,
        kind: &str,
        id: &str,
        run_id: &str,
    ) -> Result<T> {
        let record: T = self.store.require(kind, id)?;
        let value = serde_json::to_value(&record)?;
        ensure!(
            value["run_id"].as_str() == Some(run_id),
            "{kind} belongs to another run"
        );
        Ok(record)
    }
    fn target(&self, agent: &AgentRecord) -> Result<RunRecord> {
        ensure!(
            agent.role != Role::Root,
            "root cannot operate on the target"
        );
        let run: RunRecord = self.store.require("run", &agent.run_id)?;
        ensure!(
            binary::sha256_file(&run.target_path)? == run.target_sha256,
            "target SHA256 changed since run creation"
        );
        Ok(run)
    }
    fn runtime(&self) -> Result<&RuntimeClient> {
        self.runtime.as_ref().context(
            "execution runtime is disabled; configure Docker/SSH or explicitly trusted_local",
        )
    }
    fn authorize_control(&self, actor: &AgentRecord, target: &AgentRecord) -> Result<()> {
        ensure!(actor.run_id == target.run_id, "cross-run control denied");
        if actor.role == Role::Root || actor.id == target.id {
            return Ok(());
        }
        let all = self.store.list::<AgentRecord>("agent", &actor.run_id)?;
        let mut parent = target.parent_id.as_deref();
        let mut count = 0;
        while let Some(id) = parent {
            if id == actor.id {
                return Ok(());
            }
            parent = all
                .iter()
                .find(|a| a.id == id)
                .and_then(|a| a.parent_id.as_deref());
            count += 1;
            if count > all.len() {
                break;
            }
        }
        bail!("only your own descendants may be controlled")
    }

    pub(crate) fn create_agent(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        args: &Value,
    ) -> Result<Value> {
        let request: crate::coordinator::ChildRequest = serde_json::from_value(args.clone())?;
        ensure!(
            request.role != Role::Root && request.skills.len() <= 5,
            "invalid child role or skill count"
        );
        self.store
            .verify_evidence_links(&agent.run_id, &request.contract.input_evidence_ids)?;
        for skill in &request.skills {
            ensure!(
                skill != "root",
                "root orchestration skill is only available to the root role"
            );
            agents::load_skill(skill, self.config.skills_dir.as_deref())?;
        }
        self.effect(agent, call_id, |tx| {
            Ok(serde_json::to_value(Coordinator::tx_create_child(
                tx,
                &agent.id,
                request,
                stable("agent", call_id),
            )?)?)
        })
    }
    fn activate_skill(&self, agent: &AgentRecord, call_id: &str, name: &str) -> Result<Value> {
        ensure!(
            name != "root" || agent.role == Role::Root,
            "root orchestration skill is only available to the root role"
        );
        let content = agents::load_skill(name, self.config.skills_dir.as_deref())?;
        let sha256 = binary::sha256_bytes(content.as_bytes());
        let evidence = self.store.artifact(
            &agent.run_id,
            &agent.id,
            call_id,
            "skill.instructions",
            content.as_bytes(),
            "text/markdown",
            json!({"name":name,"sha256":sha256}),
        )?;
        self.effect(agent,call_id,|tx| {
            let mut current: AgentRecord = Store::tx_get(tx,"agent",&agent.id)?.context("agent missing")?;
            ensure!(current.generation == agent.generation, "agent activation changed before skill load");
            let snapshot = ActiveSkill { name:name.into(),sha256:sha256.clone(),content,evidence_id:evidence.id.clone() };
            if let Some(existing) = current.active_skills.iter_mut().find(|skill|skill.name==name) {
                *existing=snapshot;
            } else { current.active_skills.push(snapshot); }
            current.updated_at=now();
            Store::tx_put(tx,"agent",&agent.id,&agent.run_id,&current)?;
            Store::tx_event(tx,&agent.run_id,Some(&agent.id),"skill.activated",json!({"name":name,"sha256":sha256,"evidence_id":evidence.id}))?;
            Ok(json!({"name":name,"sha256":sha256,"evidence_id":evidence.id,"active":true,"applies":"next fresh model request"}))
        })
    }
    fn send_message(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        args: &Value,
        follow_up: bool,
    ) -> Result<Value> {
        let mut options: MessageOptions = serde_json::from_value(json!({
            "kind":args.get("kind").cloned().unwrap_or(json!("information")),
            "correlation_id":args.get("correlation_id"),"reply_to":args.get("reply_to"),
            "recipient_generation":args.get("recipient_generation"),
            "evidence_ids":args.get("evidence_ids").cloned().unwrap_or(json!([]))
        }))?;
        if follow_up {
            options.kind = MessageKind::FollowUp;
        }
        self.effect(agent, call_id, |tx| {
            Ok(serde_json::to_value(Coordinator::tx_send(
                tx,
                &agent.id,
                text(args, "agent_id")?,
                text(args, "message")?,
                options,
                Some(call_id),
            )?)?)
        })
    }
    fn save_document(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let kind = match name {
            "threat_model_save" => "threat_model",
            "note_save" => "note",
            _ => "todo",
        };
        self.effect(agent, call_id, |tx| {
            let document = SharedDocument {
                id: stable("doc", call_id),
                run_id: agent.run_id.clone(),
                agent_id: agent.id.clone(),
                kind: kind.into(),
                title: text(args, "title")?.into(),
                body: args["body"].as_str().unwrap_or_default().into(),
                amendments: if kind == "todo" {
                    vec![json!({"status":"pending","agent_id":agent.id,"created_at":now()})]
                } else {
                    vec![]
                },
                created_at: now(),
                updated_at: now(),
            };
            Store::tx_put(tx, kind, &document.id, &agent.run_id, &document)?;
            Ok(serde_json::to_value(document)?)
        })
    }
    fn amend_document(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let kind = if name == "threat_model_amend" {
            "threat_model"
        } else {
            "todo"
        };
        self.effect(agent, call_id, |tx| {
            let mut document: SharedDocument =
                Store::tx_get(tx, kind, text(args, "id")?)?.context("document missing")?;
            ensure!(
                document.run_id == agent.run_id,
                "document belongs to another run"
            );
            let mut amendment = json!({"agent_id":agent.id,"created_at":now(),"call_id":call_id});
            if name == "threat_model_amend" {
                amendment["body"] = json!(text(args, "amendment")?);
            } else {
                ensure!(
                    args.get("body").is_some() || args.get("status").is_some(),
                    "todo_update requires a body or status"
                );
                if let Some(body) = args["body"].as_str() {
                    document.body = body.into();
                    amendment["body"] = json!(body);
                }
                if let Some(status) = args["status"].as_str() {
                    amendment["status"] = json!(status);
                }
            }
            document.amendments.push(amendment);
            document.updated_at = now();
            Store::tx_put(tx, kind, &document.id, &agent.run_id, &document)?;
            Ok(serde_json::to_value(document)?)
        })
    }
    fn coverage(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let evidence = string_list(args, "evidence_ids")?;
        self.store.verify_evidence_links(&agent.run_id, &evidence)?;
        self.effect(agent, call_id, |tx| {
            let mut coverage = if name == "coverage_update" {
                let record: Coverage = Store::tx_get(tx, "coverage", text(args, "id")?)?
                    .context("coverage missing")?;
                ensure!(
                    record.run_id == agent.run_id,
                    "coverage belongs to another run"
                );
                record
            } else {
                Coverage {
                    id: stable("cov", call_id),
                    run_id: agent.run_id.clone(),
                    agent_id: agent.id.clone(),
                    scope: text(args, "scope")?.into(),
                    kind: text(args, "kind")?.into(),
                    outcome: String::new(),
                    notes: String::new(),
                    evidence_ids: vec![],
                    created_at: now(),
                    updated_at: now(),
                }
            };
            coverage.outcome = text(args, "outcome")?.into();
            if let Some(notes) = args["notes"].as_str() {
                coverage.notes = notes.into();
            }
            if args.get("evidence_ids").is_some() {
                coverage.evidence_ids = evidence;
            }
            coverage.updated_at = now();
            Store::tx_put(tx, "coverage", &coverage.id, &agent.run_id, &coverage)?;
            Ok(serde_json::to_value(coverage)?)
        })
    }
    fn finding(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let evidence = string_list(args, "evidence_ids")?;
        self.store.verify_evidence_links(&agent.run_id, &evidence)?;
        self.effect(agent, call_id, |tx| {
            let mut finding = if name == "finding_update" {
                let record: Finding =
                    Store::tx_get(tx, "finding", text(args, "id")?)?.context("finding missing")?;
                ensure!(
                    record.run_id == agent.run_id,
                    "finding belongs to another run"
                );
                record
            } else {
                ensure!(!evidence.is_empty(), "a finding requires original evidence");
                if let Some(record) = Store::tx_list::<Finding>(tx, "finding", &agent.run_id)?
                    .into_iter()
                    .find(|record| {
                        record.title == args["title"]
                            && record.location == args.get("location").cloned()
                    })
                {
                    return Ok(json!({"duplicate":true,"finding":record}));
                }
                Finding {
                    id: stable("finding", call_id),
                    run_id: agent.run_id.clone(),
                    agent_id: agent.id.clone(),
                    title: text(args, "title")?.into(),
                    description: text(args, "description")?.into(),
                    root_cause: None,
                    impact: None,
                    severity: None,
                    severity_rationale: None,
                    stage: FindingStage::Suspected,
                    location: None,
                    evidence_ids: vec![],
                    verification_id: None,
                    created_at: now(),
                    updated_at: now(),
                }
            };
            for key in ["title", "description"] {
                if let Some(value) = args[key].as_str() {
                    ensure!(!value.trim().is_empty(), "{key} cannot be empty");
                    if key == "title" {
                        finding.title = value.into();
                    } else {
                        finding.description = value.into();
                    }
                }
            }
            if let Some(value) = args["root_cause"].as_str() {
                finding.root_cause = Some(value.into());
            }
            if let Some(value) = args["impact"].as_str() {
                finding.impact = Some(value.into());
            }
            if let Some(value) = args.get("severity") {
                finding.severity = Some(serde_json::from_value(value.clone())?);
            }
            if let Some(value) = args["severity_rationale"].as_str() {
                finding.severity_rationale = Some(value.into());
            }
            if let Some(value) = args["verification_id"].as_str() {
                finding.verification_id = Some(value.into());
            }
            if let Some(value) = args.get("location") {
                finding.location = Some(value.clone());
            }
            if args.get("evidence_ids").is_some() {
                ensure!(!evidence.is_empty(), "a finding requires evidence");
                finding.evidence_ids = evidence;
            }
            if let Some(value) = args.get("stage") {
                finding.stage = serde_json::from_value(value.clone())?;
            }
            if let Some(severity) = finding.severity {
                ensure!(
                    !matches!(severity, Severity::Critical | Severity::High)
                        || !matches!(
                            finding.stage,
                            FindingStage::Suspected
                                | FindingStage::Inconclusive
                                | FindingStage::Rejected
                        ),
                    "severity critical/high requires a reproduced-or-better stage; \
                     a suspicion or a bare crash is at most medium"
                );
            }
            if finding.stage == FindingStage::ImpactVerified {
                let id = finding
                    .verification_id
                    .as_ref()
                    .context("impact_verified requires an independent verification_id")?;
                let verification: Verification =
                    Store::tx_get(tx, "verification", id)?.context("verification missing")?;
                ensure!(
                    verification.run_id == agent.run_id
                        && verification.finding_id == finding.id
                        && verification.verifier_agent_id != finding.agent_id
                        && verification.passed
                        && verification.attempts > 0
                        && verification.successes == verification.attempts
                        && !verification.evidence_ids.is_empty(),
                    "independent successful verification is required"
                );
                let verifier: AgentRecord =
                    Store::tx_get(tx, "agent", &verification.verifier_agent_id)?
                        .context("verifier agent missing")?;
                ensure!(
                    verifier.role == Role::Verifier,
                    "verification agent role is invalid"
                );
                finding.impact = Some(verification.impact);
            }
            finding.updated_at = now();
            Store::tx_put(tx, "finding", &finding.id, &agent.run_id, &finding)?;
            Store::tx_event(
                tx,
                &agent.run_id,
                Some(&agent.id),
                "finding.updated",
                json!({"finding_id":finding.id,"stage":finding.stage}),
            )?;
            Ok(json!({"duplicate":false,"finding":finding}))
        })
    }

    async fn ghidra(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let run = self.target(agent)?;
        let query = match name {
            "functions_search" => GhidraQuery::Functions {
                pattern: args["pattern"].as_str().map(str::to_owned),
                offset: number(args, "offset", 0) as usize,
                limit: number(args, "limit", 25) as usize,
            },
            "function_decompile" => GhidraQuery::Decompile {
                address: number(args, "address", 0),
            },
            _ => GhidraQuery::Xrefs {
                address: number(args, "address", 0),
                direction: match args["direction"].as_str().unwrap_or("both") {
                    "to" => XrefDirection::To,
                    "from" => XrefDirection::From,
                    _ => XrefDirection::Both,
                },
                limit: number(args, "limit", 100) as usize,
            },
        };
        let executable = self
            .config
            .analysis
            .ghidra_executable
            .clone()
            .or_else(|| std::env::var_os("RE_AGENT_GHIDRA_HEADLESS").map(PathBuf::from));
        if executable.is_none() && self.runtime.is_some() {
            let staged = self
                .stage_target(agent, &format!("{call_id}:target"))
                .await?;
            let result=self.worker_call(agent,call_id,"ghidra.query",json!({"target":staged["path"],"target_sha256":run.target_sha256,"query":query,"timeout_secs":self.config.analysis.timeout_seconds,"analysis_timeout_secs":self.config.analysis.analysis_timeout_seconds,"max_cpu":self.config.analysis.max_cpu})).await?;
            ensure!(
                result["target_sha256"] == run.target_sha256,
                "remote Ghidra analyzed another target"
            );
            let mut result = result;
            let mut evidence_ids = Vec::new();
            if let Some(artifacts) = result["artifacts"].as_array() {
                for artifact in artifacts {
                    let raw = self
                        .runtime()?
                        .call(
                            "file.get",
                            json!({"path":artifact["path"],"max_bytes":32*1024*1024}),
                        )
                        .await?;
                    ensure!(
                        raw["sha256"] == artifact["sha256"],
                        "remote Ghidra artifact changed during import"
                    );
                    evidence_ids.push(self.import_file(agent, call_id, &raw)?.id);
                }
            }
            // The per-artifact metadata (binding/execution/request/response
            // digests) is redundant inline: the artifacts themselves are
            // already imported as evidence above. Keep the inline result to
            // the analysis payload the model actually reads.
            if let Some(object) = result.as_object_mut() {
                object.remove("artifacts");
            }
            return Ok(json!({"analysis":result,"artifact_evidence_ids":evidence_ids}));
        }
        let executable=executable.context("Ghidra is not configured; set analysis.ghidra_executable, RE_AGENT_GHIDRA_HEADLESS, or a worker with Ghidra")?;
        let backend = GhidraBackend::new(GhidraConfig {
            executable,
            cache_dir: self.config.analysis.ghidra_cache_dir.clone(),
            script_dir: self.config.analysis.ghidra_script_dir.clone(),
            timeout_seconds: self.config.analysis.timeout_seconds,
            analysis_timeout_seconds: self.config.analysis.analysis_timeout_seconds,
            max_cpu: self.config.analysis.max_cpu,
        })?;
        let result = backend.query(&run.target_path, &query).await?;
        let mut logs = Vec::new();
        for (kind, path) in [
            ("stdout", &result.stdout_path),
            ("stderr", &result.stderr_path),
        ] {
            let bytes = fs::read(path)?;
            let evidence = self.store.artifact(
                &agent.run_id,
                &agent.id,
                call_id,
                &format!("ghidra.{kind}"),
                &bytes,
                "text/plain",
                json!({"target_sha256":run.target_sha256,"tool_version":result.tool_version}),
            )?;
            logs.push(evidence.id);
        }
        Ok(json!({"analysis":result,"log_evidence_ids":logs}))
    }

    fn worker_identity(&self) -> Result<String> {
        Ok(stable(
            "worker",
            &serde_json::to_string(&self.config.runtime)?,
        ))
    }
    fn request_identity(&self, agent: &AgentRecord, call_id: &str) -> String {
        stable(
            "request",
            &format!("{}:{}:{call_id}", agent.run_id, agent.id),
        )
    }
    async fn worker_call(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        method: &str,
        mut params: Value,
    ) -> Result<Value> {
        let request_id = self.request_identity(agent, call_id);
        params["request_id"] = json!(request_id);
        params["call_id"] = json!(request_id);
        self.runtime()?.call(method,params).await.with_context(||format!("worker dispatch {method}; reconcile stable request {request_id} before changing its arguments"))
    }
    fn register_file(
        &self,
        agent: &AgentRecord,
        raw: &Value,
        evidence_id: Option<&str>,
    ) -> Result<()> {
        let path = raw["path"].as_str().context("worker file path missing")?;
        let file_id = stable("file", &format!("{}:{path}", agent.run_id));
        let previous: Option<Value> = self.store.get("worker_file", &file_id)?;
        let retained = previous
            .as_ref()
            .filter(|record| record["sha256"] == raw["sha256"])
            .and_then(|record| record["evidence_id"].as_str());
        let record = json!({"id":file_id,"run_id":agent.run_id,"agent_id":agent.id,"worker_id":self.worker_identity()?,"path":path,"sha256":raw["sha256"],"size":raw["size"],"evidence_id":evidence_id.or(retained),"created_at":now()});
        self.store.put(
            "worker_file",
            record["id"].as_str().context("file ID missing")?,
            &agent.run_id,
            &record,
        )
    }
    fn known_file(&self, agent: &AgentRecord, path: &str, require_target: bool) -> Result<Value> {
        let id = stable("file", &format!("{}:{path}", agent.run_id));
        let record: Value = self.store.require("worker_file", &id)?;
        ensure!(
            record["run_id"] == agent.run_id && record["worker_id"] == self.worker_identity()?,
            "worker file belongs to another assessment or runtime"
        );
        if require_target {
            let run: RunRecord = self.store.require("run", &agent.run_id)?;
            ensure!(
                record["sha256"] == run.target_sha256,
                "worker target does not match this run's sample SHA256"
            );
        }
        Ok(record)
    }
    async fn stage_target(&self, agent: &AgentRecord, call_id: &str) -> Result<Value> {
        let run = self.target(agent)?;
        let bytes = fs::read(&run.target_path)?;
        ensure!(
            bytes.len() <= 32 * 1024 * 1024,
            "worker staging currently supports samples up to 32 MiB"
        );
        let raw=self.worker_call(agent,call_id,"file.put",json!({"path":format!("targets/{}/sample.bin",run.target_sha256),"data_base64":BASE64.encode(&bytes),"executable":true})).await?;
        ensure!(
            raw["sha256"] == run.target_sha256,
            "worker staged bytes differ from run target"
        );
        self.register_file(agent, &raw, None)?;
        Ok(raw)
    }
    async fn file_put(&self, agent: &AgentRecord, call_id: &str, args: &Value) -> Result<Value> {
        ensure!(
            args.get("data_base64").is_some() != args.get("evidence_id").is_some(),
            "supply exactly one of data_base64 or evidence_id"
        );
        let bytes = if let Some(evidence) = args["evidence_id"].as_str() {
            self.store.read_evidence(&agent.run_id, evidence)?
        } else {
            BASE64
                .decode(
                    args["data_base64"]
                        .as_str()
                        .context("data_base64 must be a string")?,
                )
                .context("invalid base64 upload")?
        };
        ensure!(
            bytes.len() <= 32 * 1024 * 1024,
            "file upload exceeds 32 MiB"
        );
        let raw=self.worker_call(agent,call_id,"file.put",json!({"path":text(args,"path")?,"data_base64":BASE64.encode(&bytes),"executable":args["executable"].as_bool().unwrap_or(false)})).await?;
        ensure!(
            raw["sha256"] == binary::sha256_bytes(&bytes),
            "worker upload hash mismatch"
        );
        let evidence = self.store.artifact(
            &agent.run_id,
            &agent.id,
            call_id,
            "file_put.original",
            &bytes,
            "application/octet-stream",
            json!({"worker_path":raw["path"]}),
        )?;
        self.register_file(agent, &raw, Some(&evidence.id))?;
        Ok(json!({"file":raw,"content_evidence_id":evidence.id}))
    }
    async fn file_get(&self, agent: &AgentRecord, call_id: &str, args: &Value) -> Result<Value> {
        let path = text(args, "path")?;
        if self.known_file(agent, path, false).is_err() {
            let allowed = self
                .store
                .list::<Value>("job", &agent.run_id)?
                .iter()
                .any(|job| {
                    job["worker_id"] == self.worker_identity().unwrap_or_default()
                        && job["id"]
                            .as_str()
                            .is_some_and(|id| path.starts_with(&format!("jobs/{id}/")))
                });
            ensure!(allowed, "worker path has not been declared for this run");
        }
        let raw = self
            .runtime()?
            .call(
                "file.get",
                json!({"path":path,"max_bytes":number(args,"max_bytes",32*1024*1024)}),
            )
            .await?;
        let evidence = self.import_file(agent, call_id, &raw)?;
        Ok(json!({"file":raw,"content_evidence_id":evidence.id}))
    }
    fn import_file(&self, agent: &AgentRecord, call_id: &str, raw: &Value) -> Result<Evidence> {
        let bytes = BASE64
            .decode(
                raw["data_base64"]
                    .as_str()
                    .context("worker file has no base64 content")?,
            )
            .context("worker returned invalid file base64")?;
        ensure!(
            raw["sha256"] == binary::sha256_bytes(&bytes)
                && raw["size"].as_u64() == Some(bytes.len() as u64),
            "worker file integrity mismatch"
        );
        let evidence = self.store.artifact(
            &agent.run_id,
            &agent.id,
            call_id,
            "runtime.file",
            &bytes,
            "application/octet-stream",
            json!({"worker_path":raw["path"],"worker_id":self.worker_identity()?}),
        )?;
        self.register_file(agent, raw, Some(&evidence.id))?;
        Ok(evidence)
    }

    fn known_job(&self, agent: &AgentRecord, job_id: &str, owned: bool) -> Result<Value> {
        let job: Value = self.require_in_run("job", job_id, &agent.run_id)?;
        ensure!(
            job["worker_id"] == self.worker_identity()?,
            "job belongs to a different runtime configuration"
        );
        if owned {
            ensure!(
                job["agent_id"] == agent.id,
                "this operation requires the job's owning agent"
            );
        }
        Ok(job)
    }
    fn authorize_session(&self, agent: &AgentRecord, session: &Value, closing: bool) -> Result<()> {
        ensure!(
            session["worker_id"] == self.worker_identity()?,
            "session belongs to another runtime"
        );
        if session["agent_id"] != agent.id {
            let owner: AgentRecord =
                self.require_in_run("agent", text(session, "agent_id")?, &agent.run_id)?;
            ensure!(
                closing && owner.status.terminal(),
                "session is owned by another active agent"
            );
        }
        Ok(())
    }
    async fn observe_job(&self, agent: &AgentRecord, call_id: &str, job_id: &str) -> Result<Value> {
        let mut job = self.known_job(agent, job_id, false)?;
        let status = self
            .runtime()?
            .call("job.status", json!({"job_id":job_id}))
            .await?;
        ensure!(status["job_id"] == job_id, "worker returned the wrong job");
        job["status"] = status.clone();
        job["updated_at"] = json!(now());
        self.store.put("job", job_id, &agent.run_id, &job)?;
        if let Some(crashes) = status["crashes"].as_array() {
            for artifact in crashes {
                self.register_file(agent, artifact, None)?;
                if let Some(path) = artifact["path"].as_str() {
                    let existing = self.known_file(agent, path, false)?;
                    if existing["evidence_id"].is_null() {
                        let raw = self
                            .runtime()?
                            .call("file.get", json!({"path":path,"max_bytes":32*1024*1024}))
                            .await?;
                        self.import_file(agent, call_id, &raw)?;
                    }
                }
            }
        }
        if ["completed", "failed", "cancelled", "timed_out", "lost"]
            .contains(&status["state"].as_str().unwrap_or_default())
        {
            self.import_job_artifacts(agent, call_id, job_id).await?;
        }
        Ok(status)
    }
    async fn import_job_artifacts(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        job_id: &str,
    ) -> Result<Value> {
        let mut job = self.known_job(agent, job_id, false)?;
        if let Some(imported) = job.get("imported_artifacts") {
            return Ok(imported.clone());
        }
        let artifacts = self
            .runtime()?
            .call("job.artifacts", json!({"job_id":job_id}))
            .await?;
        let mut imported = Vec::new();
        for artifact in artifacts
            .as_array()
            .context("worker artifact list is not an array")?
        {
            ensure!(
                artifact["size"]
                    .as_u64()
                    .is_some_and(|size| size <= 32 * 1024 * 1024),
                "worker artifact exceeds 32 MiB import limit"
            );
            let raw = self
                .runtime()?
                .call(
                    "file.get",
                    json!({"path":artifact["path"],"max_bytes":32*1024*1024}),
                )
                .await?;
            ensure!(
                raw["sha256"] == artifact["sha256"],
                "job artifact changed while being imported"
            );
            let evidence = self.import_file(agent, call_id, &raw)?;
            imported.push(json!({"artifact":artifact,"evidence_id":evidence.id}));
        }
        let result = json!({"job_id":job_id,"artifacts":imported});
        if ["completed", "failed", "cancelled", "timed_out", "lost"]
            .contains(&job["status"]["state"].as_str().unwrap_or_default())
        {
            job["imported_artifacts"] = result.clone();
            self.store.put("job", job_id, &agent.run_id, &job)?;
        }
        Ok(result)
    }

    async fn runtime_tool(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        mut args: Value,
    ) -> Result<Value> {
        match name {
            "job_status" | "fuzz_status" => {
                self.observe_job(agent, call_id, text(&args, "job_id")?)
                    .await
            }
            "job_cancel" => {
                let job_id = text(&args, "job_id")?;
                self.known_job(agent, job_id, true)?;
                self.worker_call(agent, call_id, "job.cancel", args).await
            }
            "job_artifacts" => {
                self.import_job_artifacts(agent, call_id, text(&args, "job_id")?)
                    .await
            }
            "debug_command" | "debug_close" => {
                let session_id = text(&args, "session_id")?.to_owned();
                let mut session: Value =
                    self.require_in_run("debug_session", &session_id, &agent.run_id)?;
                self.authorize_session(agent, &session, name == "debug_close")?;
                ensure!(
                    session["state"] == "running",
                    "debug session is closed or lost"
                );
                let raw = self
                    .worker_call(
                        agent,
                        call_id,
                        if name == "debug_close" {
                            "debug.close"
                        } else {
                            "debug.command"
                        },
                        args,
                    )
                    .await?;
                let transcript = raw["transcript"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("debug/{session_id}/transcript.mi"));
                let transcript = self
                    .runtime()?
                    .call(
                        "file.get",
                        json!({"path":transcript,"max_bytes":8*1024*1024}),
                    )
                    .await?;
                let evidence = self.import_file(agent, call_id, &transcript)?;
                session["transcript_evidence_id"] = json!(evidence.id);
                if name == "debug_close" {
                    session["state"] = json!("closed");
                }
                session["updated_at"] = json!(now());
                self.store
                    .put("debug_session", &session_id, &agent.run_id, &session)?;
                Ok(json!({"response":raw,"transcript_evidence_id":evidence.id}))
            }
            "debug_start" => {
                self.known_file(agent, text(&args, "target")?, true)?;
                let raw = self
                    .worker_call(agent, call_id, "debug.start", args)
                    .await?;
                let session_id = raw["session_id"]
                    .as_str()
                    .context("worker debug session ID missing")?;
                let record = json!({"id":session_id,"run_id":agent.run_id,"agent_id":agent.id,"worker_id":self.worker_identity()?,"state":raw["state"],"worker":raw,"created_at":now(),"updated_at":now()});
                self.store
                    .put("debug_session", session_id, &agent.run_id, &record)?;
                Ok(raw)
            }
            "interactive_start" => {
                ensure!(
                    !string_list(&args, "argv")?.is_empty(),
                    "interactive argv cannot be empty"
                );
                let raw = self
                    .worker_call(agent, call_id, "interact.start", args)
                    .await?;
                let session_id = raw["session_id"]
                    .as_str()
                    .context("worker interactive session ID missing")?;
                let record = json!({"id":session_id,"run_id":agent.run_id,"agent_id":agent.id,"worker_id":self.worker_identity()?,"state":"running","created_at":now(),"updated_at":now()});
                self.store
                    .put("interact_session", session_id, &agent.run_id, &record)?;
                Ok(raw)
            }
            "interactive_send" | "interactive_close" => {
                let session_id = text(&args, "session_id")?.to_owned();
                let mut session: Value =
                    self.require_in_run("interact_session", &session_id, &agent.run_id)?;
                self.authorize_session(agent, &session, name == "interactive_close")?;
                ensure!(
                    session["state"] == "running"
                        || (name == "interactive_close" && session["state"] == "exited"),
                    "interactive session is closed or lost"
                );
                let raw = self
                    .worker_call(
                        agent,
                        call_id,
                        if name == "interactive_close" {
                            "interact.close"
                        } else {
                            "interact.write"
                        },
                        args,
                    )
                    .await?;
                if name == "interactive_close" {
                    session["state"] = json!("closed");
                } else if raw["state"].as_str() == Some("exited") {
                    session["state"] = json!("exited");
                }
                session["updated_at"] = json!(now());
                self.store
                    .put("interact_session", &session_id, &agent.run_id, &session)?;
                Ok(raw)
            }
            "job_start" | "fuzz_start" | "crash_replay" | "input_minimize" | "poc_verify"
            | "angr_solve" => {
                let run: RunRecord = self.store.require("run", &agent.run_id)?;
                if name == "angr_solve" {
                    ensure!(
                        !address_list(&args, "find")?.is_empty(),
                        "angr_solve requires at least one find address"
                    );
                }
                if name != "job_start" {
                    self.known_file(agent, text(&args, "target")?, true)?;
                    if !matches!(name, "fuzz_start" | "angr_solve") {
                        self.known_file(agent, text(&args, "input")?, false)?;
                    }
                } else {
                    ensure!(
                        !string_list(&args, "argv")?.is_empty(),
                        "job argv cannot be empty"
                    );
                }
                if name == "fuzz_start" {
                    let seeds = text(&args, "seeds")?;
                    let prefix = format!("{}/", seeds.trim_end_matches('/'));
                    ensure!(
                        self.store
                            .list::<Value>("worker_file", &agent.run_id)?
                            .iter()
                            .any(|file| file["worker_id"]
                                == self.worker_identity().unwrap_or_default()
                                && file["path"]
                                    .as_str()
                                    .is_some_and(|path| path.starts_with(&prefix))),
                        "seed directory must contain staged in-run files"
                    );
                }
                let finding_id = args["finding_id"].as_str().map(str::to_owned);
                if name == "poc_verify" {
                    ensure!(
                        agent.role == Role::Verifier,
                        "poc_verify requires verifier role"
                    );
                    let finding: Finding = self.require_in_run(
                        "finding",
                        finding_id.as_deref().context("finding_id missing")?,
                        &agent.run_id,
                    )?;
                    ensure!(
                        finding.agent_id != agent.id,
                        "the verifier must differ from the finding producer"
                    );
                    args.as_object_mut()
                        .context("arguments not an object")?
                        .remove("finding_id");
                    args["producer_agent_id"] = json!(finding.agent_id);
                    args["verifier_agent_id"] = json!(agent.id);
                }
                let method = match name {
                    "job_start" => "job.start",
                    "fuzz_start" => "fuzz.start",
                    "crash_replay" => "crash.replay",
                    "input_minimize" => "input.minimize",
                    "angr_solve" => "angr.solve",
                    _ => "poc.verify",
                };
                // job_start argv may reference the staged target via the
                // {target} placeholder; the harness resolves it to the worker
                // path so scripts receive it as a plain argv element (staged
                // in isolated jobs) instead of guessing the worker layout.
                let params = if name == "job_start" {
                    substitute_target_argv(&args, &run.target_sha256)
                } else {
                    args.clone()
                };
                let raw = self
                    .worker_call(agent, call_id, method, params.clone())
                    .await?;
                let job_id = raw["job_id"].as_str().context("worker job ID missing")?;
                let record = json!({"id":job_id,"run_id":agent.run_id,"agent_id":agent.id,"worker_id":self.worker_identity()?,"tool_name":name,"finding_id":finding_id,"call_id":call_id,"worker_request_id":self.request_identity(agent,call_id),"parameters":params,"target_sha256":run.target_sha256,"status":raw,"created_at":now(),"updated_at":now()});
                if let Some(previous) = self.store.get::<Value>("job", job_id)? {
                    ensure!(
                        previous["run_id"] == agent.run_id
                            && previous["agent_id"] == agent.id
                            && previous["call_id"] == call_id,
                        "worker job identity is already bound to another request"
                    );
                }
                self.store.put("job", job_id, &agent.run_id, &record)?;
                Ok(raw)
            }
            _ => bail!("unsupported runtime tool {name}"),
        }
    }

    async fn verification(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        args: &Value,
    ) -> Result<Value> {
        ensure!(
            agent.role == Role::Verifier,
            "only a verifier may record independent verification"
        );
        let finding: Finding =
            self.require_in_run("finding", text(args, "finding_id")?, &agent.run_id)?;
        ensure!(
            finding.agent_id != agent.id,
            "verification must be independent of the finding producer"
        );
        let job_id = text(args, "job_id")?;
        let job = self.known_job(agent, job_id, true)?;
        ensure!(
            job["tool_name"] == "poc_verify" && job["finding_id"] == finding.id,
            "job is not an independent verification of this finding"
        );
        let status = self.observe_job(agent, call_id, job_id).await?;
        ensure!(
            status["state"] == "completed" && status["kind"] == "verify",
            "verification job has not completed successfully"
        );
        let result = &status["result"];
        let run: RunRecord = self.store.require("run", &agent.run_id)?;
        ensure!(
            result["target"]["sha256"] == run.target_sha256
                && job["target_sha256"] == run.target_sha256,
            "verification target SHA256 differs from finding target"
        );
        ensure!(
            result["verifier_agent_id"] == agent.id
                && result["producer_agent_id"] == finding.agent_id
                && result["call_id"] == job["worker_request_id"],
            "worker verification identity differs from its durable request"
        );
        ensure!(
            result["fresh_working_directory_per_trial"].as_bool() == Some(true),
            "verification did not use clean trial directories"
        );
        let attempts = result["attempts"]
            .as_u64()
            .context("worker attempt count missing")?;
        let trials = result["trials"]
            .as_u64()
            .context("worker trial count missing")?;
        let successes = result["successes"]
            .as_u64()
            .context("worker success count missing")?;
        let requested = result["requested_trials"]
            .as_u64()
            .context("worker requested trial count missing")?;
        ensure!(
            attempts > 0
                && attempts == trials
                && attempts <= 16
                && successes <= attempts
                && requested >= attempts,
            "worker trial counts are inconsistent"
        );
        let observations = result["results"]
            .as_array()
            .context("worker trial results missing")?;
        ensure!(
            observations.len() as u64 == attempts,
            "worker trial observations do not match attempt count"
        );
        let input = self.known_file(agent, text(&job["parameters"], "input")?, false)?;
        ensure!(
            result["input"]["sha256"] == input["sha256"]
                && observations
                    .iter()
                    .all(|trial| trial["input_sha256"] == input["sha256"]),
            "verification input identity mismatch"
        );
        let passed = result["passed"]
            .as_bool()
            .context("worker passed flag missing")?;
        ensure!(
            !passed || (successes == attempts && attempts == requested),
            "worker success verdict contradicts trial observations"
        );
        let raw_evidence = self.store.artifact(
            &agent.run_id,
            &agent.id,
            call_id,
            "verification.raw_job_status",
            &serde_json::to_vec(&status)?,
            "application/json",
            json!({"job_id":job_id,"finding_id":finding.id,"target_sha256":run.target_sha256}),
        )?;
        let artifacts = self.import_job_artifacts(agent, call_id, job_id).await?;
        let mut evidence_ids = vec![raw_evidence.id];
        for artifact in artifacts["artifacts"]
            .as_array()
            .context("artifact import missing")?
        {
            evidence_ids.push(text(artifact, "evidence_id")?.into());
        }
        let observed_stage = result["observed_stage"].as_str().unwrap_or("unknown");
        let impact = match observed_stage {
            "crash" => "reproducible crash",
            _ => "observed behavior matches the recorded expectation",
        };
        // These exact labels are deliberately limited to what the worker
        // observed. Analyst claims cannot turn a crash into code execution.
        let verification = Verification {
            id: stable(
                "verification",
                &format!("{}:{}:{job_id}", agent.run_id, finding.id),
            ),
            run_id: agent.run_id.clone(),
            finding_id: finding.id.clone(),
            verifier_agent_id: agent.id.clone(),
            passed,
            attempts,
            successes,
            impact: impact.into(),
            evidence_ids,
            environment: result["environment"].clone(),
            conditions: json!({"worker_parameters":job["parameters"],"expected":result["expected"],"analyst_conditions":args.get("conditions"),"analyst_impact_claim":args.get("impact"),"observed_stage":observed_stage,"job_id":job_id}),
            result: status,
            created_at: now(),
        };
        self.effect(agent,call_id,|tx|{
            if let Some(existing)=Store::tx_get::<Verification>(tx,"verification",&verification.id)?{return Ok(serde_json::to_value(existing)?);}
            Store::tx_put(tx,"verification",&verification.id,&agent.run_id,&verification)?;
            Store::tx_event(tx,&agent.run_id,Some(&agent.id),"verification.recorded",json!({"verification_id":verification.id,"finding_id":finding.id,"passed":passed,"attempts":attempts,"successes":successes,"impact":verification.impact}))?;
            Ok(serde_json::to_value(verification)?)
        })
    }
}

/// Replace the `{target}` placeholder in a job_start argv with the staged
/// target worker path, so scripts receive it as a plain argv element (the
/// bridge stages such paths in isolated jobs) instead of guessing the worker
/// layout.
fn substitute_target_argv(args: &Value, target_sha256: &str) -> Value {
    let staged = format!("files/targets/{target_sha256}/sample.bin");
    let mut params = args.clone();
    if let Some(argv) = params.get_mut("argv").and_then(Value::as_array_mut) {
        for entry in argv.iter_mut() {
            if let Some(text) = entry.as_str() {
                *entry = json!(text.replace("{target}", &staged));
            }
        }
    }
    params
}

fn finish_record(
    store: &Store,
    limit: usize,
    mut record: ToolCallRecord,
    raw: Value,
    is_error: bool,
    state: CallState,
) -> Result<ToolOutcome> {
    let bytes = serde_json::to_vec(&raw)?;
    let run: RunRecord = store.require("run", &record.run_id)?;
    let evidence=store.artifact(&record.run_id,&record.agent_id,&record.id,&record.name,&bytes,"application/json",json!({"raw_tool_output":true,"tool":record.name,"is_error":is_error,"target_sha256":run.target_sha256,"tool_protocol_version":1}))?;
    let envelope_overhead = serde_json::to_vec(&json!({"result":null,"evidence_id":evidence.id,"truncated":false,"total_bytes":bytes.len()}))?.len().saturating_sub(4);
    let truncated = bytes.len().saturating_add(envelope_overhead) > limit;
    let result = if truncated {
        let mut summary = json!({"preview":prefix_utf8(&bytes,limit.saturating_sub(1024)),"retrieve":"use evidence_read with byte ranges"});
        for key in [
            "id",
            "job_id",
            "session_id",
            "state",
            "status",
            "path",
            "sha256",
            "finding_id",
            "verification_id",
            "parked",
            "wake_at",
        ] {
            if let Some(value) = raw.get(key) {
                let value = match value {
                    Value::String(value) => Some(json!(prefix_utf8(value.as_bytes(), 128))),
                    Value::Bool(_) | Value::Number(_) | Value::Null => Some(value.clone()),
                    _ => None,
                };
                if let Some(value) = value {
                    summary[key] = value;
                }
            }
        }
        summary
    } else {
        raw
    };
    let mut outcome = ToolOutcome {
        value: json!({"result":result,"evidence_id":evidence.id,"truncated":truncated,"total_bytes":bytes.len()}),
        is_error,
    };
    if truncated {
        while serde_json::to_vec(&outcome.value)?.len() > limit {
            let preview = outcome.value["result"]["preview"]
                .as_str()
                .unwrap_or_default();
            if !preview.is_empty() {
                outcome.value["result"]["preview"] =
                    json!(prefix_utf8(preview.as_bytes(), preview.len() / 2));
            } else {
                let summary = outcome.value["result"]
                    .as_object_mut()
                    .context("bounded result is not an object")?;
                let key = summary
                    .keys()
                    .find(|key| !matches!(key.as_str(), "preview" | "retrieve"))
                    .cloned()
                    .context(
                        "configured tool output limit is too small for the evidence envelope",
                    )?;
                summary.remove(&key);
            }
        }
    }
    record.state = state;
    record.result = Some(serde_json::to_value(&outcome)?);
    record.evidence_id = Some(evidence.id);
    record.updated_at = now();
    store.finish_call(&record)?;
    Ok(outcome)
}

/// Resolve an unknown effect from operator-supplied evidence without opening
/// any provider, runtime or MCP connection. A live controller lease blocks it.
pub fn reconcile_record(
    store: &Store,
    call_id: &str,
    resolution: Value,
    evidence_ids: &[String],
    is_error: bool,
) -> Result<ToolOutcome> {
    let record: ToolCallRecord = store.require("call", call_id)?;
    ensure!(
        matches!(record.state, CallState::Running | CallState::Uncertain),
        "only an unresolved call can be reconciled"
    );
    ensure!(
        resolution.is_object() && !evidence_ids.is_empty(),
        "operator reconciliation requires a result object and supporting evidence"
    );
    store.verify_evidence_links(&record.run_id, evidence_ids)?;
    let owner = id("reconcile");
    ensure!(
        store.acquire_lease(&record.run_id, &owner, 30)?,
        "a live controller owns this assessment; pause it before reconciling an unknown effect"
    );
    let result = (|| {
        store.event(
            &record.run_id,
            Some(&record.agent_id),
            "tool.operator_reconciled",
            json!({"call_id":call_id,"evidence_ids":evidence_ids,"is_error":is_error}),
        )?;
        finish_record(
            store,
            12000,
            record.clone(),
            json!({"operator_reconciled":true,"result":resolution,"supporting_evidence_ids":evidence_ids}),
            is_error,
            if is_error {
                CallState::Failed
            } else {
                CallState::Completed
            },
        )
    })();
    let release = store.release_lease(&record.run_id, &owner);
    match result {
        Ok(value) => {
            release?;
            Ok(value)
        }
        Err(error) => {
            let _ = release;
            Err(error)
        }
    }
}

fn stable(prefix: &str, value: &str) -> String {
    let digest = hex::encode(Sha256::digest(value.as_bytes()));
    if prefix == "agent" {
        digest
    } else {
        format!("{prefix}_{digest}")
    }
}
fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .with_context(|| format!("{key} must be a nonempty string"))
}
/// Plain decimal and 0x-hex integer spellings accepted anywhere an address or
/// offset appears, matching the documented "integers and 0x-hex strings are
/// interchangeable" contract.
fn parse_u64_text(text: &str) -> Option<u64> {
    let text = text.trim().replace('_', "");
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        text.parse::<u64>().ok()
    }
}
fn number(args: &Value, key: &str, default: u64) -> u64 {
    match &args[key] {
        Value::Number(value) => value.as_u64().unwrap_or(default),
        Value::String(text) => parse_u64_text(text).unwrap_or(default),
        _ => default,
    }
}
fn address_list(args: &Value, key: &str) -> Result<Vec<u64>> {
    let Some(value) = args.get(key) else {
        return Ok(vec![]);
    };
    value
        .as_array()
        .with_context(|| format!("{key} must be an array"))?
        .iter()
        .map(|item| match item {
            Value::Number(number) => number
                .as_u64()
                .with_context(|| format!("{key} must contain nonnegative integer addresses")),
            Value::String(text) => parse_u64_text(text).with_context(|| {
                format!("{key} must contain nonnegative integer or 0x-hex string addresses")
            }),
            _ => bail!("{key} must contain nonnegative integer or 0x-hex string addresses"),
        })
        .collect()
}
fn string_list(args: &Value, key: &str) -> Result<Vec<String>> {
    args.get(key)
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .with_context(|| format!("{key} must contain strings"))
        .map(|list| list.unwrap_or_default())
}
fn prefix_utf8(bytes: &[u8], limit: usize) -> String {
    let mut end = bytes.len().min(limit);
    while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
        end -= 1;
    }
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Preserve the project's numeric-address syntax before Rig validates JSON Schema.
/// The original arguments stay in the call ledger; only dispatched values normalize.
pub(crate) fn normalize_integer_arguments(value: &mut Value, schema: &Value) {
    if schema["type"] == "integer" {
        if let Some(parsed) = value.as_str().and_then(parse_u64_text) {
            *value = json!(parsed);
        }
    } else if let Some(object) = value.as_object_mut() {
        for (name, value) in object {
            if let Some(property) = schema["properties"].get(name) {
                normalize_integer_arguments(value, property);
            }
        }
    } else if let Some(array) = value.as_array_mut() {
        if let Some(items) = schema.get("items") {
            for value in array {
                normalize_integer_arguments(value, items);
            }
        }
    }
}
fn validated_arguments(mut value: Value, schema: &Value) -> Result<Value> {
    normalize_integer_arguments(&mut value, schema);
    rig_core::tool::validate::validate(schema, &value).map_err(|problems| {
        anyhow::anyhow!(
            "invalid tool arguments: {}",
            problems
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{substitute_target_argv, validated_arguments};

    #[test]
    fn rig_schema_validation_preserves_address_syntax_and_checks_collection_limits() {
        let schema = serde_json::json!({"type":"object","properties":{"address":{"type":"integer","minimum":1},"names":{"type":"array","maxItems":1,"items":{"type":"string","maxLength":3}}},"required":["address"],"additionalProperties":false});
        let normalized = validated_arguments(
            serde_json::json!({"address":"0x40","names":["abc"]}),
            &schema,
        )
        .unwrap();
        assert_eq!(normalized["address"], 64);
        for invalid in [
            serde_json::json!({"address":0}),
            serde_json::json!({"address":"bad"}),
            serde_json::json!({"address":1,"names":["long"]}),
            serde_json::json!({"address":1,"names":["a","b"]}),
            serde_json::json!({"address":1,"extra":true}),
        ] {
            assert!(validated_arguments(invalid, &schema).is_err());
        }
    }

    #[test]
    fn target_placeholder_replaces_argv_entries() {
        let args = serde_json::json!({
            "argv": ["python3", "files/tools/exploit.py", "{target}", "extra"],
            "timeout_secs": 10,
        });
        let params = substitute_target_argv(&args, "deadbeef");
        let argv = params["argv"].as_array().unwrap();
        assert_eq!(argv[0], "python3");
        assert_eq!(argv[1], "files/tools/exploit.py");
        assert_eq!(argv[2], "files/targets/deadbeef/sample.bin");
        assert_eq!(argv[3], "extra");
        assert_eq!(params["timeout_secs"], 10);
    }

    #[test]
    fn target_placeholder_ignores_non_job_shapes() {
        let args = serde_json::json!({"argv": "not-an-array"});
        let params = substitute_target_argv(&args, "deadbeef");
        assert_eq!(params, args);
    }
}
