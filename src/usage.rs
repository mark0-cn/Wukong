//! Atomic token/cost accounting and its budget consequences.
use crate::{config::Pricing, coordinator::Coordinator, domain::*, store::Store};
use anyhow::{Context, Result};
use rig::completion::Usage;
use serde_json::json;
#[derive(Clone)]
pub(crate) struct UsageService {
    store: Store,
    pricing: Pricing,
}
impl UsageService {
    pub(crate) fn new(store: Store, pricing: Pricing) -> Self {
        Self { store, pricing }
    }
    pub(crate) fn with_store(&self, store: Store) -> Self {
        Self {
            store,
            ..self.clone()
        }
    }
    pub(crate) fn account_usage(
        &self,
        agent_id: &str,
        usage: Usage,
        model_turn: bool,
    ) -> Result<()> {
        let tokens = usage.total_tokens.or_else(|| {
            usage
                .input_tokens
                .zip(usage.output_tokens)
                .map(|(i, o)| i.saturating_add(o))
        });
        // The configured rates are flat estimates for all input/output.
        // Rig owns normalized cache counters and avoids double counting them.
        let pricing = rig_core::catalog::Pricing::new(
            self.pricing.input_usd_per_million,
            self.pricing.output_usd_per_million,
        )
        .with_cache_read(self.pricing.input_usd_per_million)
        .with_cache_write(self.pricing.input_usd_per_million);
        let mut priced_usage = usage;
        if pricing.input == 0.0 {
            priced_usage.input_tokens.get_or_insert(0);
        }
        if pricing.output == 0.0 {
            priced_usage.output_tokens.get_or_insert(0);
        }
        let estimate = pricing.cost(&priced_usage);
        let cost_known = estimate.as_ref().is_some_and(|cost| cost.is_complete());
        let cost = estimate.as_ref().map_or(0.0, |cost| cost.total);
        self.store.write(|tx| {
            let mut agent: AgentMetadata = tx.get_metadata("agent",agent_id)?.context("agent missing")?;
            let mut run: RunRecord = tx.get("run",&agent.run_id)?.context("run missing")?;
            agent.turns += usize::from(model_turn);
            agent.total_tokens = agent.total_tokens.saturating_add(tokens.unwrap_or(0));
            run.total_tokens = run.total_tokens.saturating_add(tokens.unwrap_or(0));
            if cost_known { agent.cost_usd += cost; run.cost_usd += cost; }
            agent.updated_at = now(); run.updated_at = now();
            tx.put_metadata("agent",agent_id,&run.id,&agent)?;
            tx.put("run",&run.id,&run.id,&run)?;
            tx.event(&run.id,Some(agent_id),if model_turn {"model.usage"} else {"model.usage_aux"},json!({"usage":usage,"cost_estimate_usd":cost_known.then_some(cost),"tokens_reported":tokens.is_some()}))?;
            Ok(())
        })?;
        // A response without usage cannot be costed when pricing rates are
        // configured: pause exactly then, keeping cost accounting exact.
        // With flat zero pricing the cost is complete ($0) and only the
        // token count drifts; the event above records the omission, and the
        // kernel budget gate keeps enforcing on reported usage.
        if !cost_known {
            let agent: AgentMetadata = self.store.require_metadata("agent", agent_id)?;
            Coordinator::new(self.store.clone()).pause_budget(
                &agent.run_id,
                "provider omitted usage required for accounting; response retained in Rig journal",
            )?;
        }
        Ok(())
    }
}
