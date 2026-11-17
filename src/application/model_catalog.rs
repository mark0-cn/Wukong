//! Resolved provider clients and role-specific model selection.
use crate::{
    adapters::models::{ModelConfig, ModelEngine},
    domain::Role,
};
use anyhow::Result;
use std::{collections::HashMap, sync::Arc};
#[derive(Clone)]
pub(crate) struct ModelCatalog {
    config: ModelConfig,
    engine: Arc<ModelEngine>,
    role_engines: HashMap<Role, Arc<ModelEngine>>,
}
impl ModelCatalog {
    pub(crate) fn new(mut config: ModelConfig) -> Result<Self> {
        config.validate()?;
        let engine = Arc::new(ModelEngine::new(config.clone())?);
        config = engine.resolved_config().clone();
        // Resolve per role, not per model name: two roles may name the same model
        // with different token limits, and keying engines by name alone kept only
        // one of the two limits — whichever the map happened to yield last.
        // Roles that resolve to the same model and limit share one engine.
        let mut shared: HashMap<(String, u64), Arc<ModelEngine>> = HashMap::new();
        let mut role_engines = HashMap::new();
        for role in [Role::Analyst, Role::Verifier] {
            if config.role_model_name(role).is_none() {
                continue;
            }
            let resolved = config.role_config_for(role);
            let key = (resolved.model.clone(), resolved.max_tokens);
            let role_engine = match shared.get(&key) {
                Some(existing) => Arc::clone(existing),
                None => {
                    let created = Arc::new(ModelEngine::new(resolved)?);
                    shared.insert(key, Arc::clone(&created));
                    created
                }
            };
            role_engines.insert(role, role_engine);
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
        self.role_engines.get(&role).unwrap_or(&self.engine)
    }
}
