//! Owns a controller lifetime independently of Rig frames and worker latency.
use crate::{
    domain::*,
    runner::AssessmentRunner,
    store::{LeaseToken, Store},
};
use anyhow::{Context, Result};
#[cfg(test)]
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub(crate) struct RunSupervisor {
    store: Store,
    lease: LeaseToken,
    cancel: CancellationToken,
    tasks: Vec<tokio::task::AbortHandle>,
}

impl Drop for RunSupervisor {
    fn drop(&mut self) {
        self.cancel.cancel();
        for task in &self.tasks {
            task.abort();
        }
        // The token-specific release cannot delete a successor's lease. Any
        // still-unwinding execution is fenced once this lease is released.
        let _ = self.store.release_token(&self.lease);
    }
}

impl RunSupervisor {
    pub(crate) async fn drive(
        runner: AssessmentRunner,
        run_id: &str,
        cancel: CancellationToken,
        budget: Option<Budget>,
    ) -> Result<RunRecord> {
        let lease = runner
            .store
            .claim_lease(run_id, &id("controller"), 20)?
            .context("another controller owns this run; wait for its live lease to expire")?;
        let mut supervisor = Self {
            store: runner.store.clone(),
            lease: lease.clone(),
            cancel: cancel.child_token(),
            tasks: vec![],
        };
        let runner = runner.with_store(runner.store.fenced(lease.clone()));
        if let Some(budget) = budget {
            runner.coordinator.resume(run_id, Some(budget))?;
        }
        let mut heartbeat = supervisor.heartbeat(runner.clone());
        supervisor.tasks.push(heartbeat.abort_handle());
        let monitor_runner = runner.clone();
        let monitor_run = run_id.to_owned();
        let monitor_cancel = supervisor.cancel.clone();
        let monitor = tokio::spawn(async move {
            let jobs = monitor_runner.tools.jobs();
            let mut timer = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    biased;
                    _ = monitor_cancel.cancelled() => break,
                    _ = timer.tick() => {
                        tokio::select! {
                            _ = monitor_cancel.cancelled() => break,
                            result = jobs.refresh(&monitor_run) => {
                                if let Err(error) = result { tracing::warn!(%error, "job monitor failed"); }
                            }
                        }
                    }
                }
            }
        });
        supervisor.tasks.push(monitor.abort_handle());
        let runtime = tokio::runtime::Handle::current();
        let run = run_id.to_owned();
        let owner = lease.owner.clone();
        let kernel_cancel = supervisor.cancel.clone();
        let mut kernel = tokio::task::spawn_blocking(move || {
            crate::kernel::drive(runner, &run, &owner, kernel_cancel, runtime)
        });
        tokio::select! {
            result = &mut kernel => result.context("Rig kernel thread failed")?,
            result = &mut heartbeat => {
                supervisor.cancel.cancel();
                let _ = kernel.await;
                result.context("controller supervision failed")??;
                anyhow::bail!("controller supervision ended unexpectedly")
            }
        }
    }

    fn heartbeat(&self, runner: AssessmentRunner) -> JoinHandle<Result<()>> {
        let base = self.store.clone();
        let lease = self.lease.clone();
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            let mut renewed = Instant::now();
            let mut timer = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => std::future::pending::<()>().await,
                    _ = timer.tick() => {
                        if renewed.elapsed() >= Duration::from_secs(5) {
                            base.renew_lease(&lease, 20)?;
                            renewed = Instant::now();
                        }
                        let run: RunRecord = runner.store.require("run", &lease.run_id)?;
                        if !run.status.terminal() && run.status != Status::BudgetPaused
                            && started.elapsed().as_secs() >= run.budget.max_wall_seconds {
                            runner.coordinator.pause_budget(&run.id, "wall-clock budget reached")?;
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::AppConfig, llm::Provider, runtime::RuntimeBackend};

    #[tokio::test]
    async fn stalled_worker_does_not_block_renewal_cancellation_or_wall_budget() -> Result<()> {
        for wall_budget in [false, true] {
            let directory = tempfile::tempdir()?;
            let root = directory.path();
            let bridge = root.join("bridge.py");
            std::fs::write(
                &bridge,
                r#"import json,sys,time,pathlib
root=pathlib.Path(sys.argv[sys.argv.index('--state')+1]);root.mkdir(exist_ok=True)
for line in sys.stdin:
 r=json.loads(line)
 if r['method']=='job.status':
  (root/'polling').write_text('ready');time.sleep(60)
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':{}}),flush=True)
"#,
            )?;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let mut config = AppConfig {
                state_dir: root.join("state"),
                ..AppConfig::default()
            };
            config.model.provider = Provider::Ollama;
            config.model.base_url = Some(format!("http://{}", listener.local_addr()?));
            config.execution.stream = false;
            config.runtime.backend = RuntimeBackend::TrustedLocal;
            config.runtime.bridge_path = bridge;
            config.runtime.state_dir = root.join("worker");
            if wall_budget {
                config.budget.max_wall_seconds = 1;
            }
            let server = tokio::spawn(async move {
                let (_socket, _) = listener.accept().await.unwrap();
                std::future::pending::<()>().await;
            });
            let store = Store::open(&config.state_dir)?;
            let runner = AssessmentRunner::new(config.clone(), store.clone()).await?;
            let target = root.join("sample");
            std::fs::write(&target, b"fixture")?;
            let run =
                runner
                    .coordinator
                    .create_run(&target, "supervision", config.budget, json!({}))?;
            store.put(
                "job",
                "job",
                &run.id,
                &json!({"id":"job","run_id":run.id,"agent_id":run.root_id,"owner_generation":0,"worker_id":runner.tools.worker_identity()?,"status":{"job_id":"job","state":"queued"}}),
            )?;
            let cancel = CancellationToken::new();
            let child_cancel = cancel.clone();
            let run_id = run.id.clone();
            let task = tokio::spawn(async move { runner.drive(&run_id, child_cancel).await });
            tokio::time::timeout(Duration::from_secs(3), async {
                while !root.join("worker/polling").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await?;
            if !wall_budget {
                tokio::time::sleep(Duration::from_secs(6)).await;
                let remaining: i64 = store.read_transaction(|tx| {
                    Ok(tx.query_row(
                        "SELECT expires_at-strftime('%s','now') FROM leases WHERE run_id=?1",
                        [&run.id],
                        |row| row.get(0),
                    )?)
                })?;
                assert!(
                    remaining >= 17,
                    "lease was not renewed during worker IO: {remaining}"
                );
                cancel.cancel();
            }
            let result = tokio::time::timeout(Duration::from_secs(3), task).await???;
            assert_eq!(result.status, Status::BudgetPaused);
            assert!(store.acquire_lease(&run.id, "next-controller", 20)?);
            server.abort();
        }
        Ok(())
    }
}
