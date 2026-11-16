//! Controller-side Windows lab service shared by tools.
use super::*;
use crate::adapters::environments::EnvironmentRegistry;
use crate::application::windows_lab::WindowsLab;
use crate::config::{AppConfig, WindowsLabConfig};

#[derive(Clone)]
pub(crate) struct LabService {
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
