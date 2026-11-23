//! Composition root: open concrete providers and assemble application services.
use crate::{
    application::assessment::runner::AssessmentRunner, application::coordinator::Coordinator,
    application::model_catalog::ModelCatalog, application::tools::ToolService,
    application::usage::UsageService, config::AppConfig, storage::Store,
};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::broadcast;

pub(crate) mod windows_lab;
pub(crate) async fn build_runner(
    mut config: AppConfig,
    store: Store,
    telemetry: crate::adapters::telemetry::TelemetryHandle,
) -> Result<AssessmentRunner> {
    let maintenance =
        Arc::new(crate::application::maintenance::updates::maintenance::assessment_guard()?);
    config.validate()?;
    let models = Arc::new(ModelCatalog::new(config.model.clone())?);
    config.model = models.config().clone();
    let tools = Arc::new(build_tools(config.clone(), store.clone(), telemetry).await?);
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

pub(crate) async fn build_tools(
    config: AppConfig,
    store: Store,
    telemetry: crate::adapters::telemetry::TelemetryHandle,
) -> Result<ToolService> {
    config.validate()?;
    let mcp = crate::adapters::mcp::McpRegistry::connect(&config.mcp).await?;
    ToolService::from_dependencies(config, store, mcp, windows_lab::open_configured, telemetry)
}

// Keep the public constructors at the composition boundary. Application use cases
// receive a runner factory and never call back into bootstrap to assemble dependencies.
impl crate::application::assessment::AssessmentService {
    pub fn new(
        config: AppConfig,
        store: Store,
        telemetry: crate::adapters::telemetry::TelemetryHandle,
    ) -> Self {
        Self::from_dependencies(
            config,
            store,
            |config, store, telemetry| Box::pin(build_runner(config, store, telemetry)),
            telemetry,
        )
    }
}
impl AssessmentRunner {
    pub async fn new(config: AppConfig, store: Store) -> Result<Self> {
        build_runner(
            config,
            store,
            crate::adapters::telemetry::TelemetryHandle::disabled(),
        )
        .await
    }
}
impl ToolService {
    pub async fn new(config: AppConfig, store: Store) -> Result<Self> {
        build_tools(
            config,
            store,
            crate::adapters::telemetry::TelemetryHandle::disabled(),
        )
        .await
    }
}
