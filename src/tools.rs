//! Role-aware tool dispatch with durable effect receipts and original evidence.
use crate::{
    agents,
    binary::{self, GhidraBackend, GhidraConfig, GhidraQuery, StringsQuery, XrefDirection},
    config::AppConfig,
    coordinator::Coordinator,
    domain::*,
    mcp::McpRegistry,
    runtime::RuntimeClient,
    store::{DocumentKind, SessionKind, Store, StoredRecord},
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

use crate::effects::{Admission, DurableEffects, ReplayPolicy};
pub use crate::effects::{EffectOutcome as ToolOutcome, reconcile_record};
#[derive(Clone)]
pub struct ToolService {
    pub store: Store,
    pub coordinator: Coordinator,
    limits: crate::config::ToolLimits,
    skills_dir: Option<PathBuf>,
    execution: ExecutionService,
    analysis: AnalysisService,
    mcp: McpRegistry,
    locks: Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
    environments: crate::environments::EnvironmentRegistry,
    default_environment: Option<String>,
}

mod analysis;
mod args;
mod catalog;
mod coordination;
mod discovery;
mod execution;
mod gateway;
use analysis::AnalysisService;
use execution::ExecutionService;
use files::FilesService;
use gateway::WorkerGateway;
use sessions::SessionsService;
type ResourceLocks = Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>;
mod files;
mod handlers;
mod jobs;
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
        let gateway = WorkerGateway {
            store: store.clone(),
            ..self.execution.gateway.clone()
        };
        let execution = ExecutionService::new(store.clone(), gateway.clone(), self.locks.clone());
        let analysis = AnalysisService {
            store: store.clone(),
            files: execution.files.clone(),
            gateway,
            config: self.analysis.config.clone(),
        };
        Self {
            store: store.clone(),
            coordinator: Coordinator::new(store),
            execution,
            analysis,
            ..self.clone()
        }
    }
    pub async fn new(config: AppConfig, store: Store) -> Result<Self> {
        crate::bootstrap::build_tools(config, store).await
    }
    pub(crate) fn from_dependencies(
        config: AppConfig,
        store: Store,
        mcp: McpRegistry,
    ) -> Result<Self> {
        let environments =
            crate::environments::EnvironmentRegistry::new(config.environments.clone())?;
        let locks = Arc::new(Mutex::new(BTreeMap::new()));
        let gateway = WorkerGateway {
            store: store.clone(),
            client: None,
            identity: "unbound".into(),
            output_limit: config.tools.max_output_bytes,
        };
        let execution = ExecutionService::new(store.clone(), gateway.clone(), locks.clone());
        let analysis = AnalysisService {
            store: store.clone(),
            files: execution.files.clone(),
            gateway,
            config: config.analysis,
        };
        Ok(Self {
            store: store.clone(),
            coordinator: Coordinator::new(store),
            limits: config.tools,
            skills_dir: config.skills_dir,
            execution,
            analysis,
            mcp,
            locks,
            environments,
            default_environment: config.default_environment,
        })
    }
    pub(crate) async fn bound_to(&self, agent: &AgentMetadata) -> Result<Self> {
        let Some(environment_id) = &agent.environment_id else {
            return Ok(self.clone());
        };
        let config = self.environments.config(environment_id)?;
        let run: RunRecord = self.store.load(&agent.run_id)?;
        ensure!(
            run.config["environments"][environment_id] == serde_json::to_value(config)?,
            "environment configuration changed since this assessment was created"
        );
        let client = self.environments.client(environment_id).await?;
        self.store.bind_environment(&EnvironmentBinding {
            id: stable(
                "environment-binding",
                &format!("{}:{environment_id}", agent.run_id),
            ),
            run_id: agent.run_id.clone(),
            environment_id: environment_id.clone(),
            identity: client.identity().clone(),
        })?;
        let client = self
            .environments
            .bind(
                environment_id,
                &agent.run_id,
                self.store.controller_owner().as_deref(),
            )
            .await?;
        let identity = stable("worker", &serde_json::to_string(client.identity())?);
        let gateway = WorkerGateway {
            store: self.store.clone(),
            client: Some(client),
            identity,
            output_limit: self.limits.max_output_bytes,
        };
        let execution =
            ExecutionService::new(self.store.clone(), gateway.clone(), self.locks.clone());
        let analysis = AnalysisService {
            store: self.store.clone(),
            files: execution.files.clone(),
            gateway,
            config: self.analysis.config.clone(),
        };
        Ok(Self {
            execution,
            analysis,
            ..self.clone()
        })
    }

    pub(crate) async fn refresh_jobs(&self, run: &str) -> Result<()> {
        for job in self.store.records::<JobRecord>(run)? {
            if job.notification_message_id.is_some() {
                continue;
            }
            let owner: AgentMetadata = require_in_run(&self.store, &job.agent_id, run)?;
            let tools = self.bound_to(&owner).await?;
            tools
                .execution
                .jobs
                .observe(&owner, &format!("job-observation:{}", job.id), &job.id)
                .await?;
        }
        Ok(())
    }
    pub(crate) async fn renew_environment_leases(&self, run: &str) -> Result<()> {
        if let Some(owner) = self.store.controller_owner() {
            self.environments.renew_leases(run, &owner).await?;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn jobs(&self) -> &jobs::JobService {
        &self.execution.jobs
    }
    #[cfg(test)]
    pub(crate) fn worker_identity(&self) -> Result<String> {
        self.execution.gateway.worker_identity()
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
        self.mcp.close().await;
        self.environments.close().await;
        if let Some(runtime) = &self.execution.gateway.client {
            let _ = runtime.close().await;
        }
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
                if spec.worker_bound {
                    let scoped = self.bound_to(&agent).await?;
                    (spec.handler)(&scoped, &agent, call_id, name, args).await
                } else {
                    (spec.handler)(self, &agent, call_id, name, args).await
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
                let state = self.effects().failure_state(policy, &agent, call_id, &error)?;
                self.finish_raw(record, json!({"error":format!("{error:#}")}), true, state)
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

    fn effect(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        action: impl FnOnce(&crate::store::StateSession<'_>) -> Result<Value>,
    ) -> Result<Value> {
        self.effects().commit(agent, call_id, action)
    }

    #[cfg(test)]
    pub(crate) fn runtime(&self) -> Result<&RuntimeClient> {
        self.execution.gateway.runtime()
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
