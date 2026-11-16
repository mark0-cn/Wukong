//! Operator entry point for the registered Windows experiment VM.
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
    WindowsLab::open(&config.state_dir, vm_id, vm, Arc::new(host))
}

fn read_spec(path: &Path) -> Result<crate::application::windows_lab::ExperimentSpec> {
    let bytes =
        std::fs::read(path).with_context(|| format!("read experiment spec {}", path.display()))?;
    let spec = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse experiment spec {}", path.display()))?;
    Ok(spec)
}
