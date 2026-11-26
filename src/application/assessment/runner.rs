//! Assessment lifetime facade. Composition and kernel access use dedicated modules.
use crate::{
    application::coordinator::Coordinator, application::model_catalog::ModelCatalog,
    application::tools::ToolService, application::usage::UsageService, config::AppConfig,
    domain::*, storage::Store,
};
use anyhow::Result;
#[cfg(test)]
use rig::completion::Usage;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
#[derive(Clone)]
pub struct AssessmentRunner {
    pub(crate) _maintenance:
        Arc<crate::application::maintenance::updates::maintenance::MaintenanceLock>,
    pub store: Store,
    pub coordinator: Coordinator,
    pub tools: Arc<ToolService>,
    pub(crate) config: AppConfig,
    pub(crate) models: Arc<ModelCatalog>,
    pub(crate) usage: UsageService,
    pub(crate) events: broadcast::Sender<Value>,
}
impl AssessmentRunner {
    pub fn config(&self) -> &AppConfig {
        &self.config
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }
    pub async fn drive(&self, run_id: &str, cancel: CancellationToken) -> Result<RunRecord> {
        self.drive_with_budget(run_id, cancel, None, false).await
    }
    pub(crate) async fn drive_with_budget(
        &self,
        run_id: &str,
        cancel: CancellationToken,
        budget: Option<Budget>,
        reconcile_jobs: bool,
    ) -> Result<RunRecord> {
        crate::application::assessment::supervisor::RunSupervisor::drive(
            self.clone(),
            run_id,
            cancel,
            budget,
            reconcile_jobs,
        )
        .await
    }
    pub(crate) fn with_store(&self, store: Store) -> Self {
        Self {
            coordinator: Coordinator::new(store.clone()),
            tools: Arc::new(self.tools.with_store(store.clone())),
            usage: self.usage.with_store(store.clone()),
            store,
            ..self.clone()
        }
    }
    pub(crate) fn kernel_host(&self) -> crate::application::agent_runtime::host::KernelHost {
        crate::application::agent_runtime::host::KernelHost::new(
            self.store.clone(),
            self.tools.clone(),
            self.config.clone(),
            self.models.clone(),
            self.usage.clone(),
            self.events.clone(),
        )
    }
}

#[cfg(test)]
mod tests;
