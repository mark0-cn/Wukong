//! Shared construction of a registered Windows lab from application config.
use super::WindowsLab;
use crate::adapters::environments::EnvironmentRegistry;
use crate::adapters::windows_lab::VmwareVm;
use crate::adapters::windows_lab_admin::VmwareAdmin;
use crate::adapters::windows_lab_worker::WindowsWorker;
use crate::config::AppConfig;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;

pub fn open_configured(
    config: &AppConfig,
    environments: &EnvironmentRegistry,
    vm_id: &str,
) -> Result<WindowsLab> {
    let vm = config
        .windows_lab
        .vms
        .get(vm_id)
        .with_context(|| format!("windows_lab VM {vm_id} is not registered"))?;
    let host = VmwareVm::new(
        &vm.vmrun,
        &vm.vmx,
        Duration::from_secs(vm.operation_timeout_seconds),
    )?;
    let environment_config = vm
        .environment
        .as_deref()
        .map(|id| environments.config(id))
        .transpose()?;
    let mut lab = WindowsLab::open_with_environment(
        &config.state_dir,
        vm_id,
        vm,
        environment_config,
        Arc::new(host),
    )?;
    if let Some(environment) = &vm.environment {
        lab = lab.with_worker(Arc::new(WindowsWorker::new(
            environments.clone(),
            environment.clone(),
        )));
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
