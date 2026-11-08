//! Resolved provider clients and role-specific model selection.
use crate::{
    domain::Role,
    llm::{ModelConfig, ModelEngine},
};
use anyhow::Result;
use std::{collections::HashMap, sync::Arc};
#[derive(Clone)]
pub(crate) struct ModelCatalog {
    config: ModelConfig,
    engine: Arc<ModelEngine>,
    role_engines: HashMap<String, Arc<ModelEngine>>,
}
impl ModelCatalog {
    pub(crate) fn new(mut config: ModelConfig) -> Result<Self> {
        config.validate()?;
        let engine = Arc::new(ModelEngine::new(config.clone())?);
        config = engine.resolved_config().clone();
        let mut explicit: HashMap<String, Option<u64>> = HashMap::new();
        if let Some(map) = &config.role_models {
            for spec in map.values() {
                explicit.insert(spec.model.clone(), spec.max_tokens);
            }
        }
        if let Some(name) = &config.role_model {
            explicit.entry(name.clone()).or_insert(None);
        }
        let mut role_engines = HashMap::new();
        for (name, spec_max) in explicit {
            if role_engines.contains_key(&name) {
                continue;
            }
            let mut role = config.clone();
            role.role_model = None;
            role.role_models = None;
            role.model = name.clone();
            let max_tokens = spec_max.or(config.role_max_tokens);
            if let Some(max_tokens) = max_tokens {
                role.max_tokens = max_tokens;
            }
            role_engines.insert(name, Arc::new(ModelEngine::new(role)?));
        }
        Ok(Self {
            config,
            engine,
            role_engines,
        })
    }
    #[cfg(test)]
    pub(crate) fn with_root_engine(&self, engine: ModelEngine) -> Self {
        Self {
            config: engine.resolved_config().clone(),
            engine: Arc::new(engine),
            ..self.clone()
        }
    }
    pub(crate) fn config(&self) -> &ModelConfig {
        &self.config
    }
    pub(crate) fn engine_for(&self, role: Role) -> &ModelEngine {
        if let Some(name) = self.config.role_model_name(role)
            && let Some(engine) = self.role_engines.get(name)
        {
            return engine;
        }
        &self.engine
    }
}
