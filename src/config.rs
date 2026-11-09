use crate::{domain::Budget, llm::ModelConfig, mcp::McpServerConfig, runtime::RuntimeConfig};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Pricing {
    pub input_usd_per_million: f64,
    pub output_usd_per_million: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StaticConfig {
    pub ghidra_executable: Option<PathBuf>,
    pub ghidra_cache_dir: PathBuf,
    pub ghidra_script_dir: Option<PathBuf>,
    pub timeout_seconds: u64,
    pub analysis_timeout_seconds: u64,
    pub max_cpu: usize,
}
impl Default for StaticConfig {
    fn default() -> Self {
        Self {
            ghidra_executable: None,
            ghidra_cache_dir: PathBuf::from(".re-agent/ghidra"),
            ghidra_script_dir: None,
            timeout_seconds: 300,
            analysis_timeout_seconds: 240,
            max_cpu: 2,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolLimits {
    pub max_output_bytes: usize,
    pub max_evidence_read_bytes: usize,
    pub timeout_seconds: u64,
}
impl Default for ToolLimits {
    fn default() -> Self {
        Self {
            max_output_bytes: 12000,
            max_evidence_read_bytes: 16384,
            timeout_seconds: 360,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecutionConfig {
    pub stream: bool,
    pub model_retries: usize,
    pub compact_after_chars: usize,
    pub keep_recent_messages: usize,
    /// When enabled, large tool-result envelopes are replaced in the model
    /// context by a short LM summary (plus the evidence ID). The original
    /// bytes remain in evidence and the call record; replay stays exact.
    pub summarize_tool_outputs: bool,
    pub summarize_threshold_chars: usize,
    pub summarize_max_tokens: u64,
}
impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            stream: true,
            model_retries: 2,
            compact_after_chars: 200000,
            keep_recent_messages: 12,
            summarize_tool_outputs: true,
            summarize_threshold_chars: 6000,
            summarize_max_tokens: 400,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub state_dir: PathBuf,
    pub model: ModelConfig,
    pub budget: Budget,
    pub runtime: RuntimeConfig,
    pub analysis: StaticConfig,
    pub pricing: Pricing,
    pub tools: ToolLimits,
    pub skills_dir: Option<PathBuf>,
    pub demos_dir: Option<PathBuf>,
    pub mcp: Vec<McpServerConfig>,
    pub execution: ExecutionConfig,
}
impl Default for AppConfig {
    fn default() -> Self {
        Self {
            state_dir: PathBuf::from(".re-agent"),
            model: ModelConfig::default(),
            budget: Budget::default(),
            runtime: RuntimeConfig::default(),
            analysis: StaticConfig::default(),
            pricing: Pricing::default(),
            tools: ToolLimits::default(),
            skills_dir: None,
            // The repository ships worked-example demos; a missing directory
            // simply skips them (see agents::load_demo).
            demos_dir: Some(PathBuf::from(crate::prompts::EXAMPLES_DIR)),
            mcp: vec![],
            execution: ExecutionConfig::default(),
        }
    }
}
impl AppConfig {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut config: Self = if let Some(path) = path {
            toml::from_str(
                &fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?,
            )
            .context("invalid configuration TOML")?
        } else {
            Self::default()
        };
        if let Some(path) = path {
            let absolute = fs::canonicalize(path)?;
            let base = absolute
                .parent()
                .context("configuration directory missing")?;
            config.resolve_paths(base);
        }
        config.validate()?;
        Ok(config)
    }
    fn resolve_paths(&mut self, base: &Path) {
        fn resolve(base: &Path, path: &mut PathBuf) {
            if path.is_relative() {
                *path = base.join(&*path);
            }
        }
        resolve(base, &mut self.state_dir);
        resolve(base, &mut self.runtime.state_dir);
        resolve(base, &mut self.runtime.bridge_path);
        resolve(base, &mut self.analysis.ghidra_cache_dir);
        if let Some(p) = &mut self.skills_dir {
            resolve(base, p);
        }
        if let Some(p) = &mut self.demos_dir {
            resolve(base, p);
        }
        if let Some(p) = &mut self.analysis.ghidra_script_dir {
            resolve(base, p);
        }
        if let Some(p) = &mut self.analysis.ghidra_executable
            && p.components().count() > 1
        {
            resolve(base, p);
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.model.validate()?;
        ensure!(
            self.budget.max_agents > 0 && self.budget.max_concurrent_agents > 0,
            "agent limits must be positive"
        );
        ensure!(
            self.budget.max_turns_per_agent > 0
                && self.budget.max_total_tokens > 0
                && self.budget.max_wall_seconds > 0,
            "run budgets must be positive"
        );
        ensure!(
            self.tools.max_output_bytes >= 1024
                && self.tools.max_evidence_read_bytes > 0
                && self.tools.timeout_seconds > 0,
            "invalid tool limits"
        );
        ensure!(
            self.execution.compact_after_chars >= 8192 && self.execution.keep_recent_messages > 0,
            "invalid context retention limits"
        );
        ensure!(
            self.analysis.timeout_seconds > 0
                && self.analysis.analysis_timeout_seconds > 0
                && self.analysis.max_cpu > 0,
            "analysis timeouts and CPU limits must be positive"
        );
        ensure!(
            self.analysis.timeout_seconds >= self.analysis.analysis_timeout_seconds,
            "analysis.timeout_seconds must cover analysis_timeout_seconds"
        );
        if self.runtime.backend != crate::runtime::RuntimeBackend::Disabled {
            // Worker-side Ghidra queries run synchronously up to
            // analysis.timeout_seconds; a shorter transport timeout would
            // kill the call (and previously the whole transport) mid-query.
            ensure!(
                self.runtime.request_timeout_secs >= self.analysis.timeout_seconds,
                "runtime.request_timeout_secs ({}) must cover analysis.timeout_seconds ({})",
                self.runtime.request_timeout_secs,
                self.analysis.timeout_seconds
            );
        }
        ensure!(
            self.pricing.input_usd_per_million.is_finite()
                && self.pricing.output_usd_per_million.is_finite()
                && self.pricing.input_usd_per_million >= 0.0
                && self.pricing.output_usd_per_million >= 0.0,
            "invalid pricing"
        );
        if let Some(limit) = self.budget.max_usd {
            ensure!(limit.is_finite() && limit > 0.0, "max_usd must be positive");
            ensure!(
                self.pricing.input_usd_per_million > 0.0
                    && self.pricing.output_usd_per_million > 0.0,
                "USD budget requires configured input and output prices"
            );
        }
        Ok(())
    }
    pub fn write_example(path: &Path) -> Result<()> {
        if path.exists() {
            anyhow::bail!("{} already exists", path.display());
        }
        fs::write(path, toml::to_string_pretty(&Self::default())?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeBackend;

    #[test]
    fn example_directories_are_top_level_overrides_resolved_from_config() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("re-agent.toml");
        let mut example: toml::Value =
            toml::from_str(include_str!("../re-agent.example.toml")).unwrap();
        let table = example.as_table_mut().unwrap();
        assert!(table.contains_key("demos_dir"));
        table.insert(
            "demos_dir".into(),
            toml::Value::String("custom-examples".into()),
        );
        table.insert(
            "skills_dir".into(),
            toml::Value::String("custom-instructions".into()),
        );
        fs::write(&path, toml::to_string(&example).unwrap()).unwrap();
        let config = AppConfig::load(Some(&path)).unwrap();
        let base = fs::canonicalize(directory.path()).unwrap();
        assert_eq!(config.demos_dir.unwrap(), base.join("custom-examples"));
        assert_eq!(config.skills_dir.unwrap(), base.join("custom-instructions"));
    }

    #[test]
    fn role_configuration_selects_analyst_and_verifier_models_and_budgets() {
        let config: AppConfig = toml::from_str(
            r#"
            [model.role_models]
            analyst = { model = "analysis-model", max_tokens = 16384 }
            verifier = { model = "verification-model", max_tokens = 8192 }
            [budget.role_token_shares]
            analyst = 0.4
            verifier = 0.2
        "#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.model.role_model_name(crate::domain::Role::Analyst),
            Some("analysis-model")
        );
        assert_eq!(
            config.model.role_model_name(crate::domain::Role::Verifier),
            Some("verification-model")
        );
        assert_eq!(
            config.budget.role_token_cap(crate::domain::Role::Analyst),
            400_000
        );
        assert_eq!(
            config.budget.role_token_cap(crate::domain::Role::Verifier),
            200_000
        );
        for name in [
            "recon", "reverse", "hunter", "fuzz", "crash", "exploit", "reporter",
        ] {
            assert!(
                toml::from_str::<AppConfig>(&format!(
                    "[model.role_models]\n{name} = {{ model = 'fixture' }}"
                ))
                .is_err()
            );
            assert!(
                toml::from_str::<AppConfig>(&format!("[budget.role_token_shares]\n{name} = 0.3"))
                    .is_err()
            );
        }
    }

    #[test]
    fn runtime_timeout_must_cover_worker_ghidra_queries() {
        let mut config = AppConfig::default();
        config.runtime.backend = RuntimeBackend::TrustedLocal;
        config.runtime.request_timeout_secs = 120;
        config.analysis.timeout_seconds = 300;
        let error = config.validate().unwrap_err();
        assert!(error.to_string().contains("must cover"), "{error:#}");
        // Disabled backends have no transport timeout to contradict.
        config.runtime.backend = RuntimeBackend::Disabled;
        config.validate().unwrap();
    }

    #[test]
    fn analysis_timeout_must_cover_the_analysis_phase() {
        let mut config = AppConfig::default();
        config.analysis.timeout_seconds = 100;
        config.analysis.analysis_timeout_seconds = 200;
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("must cover analysis_timeout_seconds")
        );
    }
}
