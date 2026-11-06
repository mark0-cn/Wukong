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

struct Spec {
    definition: ToolDefinition,
    side_effecting: bool,
    target: bool,
    verifier_only: bool,
    root_only: bool,
    child_only: bool,
}
fn schema(kind: &str) -> Value {
    json!({"type":kind})
}
fn strings() -> Value {
    json!({"type":"array","items":{"type":"string"}})
}
fn integer(min: u64, max: u64) -> Value {
    json!({"type":"integer","minimum":min,"maximum":max})
}
fn spec(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    side_effecting: bool,
    target: bool,
) -> Spec {
    Spec {definition:ToolDefinition::new(ToolName::new(name).expect("built-in tool name"),description,json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}))
        .with_annotations(ToolAnnotations::default().with_read_only(!side_effecting).with_open_world(target)),side_effecting,target,verifier_only:false,root_only:false,child_only:false}
}

fn builtins() -> Vec<Spec> {
    let s = schema("string");
    let a = strings();
    let o = schema("object");
    let b = schema("boolean");
    let id = json!({"type":"string","minLength":1});
    let mut specs = vec![
        spec(
            "create_agent",
            "Create an asynchronous focused child and return immediately; inherited context is optional.",
            json!({"name":id,"role":{"type":"string","enum":["recon","reverse","hunter","fuzz","crash","exploit","verifier","reporter"]},"task":id,"scope":id,"skills":a,"inherit_context":b}),
            &["name", "role", "task", "scope"],
            true,
            false,
        ),
        spec(
            "view_agent_graph",
            "List this assessment's agent tree, scopes, states and results.",
            json!({}),
            &[],
            false,
            false,
        ),
        spec(
            "send_message_to_agent",
            "Durably send an in-run message and wake a waiting recipient.",
            json!({"agent_id":id,"message":id}),
            &["agent_id", "message"],
            true,
            false,
        ),
        spec(
            "wait_for_agents",
            "Park until selected in-run agents finish or the timeout expires. Empty list means children.",
            json!({"agent_ids":a,"timeout_seconds":integer(1,3600)}),
            &[],
            true,
            false,
        ),
        spec(
            "stop_agent",
            "Stop yourself or your descendants, with a recorded reason.",
            json!({"agent_id":id,"reason":id}),
            &["agent_id", "reason"],
            true,
            false,
        ),
        spec(
            "agent_finish",
            "Finish a child task with a substantive summary; active descendants must finish first.",
            json!({"summary":id}),
            &["summary"],
            true,
            false,
        ),
        spec(
            "finish_scan",
            "Root only: finish the assessment after children and coverage are resolved.",
            json!({"summary":id}),
            &["summary"],
            true,
            false,
        ),
        spec(
            "threat_model_get",
            "Read the shared threat model documents.",
            json!({"id":s}),
            &[],
            false,
            false,
        ),
        spec(
            "threat_model_save",
            "Create a shared threat model with a stable document ID.",
            json!({"title":id,"body":id}),
            &["title", "body"],
            true,
            false,
        ),
        spec(
            "threat_model_amend",
            "Append a durable amendment to a shared threat model.",
            json!({"id":id,"amendment":id}),
            &["id", "amendment"],
            true,
            false,
        ),
        spec(
            "note_save",
            "Save a shared note linked to this task.",
            json!({"title":id,"body":id}),
            &["title", "body"],
            true,
            false,
        ),
        spec(
            "notes_list",
            "List shared notes for this assessment.",
            json!({}),
            &[],
            false,
            false,
        ),
        spec(
            "todo_save",
            "Create a shared to-do item; status defaults to pending.",
            json!({"title":id,"body":s}),
            &["title"],
            true,
            false,
        ),
        spec(
            "todo_update",
            "Update the body or status of an existing shared to-do.",
            json!({"id":id,"body":s,"status":{"type":"string","enum":["pending","in_progress","completed","inconclusive"]}}),
            &["id"],
            true,
            false,
        ),
        spec(
            "todos_list",
            "List shared to-do items.",
            json!({}),
            &[],
            false,
            false,
        ),
        spec(
            "coverage_record",
            "Record inspected scope, outcome and supporting evidence. needs_follow_up blocks root completion.",
            json!({"scope":id,"kind":id,"outcome":{"type":"string","enum":["inspected","needs_follow_up","inconclusive","completed","not_reachable"]},"notes":s,"evidence_ids":a}),
            &["scope", "kind", "outcome"],
            true,
            false,
        ),
        spec(
            "coverage_update",
            "Update a coverage record's outcome, notes and evidence.",
            json!({"id":id,"outcome":{"type":"string","enum":["inspected","needs_follow_up","inconclusive","completed","not_reachable"]},"notes":s,"evidence_ids":a}),
            &["id", "outcome"],
            true,
            false,
        ),
        spec(
            "coverage_list",
            "List all recorded assessment coverage.",
            json!({}),
            &[],
            false,
            false,
        ),
        spec(
            "finding_file",
            "File a suspected or reproduced finding with evidence. Verified impact requires a separate immutable verifier record.",
            json!({"title":id,"description":id,"root_cause":s,"impact":s,"stage":{"type":"string","enum":["suspected","reproduced","root_cause_confirmed","impact_verified","inconclusive","rejected"]},"location":o,"evidence_ids":a,"verification_id":s}),
            &["title", "description", "evidence_ids"],
            true,
            false,
        ),
        spec(
            "finding_update",
            "Update an existing finding. Impact promotion is enforced using independent execution evidence.",
            json!({"id":id,"title":s,"description":s,"root_cause":s,"impact":s,"stage":{"type":"string","enum":["suspected","reproduced","root_cause_confirmed","impact_verified","inconclusive","rejected"]},"location":o,"evidence_ids":a,"verification_id":s}),
            &["id"],
            true,
            false,
        ),
        spec(
            "findings_list",
            "List this assessment's findings and recorded stages.",
            json!({}),
            &[],
            false,
            false,
        ),
        spec(
            "finding_get",
            "Read a finding and its independent verification record.",
            json!({"id":id}),
            &["id"],
            false,
            false,
        ),
        spec(
            "verification_record",
            "Verifier only: ingest a completed poc_verify job, its raw evidence and real trial counts. A crash verifies only crash behavior.",
            json!({"finding_id":id,"job_id":id,"impact":s,"conditions":o}),
            &["finding_id", "job_id"],
            true,
            true,
        ),
        spec(
            "evidence_read",
            "Read a bounded byte range from an integrity-checked evidence artifact. Non-UTF8 is base64.",
            json!({"evidence_id":id,"offset":integer(0,u64::MAX),"length":integer(1,1024*1024)}),
            &["evidence_id"],
            false,
            false,
        ),
        spec(
            "load_skill",
            "Load one built-in or configured analysis skill by name.",
            json!({"name":id}),
            &["name"],
            false,
            false,
        ),
        spec(
            "binary_inspect",
            "Inspect the exact run target, architecture, sections, imports, symbols and mitigations.",
            json!({}),
            &[],
            false,
            true,
        ),
        spec(
            "binary_strings",
            "Search printable strings with file-offset address references and pagination.",
            json!({"min_length":integer(1,65536),"max_length":integer(1,65536),"offset":integer(0,u64::MAX),"limit":integer(1,5000),"contains":s}),
            &[],
            false,
            true,
        ),
        spec(
            "functions_search",
            "Ghidra: search analyzed functions, paginated.",
            json!({"pattern":s,"offset":integer(0,u64::MAX),"limit":integer(1,5000)}),
            &[],
            false,
            true,
        ),
        spec(
            "function_decompile",
            "Ghidra: decompile a function by MODULE-RELATIVE offset.",
            json!({"address":integer(0,u64::MAX)}),
            &["address"],
            false,
            true,
        ),
        spec(
            "xrefs_query",
            "Ghidra: references to/from a MODULE-RELATIVE offset.",
            json!({"address":integer(0,u64::MAX),"direction":{"type":"string","enum":["to","from","both"]},"limit":integer(1,5000)}),
            &["address"],
            false,
            true,
        ),
        spec(
            "runtime_doctor",
            "Inspect the configured worker's real platform, versions and available capabilities.",
            json!({}),
            &[],
            false,
            true,
        ),
        spec(
            "stage_target",
            "Copy the run's SHA256-verified original target into the worker's immutable files area.",
            json!({}),
            &[],
            true,
            true,
        ),
        spec(
            "file_put",
            "Create an immutable worker file from base64 bytes or an existing evidence artifact.",
            json!({"path":id,"data_base64":s,"evidence_id":s,"executable":b}),
            &["path"],
            true,
            true,
        ),
        spec(
            "file_get",
            "Retrieve a declared in-run worker file and persist original bytes as evidence.",
            json!({"path":id,"max_bytes":integer(1,32*1024*1024)}),
            &["path"],
            false,
            true,
        ),
        spec(
            "job_start",
            "Start a persistent process job in the configured worker; returns a job_id immediately.",
            json!({"argv":a,"cwd":s,"env":o,"timeout_secs":integer(1,86400),"output_limit":integer(1024,8*1024*1024)}),
            &["argv"],
            true,
            true,
        ),
        spec(
            "job_status",
            "Observe an in-run job, refresh persisted status and import raw observations.",
            json!({"job_id":id}),
            &["job_id"],
            false,
            true,
        ),
        spec(
            "job_cancel",
            "Request cancellation of an owned in-run job.",
            json!({"job_id":id}),
            &["job_id"],
            true,
            true,
        ),
        spec(
            "job_artifacts",
            "List and import original artifacts produced by an in-run job.",
            json!({"job_id":id}),
            &["job_id"],
            false,
            true,
        ),
        spec(
            "fuzz_start",
            "Start AFL++ QEMU fuzzing after staging target and nonempty seed files.",
            json!({"target":id,"seeds":id,"args":a,"env":o,"timeout_secs":integer(1,86400),"exec_timeout_ms":integer(10,60000),"memory_mb":integer(16,65536)}),
            &["target", "seeds"],
            true,
            true,
        ),
        spec(
            "fuzz_status",
            "Observe a bound AFL++ job and its real statistics/crash files.",
            json!({"job_id":id}),
            &["job_id"],
            false,
            true,
        ),
        spec(
            "debug_start",
            "Start an owned GDB/MI session on the staged target.",
            json!({"target":id,"args":a}),
            &["target"],
            true,
            true,
        ),
        spec(
            "debug_command",
            "Execute exactly one GDB/MI command in your owned session; stable request IDs prevent blind replay.",
            json!({"session_id":id,"command":id,"timeout_secs":integer(1,60)}),
            &["session_id", "command"],
            true,
            true,
        ),
        spec(
            "debug_close",
            "Close your owned debugger session.",
            json!({"session_id":id}),
            &["session_id"],
            true,
            true,
        ),
        spec(
            "crash_replay",
            "Start a persistent clean-directory crash reproduction job.",
            replay_properties(),
            &["target", "input"],
            true,
            true,
        ),
        spec(
            "input_minimize",
            "Start crash-preserving delta reduction using observed crash signatures.",
            replay_properties(),
            &["target", "input"],
            true,
            true,
        ),
        spec(
            "poc_verify",
            "Verifier only: independently replay a finding's input in fresh directories. Worker observations determine trial success.",
            {
                let mut p = replay_properties();
                p["finding_id"] = id.clone();
                p
            },
            &["finding_id", "target", "input"],
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
fn replay_properties() -> Value {
    json!({"target":{"type":"string","minLength":1},"input":{"type":"string","minLength":1},"args":strings(),"expected":{"type":"object","properties":{"exit_code":{"type":"integer"},"signal":{"type":"integer"},"stdout_contains":{"type":"string"},"stderr_contains":{"type":"string"}},"additionalProperties":false},"env":{"type":"object"},"timeout_secs":integer(1,60),"trials":integer(1,16),"max_attempts":integer(1,4096),"minimize_timeout_secs":integer(1,3600)})
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
    pub fn definitions(&self, role: Role) -> Vec<ToolDefinition> {
        let mut definitions = builtins()
            .into_iter()
            .filter(|item| {
                (!item.target || role != Role::Root)
                    && (!item.verifier_only || role == Role::Verifier)
                    && (!item.root_only || role == Role::Root)
                    && (!item.child_only || role != Role::Root)
            })
            .map(|item| item.definition)
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
            self.locks
                .lock()
                .await
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
                self.definitions(agent.role)
                    .iter()
                    .any(|item| item.name == name),
                "tool {name} is unavailable for role {}",
                agent.role
            );
            if let Some(spec) = builtin {
                validate_args(&args, &spec.definition.parameters, "arguments")?;
            }
            self.dispatch(&agent, call_id, name, args).await
        };
        let result = tokio::time::timeout(
            Duration::from_secs(self.config.tools.timeout_seconds),
            result,
        )
        .await;
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
            "view_agent_graph" => Ok(
                json!({"agents":self.store.list::<AgentRecord>("agent",&agent.run_id)?.iter().map(|a|json!({"id":a.id,"parent_id":a.parent_id,"name":a.name,"role":a.role,"task":a.task,"scope":a.scope,"status":a.status,"summary":a.summary,"error":a.error,"turns":a.turns,"total_tokens":a.total_tokens})).collect::<Vec<_>>()}),
            ),
            "send_message_to_agent" => self.send_message(agent, call_id, &args),
            "wait_for_agents" => {
                if let Some(value) = self.store.get::<Value>("tool_receipt", call_id)? {
                    return Ok(value);
                }
                let value = self.coordinator.wait(
                    &agent.id,
                    &string_list(&args, "agent_ids")?,
                    number(&args, "timeout_seconds", 300),
                )?;
                self.store
                    .put("tool_receipt", call_id, &agent.run_id, &value)?;
                Ok(value)
            }
            "stop_agent" => {
                let target: AgentRecord =
                    self.require_in_run("agent", text(&args, "agent_id")?, &agent.run_id)?;
                self.authorize_control(agent, &target)?;
                Ok(json!({"stopped":self.coordinator.stop(&target.id,text(&args,"reason")?)?}))
            }
            "agent_finish" | "finish_scan" => {
                ensure!(
                    self.store.inbox(&agent.id)?.is_empty(),
                    "unread mailbox messages remain; process them at a fresh model boundary before finishing"
                );
                self.coordinator
                    .finish(&agent.id, text(&args, "summary")?)?;
                Ok(json!({"finished":true,"agent_id":agent.id}))
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
                let bytes = self
                    .store
                    .read_evidence(&agent.run_id, text(&args, "evidence_id")?)?;
                let offset = usize::try_from(number(&args, "offset", 0))?;
                ensure!(offset <= bytes.len(), "evidence offset is past end of file");
                let length = usize::try_from(number(&args, "length", 8192))?
                    .min(self.config.tools.max_evidence_read_bytes);
                let end = offset.saturating_add(length).min(bytes.len());
                let part = &bytes[offset..end];
                let content = match std::str::from_utf8(part) {
                    Ok(text) => json!({"encoding":"utf8","data":text}),
                    Err(_) => json!({"encoding":"base64","data":BASE64.encode(part)}),
                };
                Ok(
                    json!({"evidence_id":args["evidence_id"],"offset":offset,"length":part.len(),"total_bytes":bytes.len(),"next_offset":if end<bytes.len(){Some(end)}else{None},"content":content}),
                )
            }
            "load_skill" => Ok(
                json!({"name":args["name"],"content":agents::load_skill(text(&args,"name")?,self.config.skills_dir.as_deref())?}),
            ),
            "binary_inspect" => {
                let run = self.target(agent)?;
                Ok(serde_json::to_value(binary::inspect(&run.target_path)?)?)
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
            | "input_minimize" | "poc_verify" => {
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

    fn create_agent(&self, agent: &AgentRecord, call_id: &str, args: &Value) -> Result<Value> {
        let request: crate::coordinator::ChildRequest = serde_json::from_value(args.clone())?;
        ensure!(
            request.role != Role::Root && request.skills.len() <= 5,
            "invalid child role or skill count"
        );
        for skill in &request.skills {
            agents::load_skill(skill, self.config.skills_dir.as_deref())?;
        }
        self.effect(agent, call_id, |tx| {
            let parent: AgentRecord =
                Store::tx_get(tx, "agent", &agent.id)?.context("parent missing")?;
            let run: RunRecord = Store::tx_get(tx, "run", &agent.run_id)?.context("run missing")?;
            ensure!(
                !parent.status.terminal() && !run.status.terminal(),
                "cannot create a child for a terminal task"
            );
            let all = Store::tx_list::<AgentRecord>(tx, "agent", &agent.run_id)?;
            if let Some(existing) = all.iter().find(|other| {
                other.role == request.role
                    && other.scope.trim() == request.scope.trim()
                    && !matches!(
                        other.status,
                        Status::Stopped | Status::Crashed | Status::Failed
                    )
            }) {
                return Ok(json!({"created":false,"agent":existing}));
            }
            ensure!(
                all.len() < run.budget.max_agents,
                "maximum total agent count reached"
            );
            ensure!(
                parent.depth < run.budget.max_depth,
                "maximum agent depth reached"
            );
            let timestamp = now();
            let child = AgentRecord {
                id: stable("agent", call_id),
                run_id: run.id.clone(),
                parent_id: Some(parent.id.clone()),
                name: request.name,
                role: request.role,
                task: request.task,
                scope: request.scope,
                skills: request.skills,
                status: Status::Queued,
                depth: parent.depth + 1,
                inherit_context: request.inherit_context,
                inherited_context: if request.inherit_context {
                    parent.checkpoint.clone()
                } else {
                    None
                },
                checkpoint: None,
                wait_for: vec![],
                wait_until: None,
                turns: 0,
                total_tokens: 0,
                cost_usd: 0.0,
                summary: None,
                error: None,
                created_at: timestamp,
                updated_at: timestamp,
            };
            Store::tx_put(tx, "agent", &child.id, &agent.run_id, &child)?;
            Store::tx_event(
                tx,
                &agent.run_id,
                Some(&child.id),
                "agent.created",
                json!({"parent_id":agent.id,"role":child.role,"scope":child.scope}),
            )?;
            Ok(json!({"created":true,"agent":child}))
        })
    }
    fn send_message(&self, agent: &AgentRecord, call_id: &str, args: &Value) -> Result<Value> {
        self.effect(agent, call_id, |tx| {
            let recipient = text(args, "agent_id")?;
            let mut target: AgentRecord =
                Store::tx_get(tx, "agent", recipient)?.context("recipient missing")?;
            ensure!(
                target.run_id == agent.run_id && !target.status.terminal(),
                "recipient is terminal or belongs to another run"
            );
            Store::tx_message(
                tx,
                &agent.run_id,
                &agent.id,
                recipient,
                text(args, "message")?,
                Some(call_id),
            )?;
            if target.status == Status::Waiting {
                target.status = Status::Queued;
                target.wait_for.clear();
                target.wait_until = None;
                target.updated_at = now();
                Store::tx_put(tx, "agent", recipient, &agent.run_id, &target)?;
            }
            Store::tx_event(
                tx,
                &agent.run_id,
                Some(recipient),
                "message.received",
                json!({"sender":agent.id}),
            )?;
            Ok(json!({"delivered":true,"agent_id":recipient}))
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
                limit: number(args, "limit", 100) as usize,
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
            let result=self.worker_call(agent,call_id,"ghidra.query",json!({"target":staged["path"],"target_sha256":run.target_sha256,"query":query,"timeout_secs":self.config.analysis.timeout_seconds})).await?;
            ensure!(
                result["target_sha256"] == run.target_sha256,
                "remote Ghidra analyzed another target"
            );
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
                ensure!(
                    session["agent_id"] == agent.id
                        && session["worker_id"] == self.worker_identity()?,
                    "debug session is owned by another agent or runtime"
                );
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
            "job_start" | "fuzz_start" | "crash_replay" | "input_minimize" | "poc_verify" => {
                let run: RunRecord = self.store.require("run", &agent.run_id)?;
                if name != "job_start" {
                    self.known_file(agent, text(&args, "target")?, true)?;
                    if name != "fuzz_start" {
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
                    _ => "poc.verify",
                };
                let raw = self
                    .worker_call(agent, call_id, method, args.clone())
                    .await?;
                let job_id = raw["job_id"].as_str().context("worker job ID missing")?;
                let record = json!({"id":job_id,"run_id":agent.run_id,"agent_id":agent.id,"worker_id":self.worker_identity()?,"tool_name":name,"finding_id":finding_id,"call_id":call_id,"worker_request_id":self.request_identity(agent,call_id),"parameters":args,"target_sha256":run.target_sha256,"status":raw,"created_at":now(),"updated_at":now()});
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
    format!("{prefix}_{}", hex::encode(Sha256::digest(value.as_bytes())))
}
fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .with_context(|| format!("{key} must be a nonempty string"))
}
fn number(args: &Value, key: &str, default: u64) -> u64 {
    match &args[key] {
        Value::Number(value) => value.as_u64().unwrap_or(default),
        Value::String(text) => {
            let text = text.trim().replace('_', "");
            let parsed =
                if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
                    u64::from_str_radix(hex, 16)
                } else {
                    text.parse::<u64>()
                };
            parsed.unwrap_or(default)
        }
        _ => default,
    }
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

fn validate_args(value: &Value, definition: &Value, path: &str) -> Result<()> {
    if let Some(kind) = definition["type"].as_str() {
        let valid = match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            _ => true,
        };
        ensure!(valid, "{path} must have type {kind}");
    }
    if let Some(choices) = definition["enum"].as_array() {
        ensure!(choices.contains(value), "{path} is not an allowed value");
    }
    if let Some(text) = value.as_str()
        && let Some(min) = definition["minLength"].as_u64()
    {
        ensure!(text.chars().count() as u64 >= min, "{path} is too short");
    }
    if let Some(number) = value.as_f64() {
        if let Some(min) = definition["minimum"].as_f64() {
            ensure!(number >= min, "{path} is below its minimum");
        }
        if let Some(max) = definition["maximum"].as_f64() {
            ensure!(number <= max, "{path} exceeds its maximum");
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = definition["required"].as_array() {
            for key in required {
                let key = key.as_str().context("invalid built-in schema")?;
                ensure!(object.contains_key(key), "missing required {path}.{key}");
            }
        }
        for (key, value) in object {
            if let Some(property) = definition["properties"].get(key) {
                validate_args(value, property, &format!("{path}.{key}"))?;
            } else {
                ensure!(
                    definition["additionalProperties"] != false,
                    "unknown argument {path}.{key}"
                );
            }
        }
    }
    if let Some(array) = value.as_array()
        && let Some(items) = definition.get("items")
    {
        for (index, item) in array.iter().enumerate() {
            validate_args(item, items, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}
