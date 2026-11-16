//! Operator entry point for the registered Windows experiment VM.
use crate::adapters::environments::EnvironmentRegistry;
use crate::adapters::windows_lab_admin::VmwareAdmin;
use crate::adapters::windows_lab_worker::WindowsWorker;
use crate::application::windows_lab::{ReconcileOutcome, WindowsLab};
use crate::config::AppConfig;
use anyhow::{Context, Result};
use clap::Subcommand;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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
    /// Create the baseline (if needed) and one immutable experiment snapshot.
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
    /// Delete unused experiment snapshots, keeping the baseline and active one.
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
    match command {
        Command::Status { vm } => open(config, &vm)?.status().await,
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
        Command::Prepare {
            vm,
            owner,
            purpose,
            spec,
        } => {
            let spec = read_spec(&spec)?;
            let lease = open(config, &vm)?.prepare(&owner, &purpose, spec).await?;
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
            let lab = open(config, &vm)?;
            let result = lab
                .run(&lab_id, generation, &artifact, stdin, timeout)
                .await;
            lab.finish().await;
            result
        }
        Command::Prune { vm } => open(config, &vm)?.prune().await,
        Command::Install {
            vm,
            lab: lab_id,
            generation,
            timeout,
        } => {
            let lab = open(config, &vm)?;
            let result = lab.install(&lab_id, generation, timeout).await;
            lab.finish().await;
            result
        }
        Command::Restore {
            vm,
            lab,
            generation,
        } => open(config, &vm)?.restore(&lab, generation).await,
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
            open(config, &vm)?.reconcile(&operation, outcome).await
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

fn open(config: &AppConfig, vm_id: &str) -> Result<WindowsLab> {
    let vm = ensure_registered(config, vm_id)?;
    let host = crate::adapters::windows_lab::VmwareVm::new(
        &vm.vmrun,
        &vm.vmx,
        Duration::from_secs(vm.operation_timeout_seconds),
    )?;
    let lab = WindowsLab::open(&config.state_dir, vm_id, vm, Arc::new(host))?;
    let mut lab = lab;
    if let Some(environment) = &vm.environment {
        let registry = EnvironmentRegistry::new(config.environments.clone())?;
        lab = lab.with_worker(Arc::new(WindowsWorker::new(registry, environment.clone())));
    }
    if let Some(admin) = &vm.admin
        && let Some(channel) = VmwareAdmin::from_env(
            &vm.vmrun,
            &vm.vmx,
            admin,
            Duration::from_secs(vm.operation_timeout_seconds),
        )?
    {
        lab = lab.with_admin(Arc::new(channel));
    }
    Ok(lab)
}

fn read_spec(path: &Path) -> Result<crate::application::windows_lab::ExperimentSpec> {
    let bytes =
        std::fs::read(path).with_context(|| format!("read experiment spec {}", path.display()))?;
    let spec = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse experiment spec {}", path.display()))?;
    Ok(spec)
}
