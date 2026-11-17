use crate::{adapters::mcp::McpServerConfig, adapters::models::ModelConfig, domain::Budget};
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
    /// Registered Linux environment used for Ghidra static analysis when the
    /// bound worker has no Ghidra (for example a Windows execution worker).
    pub static_environment: Option<String>,
    pub timeout_seconds: u64,
    pub analysis_timeout_seconds: u64,
    pub max_cpu: usize,
}
impl Default for StaticConfig {
    fn default() -> Self {
        Self {
            ghidra_executable: None,
            ghidra_cache_dir: PathBuf::from(".wukong/ghidra"),
            ghidra_script_dir: None,
            static_environment: None,
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
    /// Changed-request retries for reasoning-only output exhaustion (0 disables, at most 2).
    pub output_recovery_attempts: u32,
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
            output_recovery_attempts: 1,
            compact_after_chars: 200000,
            keep_recent_messages: 12,
            summarize_tool_outputs: true,
            summarize_threshold_chars: 6000,
            summarize_max_tokens: 400,
        }
    }
}
/// Controller-owned Windows experiment VM registry. Host paths and the VMware
/// executable come from operator configuration; model input never supplies them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowsLabConfig {
    pub vms: std::collections::BTreeMap<String, WindowsLabVm>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowsLabVm {
    pub vmx: PathBuf,
    pub vmrun: PathBuf,
    /// Registered Windows environment that executes packaged harnesses in this VM.
    pub environment: Option<String>,
    /// Administrative channel used only for service/driver installation media.
    pub admin: Option<WindowsLabAdmin>,
    pub baseline_snapshot: String,
    pub snapshot_prefix: String,
    /// Upper bound for one VMware host operation (snapshot, revert, start).
    pub operation_timeout_seconds: u64,
    pub start_after_restore: bool,
}
impl Default for WindowsLabVm {
    fn default() -> Self {
        Self {
            vmx: PathBuf::new(),
            vmrun: PathBuf::from("vmrun"),
            environment: None,
            admin: None,
            baseline_snapshot: "wukong-lab-baseline".into(),
            snapshot_prefix: "wukong-lab".into(),
            operation_timeout_seconds: 300,
            start_after_restore: false,
        }
    }
}
fn valid_snapshot_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowsLabAdmin {
    pub user: String,
    /// Environment variable holding the password; never persisted in TOML.
    pub password_env: String,
}
impl Default for WindowsLabAdmin {
    fn default() -> Self {
        Self {
            user: "Administrator".into(),
            password_env: "WUKONG_LAB_ADMIN_PASSWORD".into(),
        }
    }
}
impl AppConfig {
    fn validate_windows_lab(&self) -> Result<()> {
        let mut physical_vms = std::collections::BTreeSet::new();
        let mut lab_environments = std::collections::BTreeSet::new();
        for (id, vm) in &self.windows_lab.vms {
            ensure!(
                physical_vms.insert(crate::adapters::environments::lab_guard::physical_identity(
                    &vm.vmx
                )?),
                "multiple lab aliases name the same physical VM"
            );
            if let Some(environment) = &vm.environment {
                ensure!(
                    lab_environments.insert(environment),
                    "multiple lab VMs name the same environment"
                );
            }
            crate::adapters::environments::validate_id(id)?;
            ensure!(
                vm.vmx.is_absolute() && vm.vmrun.is_absolute(),
                "windows_lab VM {id} requires absolute vmx and vmrun paths"
            );
            ensure!(
                vm.vmx.extension().is_some_and(|ext| ext == "vmx"),
                "windows_lab VM {id} vmx must name a .vmx file"
            );
            ensure!(
                valid_snapshot_name(&vm.baseline_snapshot)
                    && valid_snapshot_name(&vm.snapshot_prefix),
                "windows_lab VM {id} snapshot names contain unsupported characters"
            );
            ensure!(
                vm.snapshot_prefix != vm.baseline_snapshot && vm.snapshot_prefix.len() <= 48,
                "windows_lab VM {id} needs a distinct, short snapshot prefix"
            );
            ensure!(
                (30..=3600).contains(&vm.operation_timeout_seconds),
                "windows_lab VM {id} operation timeout must be 30–3600 seconds"
            );
            if let Some(environment) = &vm.environment {
                let configured = self.environments.get(environment).with_context(|| {
                    format!("windows_lab VM {id} references an unknown environment")
                })?;
                ensure!(
                    configured.platform == crate::adapters::environments::Platform::Windows,
                    "windows_lab VM {id} environment must be a Windows environment"
                );
            }
            if let Some(admin) = &vm.admin {
                ensure!(
                    !admin.user.trim().is_empty()
                        && admin.user.len() <= 128
                        && !admin.user.contains(['\n', '\r', '\0']),
                    "windows_lab VM {id} admin user is invalid"
                );
                ensure!(
                    !admin.password_env.is_empty()
                        && admin
                            .password_env
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
                    "windows_lab VM {id} admin password_env must name an environment variable"
                );
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub state_dir: PathBuf,
    /// None follows the initial task language; new runs persist a concrete choice.
    pub output_language: Option<crate::i18n::Locale>,
    pub model: ModelConfig,
    pub budget: Budget,
    pub environments:
        std::collections::BTreeMap<String, crate::adapters::environments::EnvironmentConfig>,
    pub default_environment: Option<String>,
    pub workbench: crate::application::maintenance::workbench::WorkbenchConfig,
    pub analysis: StaticConfig,
    pub pricing: Pricing,
    pub tools: ToolLimits,
    pub skills_dir: Option<PathBuf>,
    pub demos_dir: Option<PathBuf>,
    pub mcp: Vec<McpServerConfig>,
    pub execution: ExecutionConfig,
    pub windows_lab: WindowsLabConfig,
}
impl Default for AppConfig {
    fn default() -> Self {
        Self {
            state_dir: PathBuf::from(".wukong"),
            output_language: None,
            model: ModelConfig::default(),
            budget: Budget::default(),
            environments: Default::default(),
            default_environment: None,
            workbench: crate::application::maintenance::workbench::WorkbenchConfig::default(),
            analysis: StaticConfig::default(),
            pricing: Pricing::default(),
            tools: ToolLimits::default(),
            skills_dir: None,
            // The repository ships worked-example demos; a missing directory
            // simply skips them (see agents::load_demo).
            demos_dir: Some(PathBuf::from(crate::application::prompts::EXAMPLES_DIR)),
            mcp: vec![],
            execution: ExecutionConfig::default(),
            windows_lab: WindowsLabConfig::default(),
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
    pub(crate) fn resolve_paths(&mut self, base: &Path) {
        fn resolve(base: &Path, path: &mut PathBuf) {
            if path.is_relative() {
                *path = base.join(&*path);
            }
        }
        resolve(base, &mut self.workbench.root);
        resolve(base, &mut self.state_dir);
        for environment in self.environments.values_mut() {
            if let crate::adapters::worker::TransportConfig::TrustedLocal(local) =
                &mut environment.runtime.transport
            {
                resolve(base, &mut local.state_dir);
                resolve(base, &mut local.bridge_path);
            }
            if let Some(ssh) = environment.runtime.managed_ssh_mut() {
                resolve(base, &mut ssh.identity_file);
                resolve(base, &mut ssh.known_hosts_file);
            }
        }
        for vm in self.windows_lab.vms.values_mut() {
            resolve(base, &mut vm.vmx);
            resolve(base, &mut vm.vmrun);
        }
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
        self.validate_windows_lab()?;
        ensure!(
            self.execution.output_recovery_attempts <= 2,
            "output_recovery_attempts must be at most 2"
        );
        for (id, environment) in &self.environments {
            crate::adapters::environments::validate_id(id)?;
            environment.validate()?;
        }
        if let Some(default) = &self.default_environment {
            ensure!(
                self.environments.contains_key(default),
                "default environment is not registered: {default}"
            );
        }
        if let Some(static_environment) = &self.analysis.static_environment {
            let environment = self
                .environments
                .get(static_environment)
                .with_context(|| "analysis.static_environment references an unknown environment")?;
            ensure!(
                environment.platform == crate::adapters::environments::Platform::Linux,
                "analysis.static_environment must be a Linux environment"
            );
        }
        ensure!(
            self.environments.is_empty() || self.default_environment.is_some(),
            "registered environments require a default_environment"
        );
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
        // Shares multiply the run token budget; a non-finite or out-of-range value
        // would silently saturate `Budget::role_token_cap` (inf -> u64::MAX,
        // NaN -> the 10k floor) instead of failing the configuration.
        for (role, share) in &self.budget.role_token_shares {
            ensure!(
                share.is_finite() && *share > 0.0 && *share <= 1.0,
                "budget.role_token_shares.{role:?} must be within (0, 1]"
            );
        }
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
        for environment in self.environments.values() {
            // Worker-side Ghidra queries run synchronously up to
            // analysis.timeout_seconds; a shorter transport timeout would
            // kill the call (and previously the whole transport) mid-query.
            ensure!(
                environment.runtime.request_timeout_secs >= self.analysis.timeout_seconds,
                "environment request_timeout_secs ({}) must cover analysis.timeout_seconds ({})",
                environment.runtime.request_timeout_secs,
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

    #[test]
    fn example_directories_are_top_level_overrides_resolved_from_config() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wukong.toml");
        let mut example: toml::Value =
            toml::from_str(include_str!("../wukong.example.toml")).unwrap();
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
            640_000
        );
        assert_eq!(
            config.budget.role_token_cap(crate::domain::Role::Verifier),
            320_000
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
        let mut config = AppConfig {
            default_environment: Some("linux".into()),
            ..Default::default()
        };
        config.environments.insert(
            "linux".into(),
            crate::adapters::environments::EnvironmentConfig {
                name: "Linux".into(),
                platform: crate::adapters::environments::Platform::Linux,
                installation_id: None,
                runtime: crate::adapters::worker::RuntimeConfig {
                    transport: crate::adapters::worker::TransportConfig::Docker(
                        crate::adapters::worker::DockerConfig {
                            ..crate::adapters::worker::DockerConfig::new("fixture".into())
                        },
                    ),
                    request_timeout_secs: 120,
                },
            },
        );
        config.analysis.timeout_seconds = 300;
        let error = config.validate().unwrap_err();
        assert!(error.to_string().contains("must cover"), "{error:#}");
        config.environments.clear();
        config.default_environment = None;
        config.validate().unwrap();
    }

    #[test]
    fn legacy_single_runtime_configuration_is_rejected_without_migration() {
        assert!(toml::from_str::<AppConfig>("[runtime]\nbackend='disabled'").is_err());
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
