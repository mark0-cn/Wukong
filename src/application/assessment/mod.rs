//! Shared assessment use cases. Frontends supply input and present results;
//! preparation, saved configuration, ownership and resource cleanup live here.
pub mod runner;
pub(crate) mod supervisor;
use crate::{
    adapters::telemetry::TelemetryHandle, application::assessment::runner::AssessmentRunner,
    application::coordinator::Coordinator, config::AppConfig, domain::*, storage::Store,
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::path::Path;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// Construct a runner only after the use case selects and validates its config.
pub(crate) type RunnerFactory = fn(
    AppConfig,
    Store,
    TelemetryHandle,
)
    -> futures::future::BoxFuture<'static, Result<AssessmentRunner>>;

#[derive(Clone)]
pub struct AssessmentService {
    config: AppConfig,
    store: Store,
    runner_factory: RunnerFactory,
    telemetry: TelemetryHandle,
}

pub struct PreparedAssessment {
    _maintenance: crate::application::maintenance::updates::maintenance::MaintenanceLock,
    runner: AssessmentRunner,
    run: RunRecord,
    budget: Option<Budget>,
    telemetry: TelemetryHandle,
}

impl PreparedAssessment {
    pub fn run(&self) -> &RunRecord {
        &self.run
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.runner.subscribe()
    }
    pub async fn drive(self, cancel: CancellationToken) -> Result<RunRecord> {
        use tracing::Instrument as _;
        let started = std::time::Instant::now();
        let active = self.telemetry.metrics().run_started();
        let span = tracing::info_span!(
            "wukong.assessment",
            run_id = %self.run.id,
            outcome = tracing::field::Empty,
        );
        tracing::info!(
            target: "wukong_telemetry",
            event = "run_started",
            run_id = %self.run.id,
        );
        // The future can be dropped by an aborted task, and ToolService has no Drop of
        // its own, so teardown belongs to a guard rather than the happy path.
        struct CloseTools(std::sync::Arc<crate::application::tools::ToolService>);
        impl Drop for CloseTools {
            fn drop(&mut self) {
                let tools = std::sync::Arc::clone(&self.0);
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move { tools.close().await });
                }
            }
        }
        let _close = CloseTools(std::sync::Arc::clone(&self.runner.tools));
        let result = self
            .runner
            .drive_with_budget(&self.run.id, cancel, self.budget)
            .instrument(span.clone())
            .await;
        let outcome = match &result {
            Ok(run) => run.status.as_str(),
            Err(_) => "error",
        };
        span.record("outcome", outcome);
        tracing::info!(
            target: "wukong_telemetry",
            event = "run_finished",
            run_id = %self.run.id,
            outcome,
            duration_seconds = started.elapsed().as_secs_f64(),
        );
        self.telemetry
            .metrics()
            .run_finished(outcome, started.elapsed().as_secs_f64());
        drop(active);
        result
    }
}

impl AssessmentService {
    pub(crate) fn from_dependencies(
        config: AppConfig,
        store: Store,
        runner_factory: RunnerFactory,
        telemetry: TelemetryHandle,
    ) -> Self {
        Self {
            config,
            store,
            runner_factory,
            telemetry,
        }
    }

    pub async fn start(&self, target: &Path, task: &str) -> Result<PreparedAssessment> {
        self.start_with_budget(target, task, None).await
    }

    pub async fn start_with_budget(
        &self,
        target: &Path,
        task: &str,
        budget: Option<Budget>,
    ) -> Result<PreparedAssessment> {
        let maintenance =
            crate::application::maintenance::updates::maintenance::assessment_guard()?;
        let target = std::fs::canonicalize(target)
            .context("target must be an existing file or directory")?;
        ensure!(
            target.is_file() || target.is_dir(),
            "target must be a file or directory"
        );
        ensure!(!task.trim().is_empty(), "task must not be empty");
        let mut config = self.config.clone();
        config.output_language.get_or_insert_with(|| {
            crate::i18n::Locale::for_task(task, crate::i18n::Locale::system())
        });
        config.budget = budget.unwrap_or_else(|| self.config.budget.clone());
        config.validate()?;
        let effective_budget = config.budget.clone();
        let runner =
            (self.runner_factory)(config, self.store.clone(), self.telemetry.clone()).await?;
        let created = runner.coordinator.create_run(
            &target,
            task,
            effective_budget,
            serde_json::to_value(runner.config())?,
        );
        match created {
            Ok(run) => Ok(PreparedAssessment {
                _maintenance: maintenance,
                runner,
                run,
                budget: None,
                telemetry: self.telemetry.clone(),
            }),
            Err(error) => {
                runner.tools.close().await;
                Err(error)
            }
        }
    }

    pub async fn resume(&self, run_id: &str, budget: Option<Budget>) -> Result<PreparedAssessment> {
        let maintenance =
            crate::application::maintenance::updates::maintenance::assessment_guard()?;
        let run: RunRecord = self.store.load(run_id)?;
        ensure!(!run.status.terminal(), "terminal run cannot be resumed");
        let mut saved: AppConfig = serde_json::from_value(run.config.clone())
            .context("saved run configuration invalid")?;
        saved.state_dir = self.store.root().to_owned();
        saved.budget = budget.clone().unwrap_or_else(|| run.budget.clone());
        saved.validate()?;
        let runner =
            (self.runner_factory)(saved, self.store.clone(), self.telemetry.clone()).await?;
        // Keep recovery work out of the CLI's main stack frame. An inline
        // reconcile future overflowed the default Windows stack even for --version.
        if let Err(error) = Box::pin(runner.tools.reconcile_jobs(run_id)).await {
            runner.tools.close().await;
            return Err(error.context("reconcile worker jobs before resuming"));
        }
        // Applying a budget is a supervised write, after ownership is acquired.
        Ok(PreparedAssessment {
            _maintenance: maintenance,
            runner,
            run,
            budget,
            telemetry: self.telemetry.clone(),
        })
    }

    pub fn pause(&self, run: &str, reason: &str) -> Result<()> {
        Coordinator::new(self.store.clone()).pause_operator(run, reason)
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
    use crate::adapters::models::Provider;

    #[tokio::test]
    async fn start_with_budget_persists_the_effective_new_run_limit() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path().join("state"))?;
        let mut config = AppConfig::default();
        config.model.provider = Provider::Ollama;
        config.model.base_url = Some("http://127.0.0.1:1".into());
        let target = dir.path().join("sample");
        std::fs::write(&target, b"fixture")?;
        let service =
            AssessmentService::new(config.clone(), store.clone(), TelemetryHandle::disabled());
        let mut budget = config.budget.clone();
        budget.max_total_tokens = 123_456;
        let prepared = service
            .start_with_budget(&target, "fixture", Some(budget))
            .await?;
        let run = prepared.run();
        assert_eq!(run.budget.max_total_tokens, 123_456);
        let saved: AppConfig = serde_json::from_value(run.config.clone())?;
        assert_eq!(saved.budget.max_total_tokens, 123_456);
        Ok(())
    }

    #[tokio::test]
    async fn resume_keeps_saved_identity_and_does_not_mutate_a_live_run() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path().join("state"))?;
        let mut config = AppConfig::default();
        config.model.provider = Provider::Ollama;
        config.model.base_url = Some("http://127.0.0.1:1".into());
        config.output_language = Some(crate::i18n::Locale::EnUs);
        let target = dir.path().join("sample");
        std::fs::write(&target, b"fixture")?;
        let service =
            AssessmentService::new(config.clone(), store.clone(), TelemetryHandle::disabled());
        let prepared = service.start(&target, "fixture").await?;
        let run = prepared.run().clone();
        drop(prepared);
        service.pause(&run.id, "fixture")?;
        store.acquire_lease(&run.id, "other-controller", 20)?;
        let mut changed = config;
        changed.model.model = "different-default".into();
        changed.output_language = Some(crate::i18n::Locale::ZhCn);
        let service = AssessmentService::new(changed, store.clone(), TelemetryHandle::disabled());
        let mut budget = run.budget.clone();
        budget.max_total_tokens += 10;
        let resumed = service.resume(&run.id, Some(budget)).await?;
        assert_eq!(resumed.runner.config().model.model, "gpt-5.2");
        assert_eq!(
            resumed.runner.config().output_language,
            Some(crate::i18n::Locale::EnUs)
        );
        assert_eq!(resumed.run.config["output_language"], "en-US");
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
