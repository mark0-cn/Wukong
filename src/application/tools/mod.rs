//! Role-aware tool dispatch with durable effect receipts and original evidence.
use crate::{
    adapters::binary::{self, GhidraBackend, GhidraConfig, GhidraQuery, XrefDirection},
    adapters::mcp::McpRegistry,
    adapters::worker::RuntimeClient,
    application::agents,
    application::coordinator::Coordinator,
    config::AppConfig,
    domain::*,
    storage::{DocumentKind, SessionKind, Store, StoredRecord},
};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rig::{
    completion::{ToolAnnotations, ToolDefinition},
    message::ToolName,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use wukong_worker_protocol::{Method, Request};

use crate::application::effects::{Admission, DurableEffects, ReplayPolicy};
pub use crate::application::effects::{EffectOutcome as ToolOutcome, reconcile_record};
#[derive(Clone)]
pub struct ToolService {
    pub store: Store,
    pub coordinator: Coordinator,
    limits: crate::config::ToolLimits,
    skills_dir: Option<PathBuf>,
    analysis_config: crate::config::StaticConfig,
    mcp: McpRegistry,
    locks: Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
    environments: crate::adapters::environments::EnvironmentRegistry,
    static_environment: Option<String>,
    windows_lab: crate::config::WindowsLabConfig,
    lab_operations: lab_operations::Operations,
    default_environment: Option<String>,
    sync_limits: evidence_sync::SyncLimits,
}

mod analysis;
mod analysis_verification;
mod args;
mod artifact_validation;
mod bundle;
mod catalog;
mod coordination;
mod discovery;
mod documents;
mod environment_service;
mod evidence;
pub(crate) use discovery::ToolSelection;
mod context;
mod execution;
mod gateway;
mod lab;
mod lab_operations;
use context::ToolContext;
pub(crate) use lab::LabService;
mod requirements;
use analysis::AnalysisService;
use execution::ExecutionService;
use files::FilesService;
use gateway::WorkerGateway;
use sessions::SessionsService;
type ResourceLocks = Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>;
mod evidence_sync;
mod files;
mod handlers;
mod jobs;
mod observation;
mod resource_recovery;
mod sessions;
mod transfers;
mod verification;
mod verification_protocol;
use catalog::builtins;
pub(crate) use catalog::child_schema;

/// Current builtin display labels share the execution registry.
pub fn tool_display_title(name: &str) -> Option<&'static str> {
    builtins()
        .iter()
        .find(|item| item.definition.name.as_str() == name)
        .map(|item| item.display_title)
}

/// Machine-readable schemas from the same registry used for model requests.
pub fn builtin_tool_definitions() -> Vec<ToolDefinition> {
    builtins()
        .iter()
        .filter(|spec| spec.model_visible)
        .map(|spec| spec.definition.clone())
        .collect()
}

mod documentation;
pub use documentation::{builtin_catalog_markdown, builtin_catalog_markdown_for_locale};

impl ToolService {
    pub(crate) fn effects(&self) -> DurableEffects<'_> {
        DurableEffects::new(&self.store, self.limits.max_output_bytes)
    }
    pub(crate) fn with_store(&self, store: Store) -> Self {
        Self {
            store: store.clone(),
            coordinator: Coordinator::new(store),
            ..self.clone()
        }
    }
    pub(crate) fn from_dependencies(
        config: AppConfig,
        store: Store,
        mcp: McpRegistry,
    ) -> Result<Self> {
        let environments =
            crate::adapters::environments::EnvironmentRegistry::new(config.environments.clone())?
                .with_lab_guards(&config.windows_lab)?;
        let locks = Arc::new(Mutex::new(BTreeMap::new()));
        let static_environment = config.analysis.static_environment.clone();
        let windows_lab = config.windows_lab.clone();
        Ok(Self {
            store: store.clone(),
            coordinator: Coordinator::new(store),
            limits: config.tools,
            skills_dir: config.skills_dir,
            analysis_config: config.analysis,
            mcp,
            locks,
            environments,
            static_environment,
            windows_lab,
            lab_operations: Default::default(),
            default_environment: config.default_environment,
            sync_limits: Default::default(),
        })
    }
    pub(crate) async fn bound_to(&self, agent: &AgentMetadata) -> Result<ToolContext> {
        let Some(environment_id) = &agent.environment_id else {
            return Ok(ToolContext::new(self, None));
        };
        self.checked_environment(agent).await?;
        let client = self
            .environments
            .bind(
                environment_id,
                &agent.run_id,
                self.store.controller_owner().as_deref(),
            )
            .await?;
        let primary = gateway::BoundWorker::new(client, environment_id)?;
        Ok(ToolContext::new(self, Some(primary)))
    }

    async fn bound_for(
        &self,
        agent: &AgentMetadata,
        requirement: requirements::Requirement,
    ) -> Result<ToolContext> {
        if matches!(requirement, requirements::Requirement::Ghidra)
            && let Some(environment) = &self.static_environment
        {
            let run: RunRecord = self.store.load(&agent.run_id)?;
            ensure!(
                run.config["analysis"]["static_environment"].as_str() == Some(environment.as_str()),
                "static backend selection changed since assessment creation"
            );
            environment_service::EnvironmentService {
                store: self.store.clone(),
                registry: self.environments.clone(),
            }
            .checked_id(agent, environment)
            .await?;
            let client = self
                .environments
                .bind(
                    environment,
                    &agent.run_id,
                    self.store.controller_owner().as_deref(),
                )
                .await?;
            return Ok(ToolContext::with_static(
                self,
                None,
                Some(gateway::BoundWorker::new(client, environment)?),
            ));
        }
        self.bound_to(agent).await
    }

    async fn checked_environment(&self, agent: &AgentMetadata) -> Result<RuntimeClient> {
        environment_service::EnvironmentService {
            store: self.store.clone(),
            registry: self.environments.clone(),
        }
        .checked(agent)
        .await
    }

    pub(crate) async fn renew_environment_leases(&self, run: &str) -> Result<()> {
        if let Some(owner) = self.store.controller_owner() {
            self.environments.renew_leases(run, &owner).await?;
        }
        Ok(())
    }
    pub(crate) fn allows(&self, role: Role, name: &str) -> bool {
        builtins()
            .iter()
            .find(|item| item.definition.name.as_str() == name)
            .map_or_else(
                || role != Role::Root && self.mcp.contains(name),
                |item| item.allows(role),
            )
    }

    pub(crate) fn model_allows(&self, role: Role, name: &str) -> bool {
        self.allows(role, name)
            && builtins()
                .iter()
                .find(|item| item.definition.name.as_str() == name)
                .is_none_or(|item| item.model_visible)
    }

    pub fn definitions(&self, role: Role) -> Vec<ToolDefinition> {
        let mut definitions = builtins()
            .iter()
            .filter(|item| item.model_visible && item.allows(role))
            .map(|item| item.definition.clone())
            .collect::<Vec<_>>();
        if role != Role::Root {
            definitions.extend(self.mcp.definitions());
        }
        definitions
    }
    pub async fn close(&self) {
        for cancellation in self.lab_operations.lock().await.values() {
            cancellation.cancel();
        }
        let drained = tokio::time::timeout(Duration::from_secs(10), async {
            while !self.lab_operations.lock().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if drained.is_err() {
            tracing::warn!(
                "lab operations did not finish reconciliation before controller shutdown"
            );
        }
        self.mcp.close().await;
        self.environments.close().await;
    }
    pub async fn doctor(&self) -> Result<Value> {
        if let Some(environment) = &self.default_environment {
            Ok(self.environments.doctor(environment).await?["observation"].clone())
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
        self.effects().replace_result(call_id, value, is_error)
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
        let agent: AgentMetadata = self.store.load::<AgentMetadata>(agent_id)?;
        let specs = builtins();
        let builtin = specs.iter().find(|item| item.definition.name == name);
        let policy = builtin.map(|spec| spec.policy).unwrap_or_else(|| {
            if self.mcp.side_effecting(name) {
                ReplayPolicy::OperatorOnly
            } else {
                ReplayPolicy::ReadOnly
            }
        });
        let record = match self.effects().admit(&agent, call_id, name, &args, policy)? {
            Admission::Replay(outcome) => return Ok(outcome),
            Admission::Execute(record) => record,
        };
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
            Duration::from_secs(self.limits.timeout_seconds)
        };
        let result = async {
            ensure!(args.is_object(), "tool arguments must be a JSON object");
            ensure!(
                !agent.status.terminal(),
                "a terminal agent cannot start a new tool effect"
            );
            let run: RunRecord = self.store.load(&agent.run_id)?;
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
            if let Some(spec) = builtin {
                if self.needs_worker(spec) {
                    let scoped = self.bound_for(&agent, spec.requirement).await?;
                    scoped.require_backend(spec, &args).await?;
                    (spec.handler)(&scoped, &agent, call_id, name, args).await
                } else {
                    let scoped = ToolContext::new(self, None);
                    scoped.require_backend(spec, &args).await?;
                    (spec.handler)(&scoped, &agent, call_id, name, args).await
                }
            } else if self.mcp.contains(name) {
                self.mcp.call(name, args).await
            } else {
                bail!("unknown tool {name}")
            }
        };
        let result = tokio::time::timeout(outer_timeout, result).await;
        match result {
            Ok(Ok(value)) => {
                let is_error = self.mcp.contains(name) && value["isError"].as_bool() == Some(true);
                self.finish_raw(record, value, is_error, if is_error {CallState::Failed} else {CallState::Completed})
            }
            Ok(Err(error)) => {
                let state = if error.is::<crate::application::windows_lab::LabError>() { CallState::Uncertain } else { self.effects().failure_state(policy, &agent, call_id, &error)? };
                let kind = error.downcast_ref::<crate::adapters::worker::protocol::WorkerError>().map(|e| json!(e.kind));
                let lab_error = error.downcast_ref::<crate::application::windows_lab::LabError>();
                self.finish_raw(record, json!({"error":format!("{error:#}"),"kind":kind,"lab_error":lab_error,
                    "effect_state":if state == CallState::Uncertain {"unknown"} else {"rejected"},
                    "next_action":if state == CallState::Uncertain {"reconcile the existing request; do not change arguments or issue a replacement write"} else {"correct the indicated input; preserve returned references and completed results"}}), true, state)
            }
            Err(_) => self.finish_raw(
                record,
                json!({"error":"tool timed out; reconcile the durable request before retrying","call_id":call_id}),
                true,
                policy.interrupted_state(),
            ),
        }
    }

    fn finish_raw(
        &self,
        record: ToolCallRecord,
        raw: Value,
        is_error: bool,
        state: CallState,
    ) -> Result<ToolOutcome> {
        self.effects().finish(record, raw, is_error, state)
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

    fn coordination_service(&self) -> coordination::CoordinationService {
        coordination::CoordinationService {
            store: self.store.clone(),
            skills_dir: self.skills_dir.clone(),
            output_limit: self.limits.max_output_bytes,
        }
    }
    pub(crate) fn create_agent(
        &self,
        agent: &AgentMetadata,
        call: &str,
        args: &Value,
    ) -> Result<Value> {
        self.coordination_service().create_agent(agent, call, args)
    }
}

/// Replace the `{target}` placeholder in a job_start argv with the staged
/// target worker path, so scripts receive it as a plain argv element (the
/// bridge stages such paths in isolated jobs) instead of guessing the worker
/// layout.
fn substitute_target_argv(args: &Value, target_sha256: &str) -> Value {
    let staged = format!("files/targets/{target_sha256}/sample.bin");
    let mut params = args.clone();
    for key in ["argv", "executable_inputs"] {
        if let Some(argv) = params.get_mut(key).and_then(Value::as_array_mut) {
            for entry in argv.iter_mut() {
                if let Some(text) = entry.as_str() {
                    *entry = json!(text.replace("{target}", &staged));
                }
            }
        }
    }
    params
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

fn require_in_run<T: StoredRecord + RunScoped>(store: &Store, id: &str, run_id: &str) -> Result<T> {
    let record: T = store.load(id)?;
    ensure!(
        record.run_id() == run_id,
        "{} belongs to another run",
        T::KIND
    );
    Ok(record)
}
fn require_document(
    store: &Store,
    kind: DocumentKind,
    id: &str,
    run: &str,
) -> Result<SharedDocument> {
    let document = store.document(kind, id)?;
    ensure!(document.run_id == run, "document belongs to another run");
    Ok(document)
}
fn require_session(store: &Store, kind: SessionKind, id: &str, run: &str) -> Result<SessionRecord> {
    let session = store.session(kind, id)?;
    ensure!(session.run_id == run, "session belongs to another run");
    Ok(session)
}
fn target(store: &Store, agent: &AgentMetadata) -> Result<RunRecord> {
    ensure!(
        agent.role != Role::Root,
        "root cannot operate on the target"
    );
    let run: RunRecord = store.load(&agent.run_id)?;
    ensure!(
        binary::sha256_file(&run.target_path)? == run.target_sha256,
        "target SHA256 changed since run creation"
    );
    Ok(run)
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
