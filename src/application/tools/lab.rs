//! Controller-side Windows lab service shared by tools.
use super::*;
use crate::adapters::environments::EnvironmentRegistry;
use crate::application::windows_lab::WindowsLab;
use crate::config::{AppConfig, WindowsLabConfig};

#[derive(Clone)]
pub(crate) struct LabService {
    pub(super) store: Store,
    pub(super) config: WindowsLabConfig,
    pub(super) state_dir: PathBuf,
    pub(super) environments: EnvironmentRegistry,
}

impl LabService {
    pub(super) fn list(&self) -> Vec<Value> {
        self.config
            .vms
            .iter()
            .map(|(id, vm)| {
                json!({
                    "vm": id,
                    "environment": vm.environment,
                    "baseline_snapshot": vm.baseline_snapshot,
                    "snapshot_prefix": vm.snapshot_prefix,
                    "start_after_restore": vm.start_after_restore,
                })
            })
            .collect()
    }

    pub(super) fn open(&self, vm_id: &str) -> Result<WindowsLab> {
        let available = self
            .config
            .vms
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        ensure!(
            self.config.vms.contains_key(vm_id),
            "windows_lab VM {vm_id} is not registered; configured VMs: {available}"
        );
        let config = AppConfig {
            state_dir: self.state_dir.clone(),
            windows_lab: self.config.clone(),
            ..Default::default()
        };
        crate::application::windows_lab::service::open_configured(
            &config,
            &self.environments,
            vm_id,
        )
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct LabReference {
    pub vm: String,
    pub lab_id: String,
    pub generation: u64,
    run_id: String,
    agent_id: String,
}
impl LabService {
    pub(super) fn authorize_vm(&self, agent: &AgentMetadata, vm: &str) -> Result<()> {
        let config = self.config.vms.get(vm).context("VM is not registered")?;
        ensure!(
            agent.environment_id.is_some() && agent.environment_id == config.environment,
            "lab VM is not assigned to this task environment"
        );
        ensure!(
            agent.contract.tool_grants.contains(&ToolGrant::WindowsLab),
            "Windows lab task grant required"
        );
        Ok(())
    }
    pub(super) fn bind_reference(
        &self,
        agent: &AgentMetadata,
        vm: &str,
        lab_id: &str,
        generation: u64,
    ) -> Result<String> {
        self.authorize_vm(agent, vm)?;
        let id = stable(
            "lab-ref",
            &format!("{}:{}:{vm}:{lab_id}:{generation}", agent.run_id, agent.id),
        );
        self.store.put(
            "lab_reference",
            &id,
            &agent.run_id,
            &LabReference {
                vm: vm.into(),
                lab_id: lab_id.into(),
                generation,
                run_id: agent.run_id.clone(),
                agent_id: agent.id.clone(),
            },
        )?;
        Ok(id)
    }
    pub(super) fn resolve_reference(
        &self,
        agent: &AgentMetadata,
        id: &str,
    ) -> Result<LabReference> {
        let reference: LabReference = self
            .store
            .get("lab_reference", id)?
            .context("unknown lab_ref; use the reference returned by lab_prepare")?;
        ensure!(
            reference.run_id == agent.run_id && reference.agent_id == agent.id,
            "lab_ref belongs to another run or agent"
        );
        self.authorize_vm(agent, &reference.vm)?;
        Ok(reference)
    }
}
