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

mod analysis;
mod args;
mod catalog;
mod coordination;
mod discovery;
mod dispatch;
mod execution;
mod files;
mod jobs;
mod sessions;
use catalog::builtins;
pub(crate) use catalog::child_schema;

/// Machine-readable schemas from the same registry used for model requests.
pub fn builtin_tool_definitions() -> Vec<ToolDefinition> {
    builtins()
        .iter()
        .filter(|spec| spec.definition.name.as_str() != "create_agent")
        .map(|spec| spec.definition.clone())
        .collect()
}

/// Generated documentation deliberately excludes runtime-configured MCP tools.
pub fn builtin_catalog_markdown() -> String {
    let mut output = String::from(
        "# 内置工具目录\n\n由 `re-agent tool-catalog` 从工具注册表生成。不要手动编辑。\n\n原生协调工具 `task`、`message`、`wait` 由 Rig 提供；MCP 工具取决于配置。下表列出模型可见的内置业务工具及生命周期可见性，具体执行仍校验目标、证据、会话归属和后端能力。\n\n| 工具 | 可见角色 | 有副作用 | 用途 |\n|---|---|---|---|\n",
    );
    let mut specs = builtins()
        .iter()
        .filter(|spec| spec.definition.name.as_str() != "create_agent")
        .collect::<Vec<_>>();
    specs.sort_by_key(|spec| spec.definition.name.as_str());
    for spec in specs {
        let roles = if spec.root_only {
            "root"
        } else if spec.child_only {
            "analyst / verifier"
        } else {
            "全部"
        };
        let description = spec
            .definition
            .description
            .replace('|', "\\|")
            .replace('\n', " ");
        output.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            spec.definition.name,
            roles,
            if spec.side_effecting { "是" } else { "否" },
            description
        ));
    }
    output
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
                    (!item.root_only || role == Role::Root)
                        && (!item.child_only || role != Role::Root)
                },
            )
    }

    pub fn definitions(&self, role: Role) -> Vec<ToolDefinition> {
        let mut definitions = builtins()
            .iter()
            .filter(|item| {
                (!item.root_only || role == Role::Root) && (!item.child_only || role != Role::Root)
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
        let agent: AgentRecord = self.store.require_metadata("agent", agent_id)?;
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
    pub(crate) fn runtime(&self) -> Result<&RuntimeClient> {
        self.runtime.as_ref().context(
            "execution runtime is disabled; configure Docker/SSH or explicitly trusted_local",
        )
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
    } else if let Some(array) = value.as_array_mut()
        && let Some(items) = schema.get("items")
    {
        for value in array {
            normalize_integer_arguments(value, items);
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
