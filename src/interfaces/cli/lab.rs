//! Operator entry point for the registered Windows experiment VM.
use crate::adapters::environments::EnvironmentRegistry;
use crate::application::windows_lab::provision::RegisteredEnvironment;
use crate::application::windows_lab::provision_environment::{
    pin_environment, prepare_environment, restore_configuration, retract_replaced_pin,
    write_configuration,
};
use crate::application::windows_lab::{
    CleanGuestOptions, MediaIdentity, ProvisionOptions, ProvisionOutcome, ProvisionSource,
    ReconcileOutcome,
};
use crate::config::AppConfig;
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use clap::Subcommand;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use wukong_worker_package::ConnectionReceipt;

#[derive(Subcommand)]
pub enum Command {
    /// Show the persisted generation, phase and live VMware identity.
    Status {
        #[arg(long)]
        vm: String,
    },
    /// Fetch, verify and publish the spec's dependency offline package.
    Package {
        #[arg(long)]
        vm: String,
        /// Experiment description JSON with the dependency list.
        #[arg(long)]
        spec: PathBuf,
    },
    /// Provision the worker into the powered-off lab VM from finalized media.
    Provision {
        #[arg(long)]
        vm: String,
        /// Finalized offline worker media directory.
        #[arg(long, conflicts_with_all = ["template", "keys", "spec"])]
        media: Option<PathBuf>,
        /// Released offline media template directory (requires --keys and --spec).
        #[arg(long, requires_all = ["keys", "spec"])]
        template: Option<PathBuf>,
        /// Absolute controller signing key directory used by media finalize.
        #[arg(long, requires = "template")]
        keys: Option<PathBuf>,
        /// Finalize enrollment spec JSON (scripts.worker.media schema 1).
        #[arg(long, requires = "template")]
        spec: Option<PathBuf>,
        /// One-command override for the configured mkisofs executable.
        #[arg(long)]
        iso_tool: Option<PathBuf>,
        /// Stage a verified replacement and publish it as the fixed base snapshot.
        #[arg(long)]
        replace_baseline: bool,
        /// Write the managed-SSH environment into the active configuration.
        #[arg(long)]
        register: bool,
        /// Require the registered environment doctor to pass after boot.
        #[arg(long)]
        verify_environment: bool,
        /// Validate and print the planned host/guest actions without mutating.
        #[arg(long)]
        dry_run: bool,
        /// Replace an existing different environment on --register.
        #[arg(long)]
        force: bool,
        /// End-to-end provisioning deadline in seconds.
        #[arg(long, default_value_t = 1800)]
        timeout: u64,
    },
    /// Retry managed-SSH registration and doctor from the settled installation receipt.
    Register {
        #[arg(long)]
        vm: String,
        /// Explicit guest IPv4 address when VMware Tools cannot report one.
        #[arg(long)]
        guest_ip: Option<std::net::Ipv4Addr>,
        /// Replace an existing different environment registration.
        #[arg(long)]
        force: bool,
        /// Registration and doctor deadline in seconds.
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
    /// Remove one failed provisioning attempt's leftovers from the powered-off guest disk.
    CleanGuest {
        #[arg(long)]
        vm: String,
        /// Operation id reported by the failed `lab provision` run.
        #[arg(long)]
        operation: String,
        /// Inspect the guest and print the planned removals without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Restore the fixed base, upgrade its worker when configured, then snapshot the experiment.
    Prepare {
        #[arg(long)]
        vm: String,
        #[arg(long)]
        owner: String,
        #[arg(long)]
        purpose: String,
        /// Experiment description JSON bound to the resulting lease.
        #[arg(long)]
        spec: PathBuf,
    },
    /// Execute one packaged prebuilt user-mode EXE through the worker.
    Run {
        #[arg(long)]
        vm: String,
        #[arg(long)]
        lab: String,
        #[arg(long)]
        generation: u64,
        /// Packaged artifact name, e.g. harness.exe.
        #[arg(long)]
        artifact: String,
        /// Optional stdin bytes delivered as a staged worker input.
        #[arg(long)]
        stdin: Option<PathBuf>,
        #[arg(long, default_value_t = 120)]
        timeout: u64,
    },
    /// Delete unused experiment and receipt-proven base-transition snapshots.
    Prune {
        #[arg(long)]
        vm: String,
    },
    /// Run the packaged install.ps1 through the administrative channel.
    Install {
        #[arg(long)]
        vm: String,
        #[arg(long)]
        lab: String,
        #[arg(long)]
        generation: u64,
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
    /// Fence the active generation, then revert the VM to the baseline.
    Restore {
        #[arg(long)]
        vm: String,
        /// Lab id returned by prepare.
        #[arg(long)]
        lab: String,
        /// Generation returned by prepare.
        #[arg(long)]
        generation: u64,
    },
    /// Resolve a pending prepare/restore after inspecting the VM yourself.
    Reconcile {
        #[arg(long)]
        vm: String,
        #[arg(long)]
        operation: String,
        #[arg(long, value_parser = ["completed", "failed", "unknown"])]
        outcome: String,
    },
}

pub async fn run(config: &AppConfig, command: Command) -> Result<Value> {
    let registry = EnvironmentRegistry::new(config.environments.clone())?
        .with_lab_guards(&config.windows_lab)?;
    let result = run_with_registry(config, command, &registry).await;
    registry.close().await;
    result
}

async fn run_with_registry(
    config: &AppConfig,
    command: Command,
    registry: &EnvironmentRegistry,
) -> Result<Value> {
    let open = |vm: &str| crate::bootstrap::windows_lab::open_configured(config, registry, vm);
    match command {
        Command::Status { vm } => open(&vm)?.status().await,
        Command::Package { vm, spec } => {
            ensure_registered(config, &vm)?;
            let spec = read_spec(&spec)?;
            match crate::application::windows_lab::materialize_dependencies(
                &config.state_dir,
                &spec,
            )
            .await?
            {
                Some(package) => Ok(serde_json::to_value(package)?),
                None => Ok(json!({"vm_id": vm, "package": null})),
            }
        }
        Command::Provision {
            vm,
            media,
            template,
            keys,
            spec,
            iso_tool,
            replace_baseline,
            register,
            verify_environment,
            dry_run,
            force,
            timeout,
        } => {
            ensure_registered(config, &vm)?;
            ensure!(!force || register, "--force is only valid with --register");
            let source = match (media, template, keys, spec) {
                (Some(directory), None, None, None) => ProvisionSource::Media {
                    directory: absolute(&directory)?,
                },
                (None, Some(template), Some(keys), Some(spec)) => ProvisionSource::Template {
                    template: absolute(&template)?,
                    keys: absolute(&keys)?,
                    spec: absolute(&spec)?,
                },
                _ => anyhow::bail!(
                    "provide either --media DIR or --template DIR --keys DIR --spec FILE"
                ),
            };
            let configuration = (register || verify_environment)
                .then(selected_configuration_path)
                .flatten()
                .map(|path| absolute(&path))
                .transpose()?;
            let mut lab = open(&vm)?;
            if register || verify_environment {
                if register {
                    ensure!(
                        configuration.is_some(),
                        "--register cannot determine the active configuration file; pass the global --config option or run where wukong.toml is discoverable"
                    );
                }
                let registrar = CliEnvironment::new(config, configuration.clone(), force);
                lab = lab.with_environment_registrar(Arc::new(registrar));
            }
            let outcome: ProvisionOutcome = lab
                .provision(ProvisionOptions {
                    source,
                    iso_tool: iso_tool.map(|path| absolute(&path)).transpose()?,
                    replace_baseline,
                    register,
                    verify_environment,
                    dry_run,
                    timeout_seconds: timeout,
                })
                .await?;
            let mut value = serde_json::to_value(&outcome)?;
            if let Some(object) = value.as_object_mut() {
                object.insert("vm_id".into(), json!(vm));
            }
            Ok(value)
        }
        Command::Register {
            vm,
            guest_ip,
            force,
            timeout,
        } => {
            ensure_registered(config, &vm)?;
            let configuration = selected_configuration_path()
                .map(|path| absolute(&path))
                .transpose()?
                .context(
                    "lab register cannot determine the active configuration file; pass the global --config option or run where wukong.toml is discoverable",
                )?;
            let lab = open(&vm)?.with_environment_registrar(Arc::new(CliEnvironment::new(
                config,
                Some(configuration),
                force,
            )));
            let outcome = lab.register_environment(guest_ip, timeout).await?;
            let mut value = serde_json::to_value(outcome)?;
            if let Some(object) = value.as_object_mut() {
                object.insert("vm_id".into(), json!(vm));
            }
            Ok(value)
        }
        Command::CleanGuest {
            vm,
            operation,
            dry_run,
        } => {
            ensure_registered(config, &vm)?;
            let outcome = open(&vm)?
                .clean_guest_leftovers(CleanGuestOptions {
                    operation_id: operation,
                    dry_run,
                })
                .await?;
            let mut value = serde_json::to_value(&outcome)?;
            if let Some(object) = value.as_object_mut() {
                object.insert("vm_id".into(), json!(vm));
            }
            Ok(value)
        }
        Command::Prepare {
            vm,
            owner,
            purpose,
            spec,
        } => {
            let spec = read_spec(&spec)?;
            let lease = open(&vm)?.prepare(&owner, &purpose, spec).await?;
            Ok(serde_json::to_value(lease)?)
        }
        Command::Run {
            vm,
            lab: lab_id,
            generation,
            artifact,
            stdin,
            timeout,
        } => {
            let stdin = match stdin {
                Some(path) => Some(
                    std::fs::read(&path)
                        .with_context(|| format!("read harness stdin {}", path.display()))?,
                ),
                None => None,
            };
            let lab = open(&vm)?;
            lab.run(&lab_id, generation, &artifact, stdin, timeout)
                .await
        }
        Command::Prune { vm } => open(&vm)?.prune().await,
        Command::Install {
            vm,
            lab: lab_id,
            generation,
            timeout,
        } => {
            let lab = open(&vm)?;
            lab.install(&lab_id, generation, timeout).await
        }
        Command::Restore {
            vm,
            lab,
            generation,
        } => open(&vm)?.restore(&lab, generation).await,
        Command::Reconcile {
            vm,
            operation,
            outcome,
        } => {
            let outcome = match outcome.as_str() {
                "completed" => ReconcileOutcome::Completed,
                "failed" => ReconcileOutcome::Failed,
                "unknown" => ReconcileOutcome::Unknown,
                other => anyhow::bail!("unsupported reconcile outcome: {other}"),
            };
            open(&vm)?.reconcile(&operation, outcome).await
        }
    }
}

fn ensure_registered<'a>(
    config: &'a AppConfig,
    vm_id: &str,
) -> Result<&'a crate::config::WindowsLabVm> {
    config
        .windows_lab
        .vms
        .get(vm_id)
        .with_context(|| format!("VM {vm_id} is not registered under [windows_lab.vms]"))
}

fn read_spec(path: &Path) -> Result<crate::application::windows_lab::ExperimentSpec> {
    let bytes =
        std::fs::read(path).with_context(|| format!("read experiment spec {}", path.display()))?;
    let spec = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse experiment spec {}", path.display()))?;
    Ok(spec)
}

fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).with_context(|| format!("resolve {}", path.display()))
}

/// Environment registration and doctor hook for `--register`/`--verify-environment`.
///
/// Registration writes only the verified managed-SSH entry into the operator
/// configuration; it never imports or rewrites private key material.
struct CliEnvironment {
    config: AppConfig,
    state_dir: PathBuf,
    configuration: Option<PathBuf>,
    force: bool,
    registry: Mutex<Option<EnvironmentRegistry>>,
}

impl CliEnvironment {
    fn new(config: &AppConfig, configuration: Option<PathBuf>, force: bool) -> Self {
        Self {
            config: config.clone(),
            state_dir: config.state_dir.clone(),
            configuration,
            force,
            registry: Mutex::new(None),
        }
    }

    fn cached_registry(&self) -> Result<Option<EnvironmentRegistry>> {
        match self.registry.lock() {
            Ok(guard) => Ok(guard.clone()),
            Err(poisoned) => Ok(poisoned.into_inner().clone()),
        }
    }

    fn store_registry(&self, registry: &EnvironmentRegistry) {
        match self.registry.lock() {
            Ok(mut guard) => *guard = Some(registry.clone()),
            Err(poisoned) => *poisoned.into_inner() = Some(registry.clone()),
        }
    }

    fn registry(&self) -> Result<EnvironmentRegistry> {
        if let Some(registry) = self.cached_registry()? {
            return Ok(registry);
        }
        EnvironmentRegistry::new(self.config.environments.clone())
    }
}

#[async_trait]
impl crate::application::windows_lab::provision::ProvisionEnvironment for CliEnvironment {
    fn register(
        &self,
        connection: &ConnectionReceipt,
        media: &MediaIdentity,
        guest_ip: &str,
        identity_file: &Path,
    ) -> Result<RegisteredEnvironment> {
        let configuration = self
            .configuration
            .as_deref()
            .context("--register requires the active configuration file")?;
        let registration = prepare_environment(
            &self.config,
            &self.state_dir,
            media,
            connection,
            guest_ip,
            identity_file,
            self.force,
        )?;
        let written = write_configuration(configuration, &registration, self.force)?;
        // Everything after the configuration rewrite is fallible. If any step
        // fails, restore the backup so the previous `[environments.*]` mapping
        // stays in force together with the lab configuration hash persisted for
        // it; only then is the old public pin retracted.
        let committed = (|| -> Result<RegisteredEnvironment> {
            let pins = pin_environment(&self.state_dir, &registration)?;
            let reloaded = AppConfig::load(Some(configuration))
                .context("reload the configuration after environment registration")?;
            let registry = EnvironmentRegistry::new(reloaded.environments.clone())?;
            // The lab persists this value as its configuration identity, so read
            // it back from the rewritten file: the next command computes the
            // guard hash from exactly that value, not the pre-write draft.
            let environment = reloaded
                .environments
                .get(&registration.environment_id)
                .cloned()
                .with_context(|| {
                    format!(
                        "registered environment {} is missing from the rewritten configuration",
                        registration.environment_id
                    )
                })?;
            let report = json!({
                "environment_id": registration.environment_id,
                "configuration": written.configuration,
                "backup": written.backup,
                "replaced": written.replaced,
                "public_pins": pins,
            });
            retract_replaced_pin(&self.state_dir, &written, &pins)?;
            self.store_registry(&registry);
            Ok(RegisteredEnvironment {
                environment_id: registration.environment_id,
                environment,
                report,
            })
        })();
        match committed {
            Ok(registered) => Ok(registered),
            Err(error) => match restore_configuration(&written) {
                Ok(()) => Err(error),
                Err(restore) => Err(error.context(format!(
                    "restore the configuration backup {} after the failed registration: {restore:#}",
                    written.backup.display()
                ))),
            },
        }
    }

    async fn verify(&self, environment_id: &str) -> Result<Value> {
        let registry = self.registry()?;
        registry.config(environment_id)?;
        registry.doctor(environment_id).await
    }
}

/// Resolve the configuration file the running process loaded. The global
/// `--config` option is not visible to the lab subcommand module, so the
/// explicit argument is recovered from argv and the standard discovery order is
/// mirrored when it is absent.
fn selected_configuration_path() -> Option<PathBuf> {
    let arguments: Vec<String> = std::env::args_os()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let cwd = std::env::current_dir().ok()?;
    let executable = std::env::current_exe().ok();
    resolve_configuration_path(&arguments, &cwd, executable.as_deref())
}

fn resolve_configuration_path(
    arguments: &[String],
    cwd: &Path,
    executable: Option<&Path>,
) -> Option<PathBuf> {
    let mut arguments = arguments.iter().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--config" {
            return arguments.next().map(PathBuf::from).map(|path| {
                if path.is_relative() {
                    cwd.join(path)
                } else {
                    path
                }
            });
        }
        if let Some(value) = argument.strip_prefix("--config=") {
            let path = PathBuf::from(value);
            return Some(if path.is_relative() {
                cwd.join(path)
            } else {
                path
            });
        }
    }
    let local = cwd.join("wukong.toml");
    if local.is_file() {
        return Some(local);
    }
    let portable = executable?.parent()?.join("wukong.toml");
    portable.is_file().then_some(portable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_configuration_argument_wins_over_discovery() {
        let directory = tempfile::tempdir().unwrap();
        let discovered = directory.path().join("wukong.toml");
        std::fs::write(&discovered, "").unwrap();
        let arguments = vec![
            "wukong".to_string(),
            "--config".to_string(),
            "custom.toml".to_string(),
            "lab".to_string(),
        ];
        assert_eq!(
            resolve_configuration_path(&arguments, directory.path(), None),
            Some(directory.path().join("custom.toml"))
        );
    }

    #[test]
    fn configuration_discovery_mirrors_the_cli_order() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("wukong.toml");
        std::fs::write(&local, "").unwrap();
        let arguments = vec!["wukong".to_string(), "lab".to_string()];
        assert_eq!(
            resolve_configuration_path(&arguments, directory.path(), None),
            Some(local)
        );
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_configuration_path(&arguments, empty.path(), None),
            None
        );
    }
}
