//! Composition root: open concrete providers and assemble application services.
use crate::{
    config::AppConfig, coordinator::Coordinator, model_catalog::ModelCatalog,
    runner::AssessmentRunner, store::Store, tools::ToolService, usage::UsageService,
};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::broadcast;
pub(crate) async fn build_runner(mut config: AppConfig, store: Store) -> Result<AssessmentRunner> {
    let maintenance = Arc::new(crate::updates::maintenance::assessment_guard()?);
    config.validate()?;
    let models = Arc::new(ModelCatalog::new(config.model.clone())?);
    config.model = models.config().clone();
    let tools = Arc::new(ToolService::new(config.clone(), store.clone()).await?);
    Ok(AssessmentRunner {
        _maintenance: maintenance,
        coordinator: Coordinator::new(store.clone()),
        tools,
        usage: UsageService::new(store.clone(), config.pricing.clone()),
        store,
        config,
        models,
        events: broadcast::channel(1024).0,
    })
}

pub(crate) async fn build_tools(config: AppConfig, store: Store) -> Result<ToolService> {
    config.validate()?;
    let client = if !config.environments.is_empty()
        || config.runtime.backend == crate::runtime::RuntimeBackend::Disabled
    {
        None
    } else {
        Some(crate::runtime::RuntimeClient::connect(config.runtime.clone()).await?)
    };
    let mcp = crate::mcp::McpRegistry::connect(&config.mcp).await?;
    ToolService::from_dependencies(config, store, client, mcp)
}
