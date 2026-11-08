//! Shared assessment use cases. Frontends supply input and present results;
//! preparation, saved configuration, ownership and resource cleanup live here.
use crate::{
    config::AppConfig, coordinator::Coordinator, domain::*, runner::AssessmentRunner, store::Store,
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::path::Path;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AssessmentService {
    config: AppConfig,
    store: Store,
}

pub struct PreparedAssessment {
    runner: AssessmentRunner,
    run: RunRecord,
    budget: Option<Budget>,
}

impl PreparedAssessment {
    pub fn run(&self) -> &RunRecord {
        &self.run
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.runner.subscribe()
    }
    pub async fn drive(self, cancel: CancellationToken) -> Result<RunRecord> {
        let result = self
            .runner
            .drive_with_budget(&self.run.id, cancel, self.budget)
            .await;
        self.runner.tools.close().await;
        result
    }
}

impl AssessmentService {
    pub fn new(config: AppConfig, store: Store) -> Self {
        Self { config, store }
    }

    pub async fn start(&self, target: &Path, task: &str) -> Result<PreparedAssessment> {
        let target = std::fs::canonicalize(target).context("target must be an existing file")?;
        ensure!(target.is_file(), "target must be a file");
        ensure!(!task.trim().is_empty(), "task must not be empty");
        let runner = AssessmentRunner::new(self.config.clone(), self.store.clone()).await?;
        let created = runner.coordinator.create_run(
            &target,
            task,
            self.config.budget.clone(),
            serde_json::to_value(runner.config())?,
        );
        match created {
            Ok(run) => Ok(PreparedAssessment {
                runner,
                run,
                budget: None,
            }),
            Err(error) => {
                runner.tools.close().await;
                Err(error)
            }
        }
    }

    pub async fn resume(&self, run_id: &str, budget: Option<Budget>) -> Result<PreparedAssessment> {
        let run: RunRecord = self.store.load(run_id)?;
        ensure!(!run.status.terminal(), "terminal run cannot be resumed");
        let mut saved: AppConfig = serde_json::from_value(run.config.clone())
            .context("saved run configuration invalid")?;
        saved.state_dir = self.store.root().to_owned();
        saved.budget = budget.clone().unwrap_or_else(|| run.budget.clone());
        saved.validate()?;
        let runner = AssessmentRunner::new(saved, self.store.clone()).await?;
        // Applying a budget is a supervised write, after ownership is acquired.
        Ok(PreparedAssessment {
            runner,
            run,
            budget,
        })
    }

    pub fn pause(&self, run: &str, reason: &str) -> Result<()> {
        Coordinator::new(self.store.clone()).pause_budget(run, reason)
    }
    pub fn stop(&self, agent: &str, reason: &str) -> Result<Vec<String>> {
        Coordinator::new(self.store.clone()).stop(agent, reason)
    }
    pub fn message(
        &self,
        recipient: &str,
        content: &str,
        options: MessageOptions,
    ) -> Result<MessageReceipt> {
        Coordinator::new(self.store.clone()).send("operator", recipient, content, options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Provider;

    #[tokio::test]
    async fn resume_keeps_saved_identity_and_does_not_mutate_a_live_run() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path().join("state"))?;
        let mut config = AppConfig::default();
        config.model.provider = Provider::Ollama;
        config.model.base_url = Some("http://127.0.0.1:1".into());
        let target = dir.path().join("sample");
        std::fs::write(&target, b"fixture")?;
        let service = AssessmentService::new(config.clone(), store.clone());
        let prepared = service.start(&target, "fixture").await?;
        let run = prepared.run().clone();
        drop(prepared);
        service.pause(&run.id, "fixture")?;
        store.acquire_lease(&run.id, "other-controller", 20)?;
        let mut changed = config;
        changed.model.model = "different-default".into();
        let service = AssessmentService::new(changed, store.clone());
        let mut budget = run.budget.clone();
        budget.max_total_tokens += 10;
        let resumed = service.resume(&run.id, Some(budget)).await?;
        assert_eq!(resumed.runner.config().model.model, "gpt-5.2");
        assert_eq!(
            resumed.runner.config().model.base_url.as_deref(),
            Some("http://127.0.0.1:1")
        );
        assert!(resumed.drive(CancellationToken::new()).await.is_err());
        let after: RunRecord = store.require("run", &run.id)?;
        assert_eq!(after.status, Status::BudgetPaused);
        assert_eq!(after.budget.max_total_tokens, run.budget.max_total_tokens);
        assert!(
            service
                .start(&dir.path().join("missing"), "fixture")
                .await
                .is_err()
        );
        assert_eq!(store.runs()?.len(), 1);
        Ok(())
    }
}
