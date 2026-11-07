//! Rig's serializable agent state machine, driven one durable effect at a time.
//!
//! A checkpoint owns conversation and provider-native response state, never a
//! model client or credential. Persist after `next_step`, after preparation,
//! and after every answer. Tool execution belongs to the host coordinator.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::Duration,
};

use crate::domain::Role;
use anyhow::{Context, Result, bail, ensure};
use rig::{
    DynModel,
    completion::{CompletionRequest, Message, ToolDefinition, Usage},
    operation::Completion,
    providers::{anthropic::AnthropicConfig, ollama::OllamaConfig, openai::OpenAIConfig},
    run::{
        AgentRun, InvalidToolCallAction, ModelTurn, ModelTurnOutcome, PreparedRequest, RunSpec,
        prepare_request,
    },
    tool::{ToolExecutionError, ToolOutput, ToolResult},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use rig::run::{AgentRunStep, ExecCall, PendingToolCall};

pub const RIG_STATE_VERSION: &str = "0.46.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// OpenAI-compatible Chat Completions (`/chat/completions`).
    #[default]
    #[serde(rename = "openai", alias = "open_ai")]
    OpenAi,
    /// OpenAI Responses API, including native encrypted reasoning replay.
    #[serde(rename = "openai_responses", alias = "open_ai_responses")]
    OpenAiResponses,
    Anthropic,
    /// Ollama's native `/api/chat`, including its native thinking content.
    Ollama,
}

/// Per-role model settings: the model name plus an optional per-request
/// output budget. Provider, endpoint and credentials are inherited from the
/// root settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleModelSpec {
    pub model: String,
    #[serde(default)]
    pub max_tokens: Option<u64>,
}

/// Persistable model settings. `api_key_env` is a variable name, not a secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub provider: Provider,
    pub model: String,
    /// Model name used by role (non-root) agents. Provider, endpoint and
    /// credentials are inherited from the root settings above; only the model
    /// name is swapped. When unset, role agents share the root model.
    pub role_model: Option<String>,
    /// Per-role overrides; a role present here wins over `role_model`.
    /// Provider, endpoint and credentials are still inherited from the root
    /// settings. A role mapped to the root model name runs with the root
    /// settings except its explicit `max_tokens`.
    #[serde(default)]
    pub role_models: Option<HashMap<Role, RoleModelSpec>>,
    /// `max_tokens` override for role agents with an explicit resolution
    /// (role_models or role_model) that does not carry its own max_tokens,
    /// even when they resolve to the root model name. Verbose role models
    /// (such as deepseek-flash) can exhaust the root model's output budget
    /// and stop with finish_reason=Length; raise it here. Unmapped roles
    /// keep the root `max_tokens`.
    pub role_max_tokens: Option<u64>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub max_tokens: u64,
    pub temperature: Option<f64>,
    pub request_timeout_secs: u64,
    pub additional_params: Option<Value>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: Provider::OpenAi,
            model: "gpt-5.2".into(),
            role_model: None,
            role_models: None,
            role_max_tokens: None,
            base_url: None,
            api_key_env: None,
            max_tokens: 8192,
            temperature: None,
            request_timeout_secs: 180,
            additional_params: None,
        }
    }
}

impl ModelConfig {
    /// The model name a role (non-root) agent resolves to: the per-role map
    /// wins, then the legacy `role_model`, then the root model name.
    pub fn role_model_name(&self, role: Role) -> Option<&str> {
        if role == Role::Root {
            return None;
        }
        if let Some(spec) = self.role_models.as_ref().and_then(|map| map.get(&role)) {
            return Some(spec.model.as_str());
        }
        self.role_model.as_deref()
    }

    /// The model configuration for a role (non-root) agent: the root settings
    /// with the model name swapped when a role model is configured. A role
    /// with an explicit resolution (per-role map or `role_model`) receives
    /// its spec's `max_tokens` or the global `role_max_tokens` when set,
    /// even when it resolves to the root model name; roles without a
    /// resolution keep the root `max_tokens`.
    pub fn role_config_for(&self, role: Role) -> ModelConfig {
        let mut config = self.clone();
        config.role_model = None;
        config.role_models = None;
        if role == Role::Root {
            return config;
        }
        let explicit = self.role_models.as_ref().and_then(|map| map.get(&role));
        let name = explicit
            .map(|spec| spec.model.as_str())
            .or(self.role_model.as_deref());
        if let Some(name) = name {
            config.model = name.to_owned();
            let max_tokens = explicit
                .and_then(|spec| spec.max_tokens)
                .or(self.role_max_tokens);
            if let Some(max_tokens) = max_tokens {
                config.max_tokens = max_tokens;
            }
        }
        config
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(!self.model.trim().is_empty(), "model must not be empty");
        if let Some(role_model) = &self.role_model {
            ensure!(
                !role_model.trim().is_empty(),
                "role_model must not be empty when set"
            );
        }
        if let Some(role_models) = &self.role_models {
            for spec in role_models.values() {
                ensure!(
                    !spec.model.trim().is_empty(),
                    "role_models model must not be empty"
                );
                if let Some(max_tokens) = spec.max_tokens {
                    ensure!(max_tokens > 0, "role_models max_tokens must be positive");
                }
            }
        }
        if let Some(max_tokens) = self.role_max_tokens {
            ensure!(max_tokens > 0, "role_max_tokens must be positive");
        }
        ensure!(self.max_tokens > 0, "max_tokens must be positive");
        ensure!(
            self.request_timeout_secs > 0,
            "model timeout must be positive"
        );
        if let Some(temperature) = self.temperature {
            ensure!(
                temperature.is_finite() && temperature >= 0.0,
                "invalid temperature"
            );
        }
        if let Some(url) = &self.base_url {
            let parsed =
                rig::rig_reqwest::reqwest::Url::parse(url).context("invalid model base_url")?;
            ensure!(
                matches!(parsed.scheme(), "http" | "https"),
                "model base_url must use HTTP(S)"
            );
            ensure!(
                parsed.username().is_empty() && parsed.password().is_none(),
                "put model credentials in the API key environment variable, not the URL"
            );
            ensure!(
                parsed.query().is_none() && parsed.fragment().is_none(),
                "model base_url must not contain query parameters or a fragment"
            );
        }
        if let Some(parameters) = &self.additional_params {
            ensure!(
                parameters.is_object(),
                "additional_params must be a JSON object"
            );
            reject_credentials(parameters)?;
        }
        Ok(())
    }

    fn key_environment(&self) -> &str {
        self.api_key_env.as_deref().unwrap_or(match self.provider {
            Provider::OpenAi | Provider::OpenAiResponses => "OPENAI_API_KEY",
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::Ollama => "OLLAMA_API_KEY",
        })
    }

    fn resolved_base_url(&self) -> Result<String> {
        let (variable, default) = match self.provider {
            Provider::OpenAi | Provider::OpenAiResponses => {
                ("OPENAI_BASE_URL", "https://api.openai.com/v1")
            }
            Provider::Anthropic => ("ANTHROPIC_BASE_URL", "https://api.anthropic.com"),
            Provider::Ollama => ("OLLAMA_API_BASE_URL", "http://localhost:11434"),
        };
        Ok(self
            .base_url
            .clone()
            .or_else(|| std::env::var(variable).ok())
            .unwrap_or_else(|| default.into())
            .trim_end_matches('/')
            .to_owned())
    }
}

fn reject_credentials(value: &Value) -> Result<()> {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let key = key.to_ascii_lowercase().replace('-', "_");
                ensure!(
                    !matches!(
                        key.as_str(),
                        "api_key"
                            | "apikey"
                            | "authorization"
                            | "password"
                            | "access_token"
                            | "secret"
                    ) && !["_api_key", "_password", "_secret", "_token", "_credential"]
                        .iter()
                        .any(|suffix| key.ends_with(suffix)),
                    "credentials must be supplied via environment variables"
                );
                reject_credentials(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_credentials(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelIdentity {
    pub provider: Provider,
    pub model: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PreparedAttempt {
    turn: usize,
    prompt: Message,
    request: PreparedRequest,
}

/// Same-version durable envelope around Rig's native sans-I/O state machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurableRun {
    pub rig_version: String,
    pub model: ModelIdentity,
    pub spec: RunSpec,
    pub tools: Vec<Arc<ToolDefinition>>,
    pub run: AgentRun,
    #[serde(default)]
    notes: Vec<String>,
    #[serde(default)]
    pending_request: Option<PreparedAttempt>,
    #[serde(default)]
    compacted_usage: Usage,
    #[serde(default)]
    compacted_turns: usize,
    #[serde(default)]
    pub compactions: Vec<CompactionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionRecord {
    pub before_turn: usize,
    pub removed_messages: usize,
    pub summary: String,
    pub usage_before: Usage,
}

impl DurableRun {
    pub fn next_step(&mut self) -> Result<AgentRunStep> {
        self.validate_version()?;
        // Recovery after a checkpoint in invalid-call resolution still uses
        // Rig's own bounded retry policy and canonical recovery transcript.
        self.resolve_invalid_calls()?;
        let mut step = self.run.next_step().context("advance Rig agent run")?;
        if let AgentRunStep::Done(response) = &mut step {
            response.usage += self.compacted_usage;
        }
        Ok(step)
    }

    fn validate_version(&self) -> Result<()> {
        ensure!(
            self.rig_version == RIG_STATE_VERSION,
            "checkpoint uses Rig {}, this binary uses {}; cross-version state needs an explicit migration",
            self.rig_version,
            RIG_STATE_VERSION
        );
        Ok(())
    }

    fn resolve_invalid_calls(&mut self) -> Result<()> {
        while let Some(context) = self.run.pending_invalid_tool_call() {
            self.run
                .resolve_invalid_tool_call(InvalidToolCallAction::retry(format!(
                    "Tool {:?} is unavailable. Call only one of these exact tools: {}.",
                    context.tool_name,
                    context.allowed_tools.join(", ")
                )))?;
        }
        Ok(())
    }

    /// Mailbox messages remain in durable context and are applied only when
    /// preparing a fresh model attempt. A re-issued attempt uses its snapshot.
    pub fn queue_note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }

    pub fn usage(&self) -> Usage {
        self.compacted_usage + self.run.usage()
    }

    pub fn turn(&self) -> usize {
        self.compacted_turns + self.run.turn()
    }

    pub fn is_done(&self) -> bool {
        self.run.is_done()
    }

    pub fn has_prepared_model_call(&self) -> bool {
        self.pending_request.is_some()
    }

    pub fn history(&self) -> Vec<Message> {
        let mut history = self.run.canonical_history();
        for note in &self.notes {
            history.push(Message::user(format!("Coordinator message: {note}")));
        }
        history
    }

    /// Commit queued mail to the conversation at a fresh model boundary.
    /// It can then be compacted like other input instead of growing forever
    /// in the system preamble. Frozen requests keep their exact old payload.
    fn flush_notes_before_model(&mut self) -> Result<Option<AgentRunStep>> {
        if self.notes.is_empty() || self.pending_request.is_some() {
            return Ok(None);
        }
        let AgentRunStep::CallModel {
            prompt, history, ..
        } = self.next_step()?
        else {
            bail!("coordinator mail requires a model boundary");
        };
        let mut messages: Vec<Message> = history.iter().map(|m| Message::clone(m)).collect();
        messages.push(Message::clone(&prompt));
        messages.extend(
            self.notes
                .iter()
                .map(|note| Message::user(format!("Coordinator message (data): {note}"))),
        );
        let step = self.restart_at_model_boundary(messages)?;
        self.notes.clear();
        Ok(Some(step))
    }

    fn restart_at_model_boundary(&mut self, mut messages: Vec<Message>) -> Result<AgentRunStep> {
        rig::conversation::pairing::validate(&messages)?;
        let turns = self.compacted_turns + self.run.turn().saturating_sub(1);
        let remaining = self.spec.effective_max_turns().saturating_sub(turns);
        ensure!(remaining > 0, "model turn budget is exhausted");
        let prompt = messages.pop().context("model boundary requires a prompt")?;
        let mut spec = self.spec.clone();
        spec.max_turns = Some(remaining);
        self.compacted_usage += self.run.usage();
        self.compacted_turns = turns;
        self.run = AgentRun::from_spec(
            &spec,
            prompt,
            Some(messages.into_iter().map(Arc::new).collect()),
        );
        self.next_step()
    }

    /// Replace old, complete message groups with an explicit summary. The
    /// caller produces the summary; tool-call/result pairs in the retained
    /// suffix and native provider reasoning there remain byte-for-byte data.
    /// This is allowed only before an unprepared model effect, never while a
    /// tool or network request is outstanding. The returned step replaces any
    /// previously obtained model step.
    pub fn compact_before_model(
        &mut self,
        summary: &str,
        keep_recent: usize,
    ) -> Result<AgentRunStep> {
        // A failed compaction must not consume notes or replace the model
        // boundary for the caller that continues with its original step.
        let mut compacted = self.clone();
        let step = compacted.compact_at_boundary(summary, keep_recent)?;
        *self = compacted;
        Ok(step)
    }

    fn compact_at_boundary(&mut self, summary: &str, keep_recent: usize) -> Result<AgentRunStep> {
        ensure!(
            self.pending_request.is_none(),
            "cannot compact a prepared model attempt"
        );
        ensure!(
            !summary.trim().is_empty(),
            "compaction requires an explicit summary"
        );
        self.flush_notes_before_model()?;
        let step = self.next_step()?;
        let AgentRunStep::CallModel {
            prompt, history, ..
        } = step
        else {
            bail!("compaction is only valid at a model boundary with all tools answered");
        };
        let mut messages: Vec<Message> = history.iter().map(|item| Message::clone(item)).collect();
        messages.push(Message::clone(&prompt));
        let mut start = messages.len().saturating_sub(keep_recent.max(1));
        while start > 0 && rig::conversation::pairing::validate(&messages[start..]).is_err() {
            start -= 1;
        }
        ensure!(
            start > 0,
            "no complete message group can be removed with this retention window"
        );
        let usage = self.usage();
        let retained = messages.split_off(start);
        let mut history = vec![Message::user(format!("Prior analysis summary:\n{summary}"))];
        history.extend(retained);
        let step = self.restart_at_model_boundary(history)?;
        self.compactions.push(CompactionRecord {
            before_turn: self.compacted_turns + 1,
            removed_messages: start,
            summary: summary.into(),
            usage_before: usage,
        });
        Ok(step)
    }

    pub fn answer(&mut self, call: ExecCall, output: Value, is_error: bool) -> Result<()> {
        let output = ToolOutput::json(output);
        let result = if is_error {
            ToolResult::failed(ToolExecutionError::other("tool execution failed"))
                .with_output(output)
        } else {
            ToolResult::success(output)
        };
        self.run
            .answer(call.answer(result))
            .context("accept Rig tool result")
    }

    /// Malformed model calls never reach the host's tool implementation.
    pub fn answer_malformed(&mut self, call: rig::run::MalformedCall) -> Result<()> {
        self.run
            .answer(call.answer(None))
            .context("return malformed-call feedback")
    }

    pub fn from_json(json: &str) -> Result<Self> {
        let run: Self = serde_json::from_str(json).context("decode Rig checkpoint")?;
        run.validate_version()?;
        Ok(run)
    }

    pub fn to_json(&self) -> Result<String> {
        self.validate_version()?;
        serde_json::to_string(self).context("encode Rig checkpoint")
    }
}

#[derive(Clone, Debug)]
pub struct DurableEngine {
    config: ModelConfig,
    identity: ModelIdentity,
    model: DynModel<Completion>,
}

impl DurableEngine {
    pub fn new(mut config: ModelConfig) -> Result<Self> {
        config.validate()?;
        config.base_url = Some(config.resolved_base_url()?);
        config.validate()?;
        let variable = config.key_environment();
        let key = match std::env::var(variable) {
            Ok(value) if !value.trim().is_empty() => value,
            _ if config.provider == Provider::Ollama && config.api_key_env.is_none() => {
                String::new()
            }
            _ => bail!("missing model credential: set environment variable {variable}"),
        };
        Self::connect(config, key)
    }

    fn connect(config: ModelConfig, key: String) -> Result<Self> {
        config.validate()?;
        let base_url = config.resolved_base_url()?;
        let client = rig::rig_reqwest::reqwest::Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .connect_timeout(Duration::from_secs(20))
            .build()
            .context("build model HTTP client")?;
        let transport = rig::http_client::ReqwestClient::from(client);
        let model = match config.provider {
            Provider::OpenAi => OpenAIConfig::new(key)
                .with_base_url(&base_url)
                .connect(transport)
                .chat(&config.model)
                .erase(),
            Provider::OpenAiResponses => OpenAIConfig::new(key)
                .with_base_url(&base_url)
                .connect(transport)
                .responses(&config.model)
                .erase(),
            Provider::Anthropic => AnthropicConfig::new(key)
                .with_base_url(&base_url)
                .connect(transport)
                .completion(&config.model)
                .erase(),
            Provider::Ollama => OllamaConfig::new()
                .with_base_url(&base_url)
                .with_api_key(key)
                .connect(transport)
                .native_completion(&config.model)
                .erase(),
        };
        let identity = ModelIdentity {
            provider: config.provider,
            model: config.model.clone(),
            base_url,
        };
        Ok(Self {
            config,
            identity,
            model,
        })
    }

    pub fn new_run(
        &self,
        prompt: &str,
        preamble: &str,
        tools: Vec<ToolDefinition>,
        max_turns: usize,
    ) -> Result<DurableRun> {
        self.new_run_with_history(prompt, preamble, tools, max_turns, Vec::new())
    }

    pub fn new_run_with_history(
        &self,
        prompt: &str,
        preamble: &str,
        tools: Vec<ToolDefinition>,
        max_turns: usize,
        history: Vec<Message>,
    ) -> Result<DurableRun> {
        ensure!(max_turns > 0, "max_turns must be positive");
        let mut names = BTreeSet::new();
        for definition in &tools {
            ensure!(
                names.insert(definition.name.to_string()),
                "duplicate tool {}",
                definition.name
            );
            ensure!(
                definition.parameters.is_object(),
                "tool {} schema must be an object",
                definition.name
            );
        }
        rig::conversation::pairing::validate(&history)
            .context("inherited conversation has unpaired tool calls")?;
        let spec = RunSpec {
            preamble: Some(preamble.into()),
            max_tokens: Some(self.config.max_tokens),
            temperature: self.config.temperature,
            max_turns: Some(max_turns),
            max_invalid_tool_call_retries: 2,
            max_consecutive_malformed_tool_calls: Some(2),
            additional_params: self.config.additional_params.clone(),
            ..RunSpec::default()
        };
        let history = history.into_iter().map(Arc::new).collect();
        let run = AgentRun::from_spec(&spec, prompt, Some(history));
        Ok(DurableRun {
            rig_version: RIG_STATE_VERSION.into(),
            model: self.identity.clone(),
            spec,
            tools: tools.into_iter().map(Arc::new).collect(),
            run,
            notes: Vec::new(),
            pending_request: None,
            compacted_usage: Usage::default(),
            compacted_turns: 0,
            compactions: Vec::new(),
        })
    }

    /// Freeze the precise Rig request before persisting the checkpoint and
    /// issuing network IO. Repeating this on resume retains the same attempt.
    pub fn prepare_model_call(&self, run: &mut DurableRun, step: &AgentRunStep) -> Result<()> {
        run.validate_version()?;
        ensure!(
            run.model == self.identity,
            "checkpoint model/endpoint differs from configured model; resume with the original configuration"
        );
        let AgentRunStep::CallModel { .. } = step else {
            bail!("prepare_model_call requires a CallModel step");
        };
        if let Some(pending) = &run.pending_request {
            ensure!(
                pending.turn == run.run.turn(),
                "pending model request belongs to another turn"
            );
            return Ok(());
        }
        let updated = run.flush_notes_before_model()?;
        let AgentRunStep::CallModel {
            prompt,
            history,
            turn,
        } = updated.as_ref().unwrap_or(step)
        else {
            bail!("coordinator mail did not yield a model boundary");
        };
        let request = prepare_request(
            &run.spec,
            &self.model.capabilities().completion,
            history,
            run.tools.clone(),
            run.run.output_tool_name(),
            None,
        )
        .context("prepare Rig provider request")?;
        run.run.advertise_tools(*turn, request.tools.clone());
        run.pending_request = Some(PreparedAttempt {
            turn: *turn,
            prompt: Message::clone(prompt),
            request,
        });
        Ok(())
    }

    /// Execute exactly one model effect. The host checkpoints the new state
    /// before dispatching any returned tools. API errors leave it retryable.
    pub async fn call_model(&self, run: &mut DurableRun, step: AgentRunStep) -> Result<Usage> {
        self.prepare_model_call(run, &step)?;
        let pending = run
            .pending_request
            .as_ref()
            .context("model attempt was not prepared")?;
        let request = pending
            .request
            .clone()
            .apply(CompletionRequest::new(pending.prompt.clone()));
        let response = self
            .model
            .call(request)
            .await
            .context("Rig provider completion failed")?;
        Self::accept_model_response(run, response)
    }

    /// Stream Rig's typed text/reasoning/tool events to a UI while Rig folds
    /// the same provider-native final response. Tools become executable only
    /// after the completed response has been accepted and checkpointed.
    /// Dropping this future cancels its local stream; a restart reissues the
    /// persisted request, with the host bounding retries.
    pub async fn call_model_streaming<F>(
        &self,
        run: &mut DurableRun,
        step: AgentRunStep,
        mut on_event: F,
    ) -> Result<Usage>
    where
        F: FnMut(&rig::streaming::Item<rig::streaming::StreamEvent>) + Send,
    {
        use futures::StreamExt;
        self.prepare_model_call(run, &step)?;
        let pending = run
            .pending_request
            .as_ref()
            .context("model attempt was not prepared")?;
        let request = pending
            .request
            .clone()
            .apply(CompletionRequest::new(pending.prompt.clone()));
        let mut stream = self
            .model
            .stream(request)
            .context("open Rig provider stream")?;
        while let Some(item) = stream.next().await {
            let item = item.context("read Rig provider stream")?;
            on_event(&item);
        }
        let response = stream
            .finish()
            .await
            .context("finish Rig provider stream")?;
        Self::accept_model_response(run, response)
    }

    fn accept_model_response(
        run: &mut DurableRun,
        response: rig::completion::CompletionResponse,
    ) -> Result<Usage> {
        let pending = run
            .pending_request
            .as_ref()
            .context("model attempt was not prepared")?;
        let usage = response.usage;
        let turn = ModelTurn::from_response(&response, &pending.request);
        run.pending_request = None;
        let outcome = run
            .run
            .model_response(turn)
            .context("accept Rig model response")?;
        if matches!(outcome, ModelTurnOutcome::NeedsResolution(_)) {
            run.resolve_invalid_calls()?;
        }
        Ok(usage)
    }

    /// One-shot bare completion that condenses a large tool-result envelope
    /// into a compact object for the model context. The original envelope is
    /// always in evidence; provider failures return it unchanged so nothing
    /// is ever lost. Returns the (possibly summarized) envelope and the
    /// summarizer's own usage for budget accounting.
    pub async fn summarize_tool_output(
        &self,
        tool_name: &str,
        envelope: &Value,
        max_tokens: u64,
    ) -> (Value, Usage) {
        let input = serde_json::to_string(envelope).unwrap_or_default();
        let prompt = format!(
            "Condense the tool-result envelope below for a binary reverse-engineering agent. \
             Preserve every concrete fact VERBATIM: addresses, offsets, sizes, hashes, flags, \
             register/stack values, error messages, function names, exit codes, counts. Drop \
             redundant prose and boilerplate. Respond with STRICT JSON only: a \"summary\" \
             string of at most 150 words, plus a flat copy of these scalar fields when present: \
             id, job_id, session_id, state, status, path, sha256, finding_id, verification_id, \
             parked, wake_at, exit_code, signal, passed, attempts, successes.\n\
             Tool: {tool_name}\nEnvelope: {input}"
        );
        let request = CompletionRequest::new(prompt)
            .temperature(0.0)
            .max_tokens(max_tokens);
        match self.model.call(request).await {
            Ok(response) => {
                use rig::completion::AssistantContent;
                let usage = response.usage;
                let text: String = response
                    .choice
                    .iter()
                    .filter_map(|part| match part {
                        AssistantContent::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect();
                let trimmed = text
                    .trim()
                    .trim_start_matches("```json")
                    .trim_start_matches("```")
                    .trim_end_matches("```");
                let parsed: Value =
                    serde_json::from_str(trimmed).unwrap_or(json!({"summary": text.trim()}));
                let mut object = parsed.as_object().cloned().unwrap_or_default();
                if !object.contains_key("summary") {
                    object.insert("summary".into(), json!(text.trim()));
                }
                if let Some(id) = envelope.get("evidence_id").cloned() {
                    object.insert("evidence_id".into(), id);
                }
                object.insert(
                    "total_bytes".into(),
                    envelope.get("total_bytes").cloned().unwrap_or(Value::Null),
                );
                object.insert("summarized".into(), json!(true));
                object.insert(
                    "retrieve".into(),
                    json!("use evidence_read with byte ranges"),
                );
                (
                    json!({"result": Value::Object(object), "evidence_id": envelope.get("evidence_id").cloned().unwrap_or(Value::Null), "truncated": false, "total_bytes": envelope.get("total_bytes").cloned().unwrap_or(Value::Null), "summarized": true}),
                    usage,
                )
            }
            Err(_) => (envelope.clone(), Usage::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    async fn server(
        replies: Vec<Value>,
    ) -> (String, tokio::task::JoinHandle<Vec<(String, Value)>>) {
        server_wire(
            replies
                .into_iter()
                .map(|reply| {
                    (
                        "application/json".to_owned(),
                        serde_json::to_string(&reply).unwrap(),
                    )
                })
                .collect(),
        )
        .await
    }

    async fn server_wire(
        replies: Vec<(String, String)>,
    ) -> (String, tokio::task::JoinHandle<Vec<(String, Value)>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut received = Vec::new();
            for (content_type, reply) in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0_u8; 8192];
                let (header_end, content_length) = loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0, "request ended before its headers");
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(index) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..index]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.split_once(':')
                                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                    .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        break (index + 4, length);
                    }
                };
                while bytes.len() < header_end + content_length {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0, "request ended before its body");
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let path = String::from_utf8_lossy(&bytes[..header_end])
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                let body = serde_json::from_slice(&bytes[header_end..header_end + content_length])
                    .unwrap();
                received.push((path, body));
                let body = reply.as_bytes();
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(headers.as_bytes()).await.unwrap();
                socket.write_all(body).await.unwrap();
            }
            received
        });
        (base, task)
    }

    fn engine(provider: Provider, base: String) -> DurableEngine {
        DurableEngine::connect(
            ModelConfig {
                provider,
                model: match provider {
                    Provider::Anthropic => "claude-sonnet-4-5",
                    Provider::Ollama => "qwen3",
                    _ => "gpt-5.2",
                }
                .into(),
                base_url: Some(base),
                ..ModelConfig::default()
            },
            "test-api-credential-never-checkpoint".into(),
        )
        .unwrap()
    }

    fn tool() -> ToolDefinition {
        ToolDefinition::new(
            rig::message::ToolName::new("binary_inspect").unwrap(),
            "Inspect a binary",
            json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        )
    }

    fn roundtrip(run: &DurableRun) -> DurableRun {
        let checkpoint = run.to_json().unwrap();
        assert!(!checkpoint.contains("test-api-credential-never-checkpoint"));
        DurableRun::from_json(&checkpoint).unwrap()
    }

    async fn finish_tool_turn(engine: &DurableEngine, run: &mut DurableRun) {
        let AgentRunStep::CallTools { mut calls } = run.next_step().unwrap() else {
            panic!("expected tool call");
        };
        let PendingToolCall::Execute(call) = calls.remove(0) else {
            panic!("expected executable call");
        };
        assert_eq!(call.name().as_str(), "binary_inspect");
        assert_eq!(call.arguments()["path"], "fixture");
        run.answer(call, json!({"format":"ELF"}), false).unwrap();
        *run = roundtrip(run);
        let step = run.next_step().unwrap();
        engine.prepare_model_call(run, &step).unwrap();
        *run = roundtrip(run);
        engine.call_model(run, step).await.unwrap();
        let AgentRunStep::Done(response) = run.next_step().unwrap() else {
            panic!("expected final answer");
        };
        assert_eq!(response.output(), "verified");
        assert!(response.usage.input_tokens.unwrap() >= 10);
    }

    #[tokio::test]
    async fn openai_tool_protocol_survives_each_checkpoint_boundary() {
        let first = json!({"id":"chat-1","object":"chat.completion","created":1,"model":"gpt-5.2","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"binary_inspect","arguments":"{\"path\":\"fixture\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13}});
        let second = json!({"id":"chat-2","object":"chat.completion","created":1,"model":"gpt-5.2","choices":[{"index":0,"message":{"role":"assistant","content":"verified"},"finish_reason":"stop"}],"usage":{"prompt_tokens":15,"completion_tokens":2,"total_tokens":17}});
        let (base, server) = server(vec![first, second]).await;
        let engine = engine(Provider::OpenAi, format!("{base}/v1"));
        let mut run = engine
            .new_run("Inspect fixture", "Use evidence", vec![tool()], 4)
            .unwrap();
        let step = run.next_step().unwrap();
        engine.prepare_model_call(&mut run, &step).unwrap();
        run.queue_note("A message arriving after preparation applies to the next attempt");
        run = roundtrip(&run);
        let usage = engine.call_model(&mut run, step).await.unwrap();
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(run.turn(), 1);
        run = roundtrip(&run);
        finish_tool_turn(&engine, &mut run).await;
        assert_eq!(run.usage().total_tokens, Some(30));
        let requests = server.await.unwrap();
        assert_eq!(requests[0].0, "/v1/chat/completions");
        assert_eq!(
            requests[0].1["tools"][0]["function"]["name"],
            "binary_inspect"
        );
        assert!(!requests[0].1.to_string().contains("message arriving"));
        assert!(requests[1].1.to_string().contains("message arriving"));
        assert!(
            requests[1].1["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "tool" && message["tool_call_id"] == "call_1")
        );
        assert!(run.history().iter().any(|message| {
            serde_json::to_string(message)
                .unwrap()
                .contains("message arriving")
        }));
    }

    #[tokio::test]
    async fn anthropic_thinking_signature_and_tool_use_replay_natively() {
        let first = json!({"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"thinking","thinking":"inspect the input parser","signature":"provider-signed-thinking"},{"type":"tool_use","id":"toolu_1","name":"binary_inspect","input":{"path":"fixture"}}],"stop_reason":"tool_use","stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":3}});
        let second = json!({"id":"msg_2","type":"message","role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"text","text":"verified"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":12,"output_tokens":2}});
        let (base, server) = server(vec![first, second]).await;
        let engine = engine(Provider::Anthropic, base);
        let mut run = engine
            .new_run("Inspect fixture", "Use evidence", vec![tool()], 4)
            .unwrap();
        let step = run.next_step().unwrap();
        engine.call_model(&mut run, step).await.unwrap();
        run = roundtrip(&run);
        finish_tool_turn(&engine, &mut run).await;
        let requests = server.await.unwrap();
        assert_eq!(requests[0].0, "/v1/messages");
        let second = requests[1].1.to_string();
        assert!(
            second.contains("provider-signed-thinking"),
            "native signature was lost: {second}"
        );
        assert!(second.contains("tool_result") && second.contains("toolu_1"));
    }

    #[tokio::test]
    async fn ollama_uses_native_chat_and_retains_thinking_and_usage() {
        let reply = json!({"model":"qwen3","created_at":"2026-11-06T00:00:00Z","message":{"role":"assistant","content":"verified","thinking":"inspect the parser"},"done":true,"done_reason":"stop","prompt_eval_count":10,"eval_count":2});
        let (base, server) = server(vec![reply]).await;
        let engine = engine(Provider::Ollama, base);
        let mut run = engine
            .new_run("Inspect fixture", "Use evidence", vec![], 2)
            .unwrap();
        let step = run.next_step().unwrap();
        engine.call_model(&mut run, step).await.unwrap();
        run = roundtrip(&run);
        let AgentRunStep::Done(response) = run.next_step().unwrap() else {
            panic!("expected final answer");
        };
        assert_eq!(response.output(), "verified");
        assert_eq!(response.usage.input_tokens, Some(10));
        assert!(run.to_json().unwrap().contains("inspect the parser"));
        assert_eq!(server.await.unwrap()[0].0, "/api/chat");
    }

    #[tokio::test]
    async fn responses_api_replays_encrypted_reasoning_and_call_identity() {
        let first = json!({"id":"resp_1","object":"response","created_at":1,"status":"completed","model":"gpt-5.2","output":[{"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"inspect parser"}],"encrypted_content":"opaque-native-reasoning"},{"type":"function_call","id":"fc_1","call_id":"call_1","name":"binary_inspect","arguments":"{\"path\":\"fixture\"}","status":"completed"}],"usage":{"input_tokens":10,"output_tokens":4,"total_tokens":14},"tools":[]});
        let second = json!({"id":"resp_2","object":"response","created_at":1,"status":"completed","model":"gpt-5.2","output":[{"type":"message","id":"msg_2","status":"completed","role":"assistant","content":[{"type":"output_text","text":"verified","annotations":[]}]}],"usage":{"input_tokens":14,"output_tokens":2,"total_tokens":16},"tools":[]});
        let (base, server) = server(vec![first, second]).await;
        let engine = engine(Provider::OpenAiResponses, format!("{base}/v1"));
        let mut run = engine
            .new_run("Inspect fixture", "Use evidence", vec![tool()], 4)
            .unwrap();
        let step = run.next_step().unwrap();
        engine.call_model(&mut run, step).await.unwrap();
        run = roundtrip(&run);
        finish_tool_turn(&engine, &mut run).await;
        let requests = server.await.unwrap();
        assert_eq!(requests[0].0, "/v1/responses");
        let second = requests[1].1.to_string();
        assert!(second.contains("opaque-native-reasoning"));
        assert!(second.contains("function_call_output") && second.contains("call_1"));
    }

    #[tokio::test]
    async fn streaming_emits_typed_events_and_commits_the_folded_response() {
        let frames = [
            json!({"id":"chat-stream","object":"chat.completion.chunk","created":1,"model":"gpt-5.2","choices":[{"index":0,"delta":{"role":"assistant","content":"ver"},"finish_reason":null}]}),
            json!({"id":"chat-stream","object":"chat.completion.chunk","created":1,"model":"gpt-5.2","choices":[{"index":0,"delta":{"content":"ified"},"finish_reason":null}]}),
            json!({"id":"chat-stream","object":"chat.completion.chunk","created":1,"model":"gpt-5.2","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
            json!({"id":"chat-stream","object":"chat.completion.chunk","created":1,"model":"gpt-5.2","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}),
        ];
        let body = frames
            .iter()
            .map(|frame| format!("data: {frame}\n\n"))
            .collect::<String>()
            + "data: [DONE]\n\n";
        let (base, server) = server_wire(vec![("text/event-stream".into(), body)]).await;
        let engine = engine(Provider::OpenAi, format!("{base}/v1"));
        let mut run = engine
            .new_run("Inspect fixture", "Use evidence", vec![], 3)
            .unwrap();
        let step = run.next_step().unwrap();
        let mut text = String::new();
        engine
            .call_model_streaming(&mut run, step, |item| {
                if let rig::streaming::Item::Event(rig::streaming::StreamEvent::Text {
                    text: delta,
                    ..
                }) = item
                {
                    text.push_str(delta);
                }
            })
            .await
            .unwrap();
        assert_eq!(text, "verified");
        run = roundtrip(&run);
        let AgentRunStep::Done(response) = run.next_step().unwrap() else {
            panic!("expected final answer");
        };
        assert_eq!(response.output(), "verified");
        assert_eq!(response.usage.total_tokens, Some(12));
        assert_eq!(server.await.unwrap()[0].1["stream"], true);
    }

    #[tokio::test]
    async fn compaction_retains_tool_pairs_and_total_usage_and_rejects_pending_effects() {
        let tool_turn = |number| json!({"id":format!("chat-{number}"),"object":"chat.completion","created":1,"model":"gpt-5.2","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":format!("call_{number}"),"type":"function","function":{"name":"binary_inspect","arguments":"{\"path\":\"fixture\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13}});
        let final_turn = json!({"id":"chat-3","object":"chat.completion","created":1,"model":"gpt-5.2","choices":[{"index":0,"message":{"role":"assistant","content":"verified"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}});
        let (base, server) = server(vec![tool_turn(1), tool_turn(2), final_turn]).await;
        let engine = engine(Provider::OpenAi, format!("{base}/v1"));
        let mut run = engine
            .new_run("Inspect fixture", "Use evidence", vec![tool()], 4)
            .unwrap();
        for index in 0..2 {
            run.queue_note(format!("older coordination mail {index}"));
            let step = run.next_step().unwrap();
            engine.call_model(&mut run, step).await.unwrap();
            assert!(
                run.notes.is_empty(),
                "delivered mail must enter the canonical history"
            );
            let AgentRunStep::CallTools { mut calls } = run.next_step().unwrap() else {
                panic!("expected calls");
            };
            assert!(run.compact_before_model("summary", 1).is_err());
            let PendingToolCall::Execute(call) = calls.remove(0) else {
                panic!("expected executable call");
            };
            run.answer(call, json!({"format":"ELF","index":index}), false)
                .unwrap();
        }
        let step = run
            .compact_before_model("Two prior inspection calls completed", 1)
            .unwrap();
        assert_eq!(run.compactions.len(), 1);
        assert_eq!(run.usage().total_tokens, Some(26));
        assert_eq!(run.turn(), 3);
        rig::conversation::pairing::validate(&run.run.full_history()).unwrap();
        assert!(
            !serde_json::to_string(&run.history())
                .unwrap()
                .contains("older coordination mail 0")
        );
        engine.prepare_model_call(&mut run, &step).unwrap();
        assert!(
            run.compact_before_model("invalid while request prepared", 1)
                .is_err()
        );
        run = roundtrip(&run);
        engine.call_model(&mut run, step).await.unwrap();
        let AgentRunStep::Done(response) = run.next_step().unwrap() else {
            panic!("expected final");
        };
        assert_eq!(response.usage.total_tokens, Some(38));
        assert_eq!(run.turn(), 3);
        let requests = server.await.unwrap();
        assert!(
            requests[2]
                .1
                .to_string()
                .contains("Two prior inspection calls completed")
        );
        assert!(
            requests[0]
                .1
                .to_string()
                .contains("older coordination mail 0")
        );
        for request in &requests {
            for message in request.1["messages"].as_array().unwrap() {
                if message["role"] == "system" {
                    assert!(!message.to_string().contains("coordination mail"));
                }
            }
        }
    }

    #[test]
    fn rejected_compaction_preserves_mail_and_the_original_model_boundary() {
        let engine = engine(Provider::Ollama, "http://127.0.0.1:1".into());
        let mut run = engine
            .new_run("understand the format", "Use evidence", vec![tool()], 4)
            .unwrap();
        let step = run.next_step().unwrap();
        run.queue_note("operator constraint that must survive a rejected compaction");
        let before = run.to_json().unwrap();
        assert!(run.compact_before_model("summary", 12).is_err());
        assert_eq!(run.to_json().unwrap(), before);
        engine.prepare_model_call(&mut run, &step).unwrap();
        assert!(
            serde_json::to_string(&run.pending_request)
                .unwrap()
                .contains("operator constraint")
        );
        assert!(run.notes.is_empty());
    }

    #[test]
    fn checkpoint_and_config_reject_version_or_credential_mistakes() {
        let engine = engine(Provider::Ollama, "http://127.0.0.1:1".into());
        let mut run = engine.new_run("inspect", "preamble", vec![], 2).unwrap();
        run.rig_version = "0.45.0".into();
        assert!(
            run.next_step()
                .unwrap_err()
                .to_string()
                .contains("migration")
        );
        let mut config = ModelConfig {
            base_url: Some("https://user:secret@example.org".into()),
            ..ModelConfig::default()
        };
        assert!(config.validate().is_err());
        config.base_url = Some("http://localhost:11434".into());
        config.additional_params = Some(json!({"nested":{"api_key":"secret"}}));
        assert!(config.validate().is_err());
    }
}
