//! Durable limits for changed-request recovery; Rig owns physical retries.
use crate::{domain::AgentMetadata, storage::Store};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Attempts {
    pub count: u32,
    effects: Vec<String>,
}

pub(crate) fn attempts(store: &Store, step: &str) -> Result<u32> {
    Ok(store
        .get::<Attempts>("model_recovery", step)?
        .unwrap_or_default()
        .count)
}

pub(crate) fn claim(
    store: &Store,
    agent: &AgentMetadata,
    step: &str,
    effect: &str,
    limit: u32,
) -> Result<bool> {
    store.write(|tx| {
        let mut state = tx
            .get::<Attempts>("model_recovery", step)?
            .unwrap_or_default();
        if state.effects.iter().any(|id| id == effect) {
            return Ok(true);
        }
        if state.count >= limit {
            return Ok(false);
        }
        state.count += 1;
        state.effects.push(effect.into());
        tx.put("model_recovery", step, &agent.run_id, &state)?;
        tx.event(
            &agent.run_id,
            Some(&agent.id),
            "model.recovery",
            json!({
                "logical_step":step,"attempt":state.count,"effect_id":effect,
                "classification":"reasoning_only_output_limit","action":"changed_request"
            }),
        )?;
        Ok(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{application::coordinator::Coordinator, domain::Budget};
    #[test]
    fn recovery_limit_and_effect_identity_survive_reopening() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("state");
        let target = dir.path().join("target");
        std::fs::write(&target, b"inert fixture")?;
        let store = Store::open(&path)?;
        let run = Coordinator::new(store.clone()).create_run(
            &target,
            "fixture",
            Budget::default(),
            json!({}),
        )?;
        let agent = store.load(&run.root_id)?;
        assert!(claim(&store, &agent, "step", "first", 1)?);
        assert!(claim(&store, &agent, "step", "first", 1)?);
        drop(store);
        let store = Store::open(path)?;
        assert_eq!(attempts(&store, "step")?, 1);
        assert!(!claim(&store, &agent, "step", "second", 1)?);
        assert!(claim(&store, &agent, "next-step", "third", 1)?);
        Ok(())
    }
}
