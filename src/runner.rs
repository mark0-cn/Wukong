//! Assessment lifetime facade. Composition and kernel access use dedicated modules.
use crate::{
    config::AppConfig, coordinator::Coordinator, domain::*, model_catalog::ModelCatalog,
    store::Store, tools::ToolService, usage::UsageService,
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
    pub(crate) _maintenance: Arc<std::fs::File>,
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
        self.drive_with_budget(run_id, cancel, None).await
    }
    pub(crate) async fn drive_with_budget(
        &self,
        run_id: &str,
        cancel: CancellationToken,
        budget: Option<Budget>,
    ) -> Result<RunRecord> {
        crate::supervisor::RunSupervisor::drive(self.clone(), run_id, cancel, budget).await
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
    pub(crate) fn kernel_host(&self) -> crate::kernel_host::KernelHost {
        crate::kernel_host::KernelHost::new(
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
