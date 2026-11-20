//! Configured Rig provider connections and one-shot typed extraction.
//! Conversation/effect state belongs to the native Rig kernel and memory journal.

use crate::domain::Role;
use anyhow::{Context, Result, bail, ensure};
use rig::{
    DynModel,
    completion::Usage,
    operation::Completion,
    providers::{anthropic::AnthropicConfig, ollama::OllamaConfig, openai::OpenAIConfig},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};

pub const RIG_VERSION: &str = "0.46.0";

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
    /// Explicit per-request recovery ceiling; never changes run or role quotas.
    pub recovery_max_tokens: Option<u64>,
    /// Optional provider-supported overrides, used only on output-limit recovery.
    /// No provider's reasoning is disabled by default.
    pub recovery_additional_params: Option<Value>,
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
            recovery_max_tokens: None,
            recovery_additional_params: None,
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
            self.recovery_max_tokens.is_none_or(|limit| limit > 0),
            "recovery_max_tokens must be positive"
        );
        ensure!(
            self.recovery_additional_params
                .as_ref()
                .is_none_or(Value::is_object),
            "recovery_additional_params must be an object"
        );
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

#[derive(Clone, Debug)]
pub struct ModelEngine {
    config: ModelConfig,
    model: DynModel<Completion>,
}

impl ModelEngine {
    pub fn new(mut config: ModelConfig) -> Result<Self> {
        config.validate()?;
        config.base_url = Some(config.resolved_base_url()?);
        config.validate()?;
        let variable = config.key_environment();
        let key = match crate::adapters::credentials::lookup(variable) {
            Some(value) => value,
            _ if config.provider == Provider::Ollama && config.api_key_env.is_none() => {
                String::new()
            }
            _ => bail!("missing model credential: set environment variable {variable}"),
        };
        Self::connect(config, key)
    }

    pub(crate) fn connect(config: ModelConfig, key: String) -> Result<Self> {
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
        Ok(Self { config, model })
    }

    pub(crate) fn resolved_config(&self) -> &ModelConfig {
        &self.config
    }

    /// Configured provider/endpoint as a native Rig kernel connection.
    pub(crate) fn connection(&self, stream: bool) -> rig_ecs::model::Connection {
        use rig_core::{
            catalog::{Catalog, ModelSpec},
            providers::registry::ProviderId,
            serve::{ErasedHandler, adapters::ModelAdapter},
        };
        let vendor = match self.config.provider {
            Provider::Anthropic => "anthropic",
            Provider::Ollama => "ollama",
            _ => "openai",
        };
        let provider = ProviderId::catalog(vendor).expect("supported provider");
        let mut spec = Catalog::builtin()
            .get(provider, &self.config.model)
            .map(|resolved| resolved.spec.clone())
            .unwrap_or_else(|| ModelSpec::new(provider, &self.config.model));
        spec.tools = true;
        let reference = spec.reference();
        rig_ecs::model::Connection {
            spec: Arc::new(spec),
            handler: rig_ecs::effects::Handler(ErasedHandler::new(ModelDelivery {
                inner: ModelAdapter::<Completion>::new(reference, self.model.clone()),
                stream,
            })),
        }
    }
}

/// Keep unary/stream configuration while using the same Rig effect adapter.
struct ModelDelivery {
    inner: rig_core::serve::adapters::ModelAdapter<Completion>,
    stream: bool,
}
impl rig_core::serve::Serve for ModelDelivery {
    type Family = rig_core::effect::family::Completion;
    fn descriptor(&self) -> rig_core::effect::HandlerDescriptor {
        self.inner.descriptor()
    }
    fn check(
        &self,
        kind: &rig_core::effect::EffectKind,
    ) -> Vec<rig_core::completion::UnsupportedOption> {
        self.inner.check(kind)
    }
    async fn serve(
        &self,
        mut kind: rig_core::effect::EffectKind,
        dispatch: rig_core::serve::Dispatch,
    ) -> rig_core::serve::Reply {
        if let rig_core::effect::EffectKind::Completion { stream, .. } = &mut kind {
            *stream = self.stream;
        }
        self.inner.serve(kind, dispatch).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn configurations_reject_embedded_credentials() {
        assert!(
            ModelConfig {
                base_url: Some("https://user:pass@example.com".into()),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            ModelConfig {
                additional_params: Some(json!({"headers":{"authorization":"credential"}})),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
}
