//! Worker-backed execution for packaged user-mode harnesses.
use crate::adapters::environments::EnvironmentRegistry;
use crate::adapters::worker::RuntimeClient;
use crate::application::windows_lab::runner::{LabWorker, LabWorkerOutcome, LabWorkerRequest};
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use base64::Engine;
use serde_json::json;
use std::time::{Duration, Instant};

const MAX_UPLOAD_BYTES: usize = 32 * 1024 * 1024;
const OUTPUT_LIMIT: u64 = 1024 * 1024;

pub struct WindowsWorker {
    registry: EnvironmentRegistry,
    environment_id: String,
}

impl WindowsWorker {
    pub fn new(registry: EnvironmentRegistry, environment_id: impl Into<String>) -> Self {
        Self {
            registry,
            environment_id: environment_id.into(),
        }
    }

    async fn upload(
        &self,
        client: &RuntimeClient,
        path: &str,
        bytes: &[u8],
        executable: bool,
    ) -> Result<String> {
        ensure!(
            bytes.len() <= MAX_UPLOAD_BYTES,
            "worker upload exceeds the 32 MiB single-request bound"
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let reply = client
            .call(
                "file.put",
                json!({"path": path, "data_base64": encoded, "executable": executable}),
            )
            .await?;
        reply["path"]
            .as_str()
            .map(str::to_owned)
            .context("worker file.put did not return a path")
    }
}

#[async_trait]
impl LabWorker for WindowsWorker {
    async fn doctor(&self) -> Result<serde_json::Value> {
        self.registry.doctor(&self.environment_id).await
    }
    async fn refresh(&self) -> Result<serde_json::Value> {
        self.registry.refresh(&self.environment_id).await
    }
    async fn maintenance(
        &self,
    ) -> Result<Box<dyn crate::application::windows_lab::runner::LabMaintenance>> {
        Ok(Box::new(WorkerMaintenance(
            self.registry.maintenance(&self.environment_id).await?,
        )))
    }
    async fn execute(&self, request: LabWorkerRequest) -> Result<LabWorkerOutcome> {
        let run = request.run_id.clone();
        let owner = request.owner.clone();
        let execution = self.execute_inner(request);
        tokio::pin!(execution);
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        let result = loop {
            tokio::select! {
                result = &mut execution => break result,
                _ = heartbeat.tick() => { self.registry.renew_leases(&run, &owner).await?; }
            }
        };
        // This CLI adapter owns only this standalone scope. Shared assessment
        // execution uses the ordinary JobService adapter instead.
        if result.as_ref().is_ok_and(|outcome| outcome.settled) {
            self.registry
                .release_scope(&self.environment_id, &run, &owner)
                .await?;
        }
        result
    }
}

impl WindowsWorker {
    async fn execute_inner(&self, request: LabWorkerRequest) -> Result<LabWorkerOutcome> {
        let client = self
            .registry
            .bind(&self.environment_id, &request.run_id, Some(&request.owner))
            .await?;
        let executable = self
            .upload(
                &client,
                &format!("lab/{}/{}", request.run_id, request.artifact_name),
                &request.executable,
                true,
            )
            .await?;
        let stdin = match &request.stdin {
            Some(bytes) => Some(
                self.upload(
                    &client,
                    &format!("lab/{}/stdin.bin", request.run_id),
                    bytes,
                    false,
                )
                .await?,
            ),
            None => None,
        };
        let mut params = json!({
            "argv": [executable],
            "request_id": format!("lab-{}", request.run_id),
            "timeout_secs": request.timeout_secs,
            "output_limit": OUTPUT_LIMIT,
        });
        if let Some(stdin) = &stdin {
            params["input"] = json!(stdin);
        }
        let started = client.call("job.start", params).await?;
        let job_id = started["job_id"]
            .as_str()
            .context("worker job.start did not return a job id")?
            .to_owned();
        let deadline = Instant::now() + Duration::from_secs(request.timeout_secs + 60);
        let status = loop {
            let status = client.call("job.status", json!({"job_id": job_id})).await?;
            let observed: crate::adapters::worker::protocol::JobStatus =
                serde_json::from_value(status.clone())?;
            if observed.settled() || Instant::now() >= deadline {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        };
        let state = status["state"].as_str().unwrap_or("unknown").to_owned();
        ensure!(
            matches!(
                state.as_str(),
                "completed" | "failed" | "cancelled" | "timed_out" | "lost"
            ),
            "harness job {job_id} did not reach a terminal state before the poll deadline"
        );
        let stdout = status["stdout"].as_str().unwrap_or_default().to_owned();
        let stderr = status["stderr"].as_str().unwrap_or_default().to_owned();
        Ok(LabWorkerOutcome {
            environment_id: self.environment_id.clone(),
            installation_id: client.identity().installation_id.clone(),
            job_id,
            state,
            settled: serde_json::from_value::<crate::adapters::worker::protocol::JobStatus>(
                status.clone(),
            )?
            .settled(),
            exit_code: status["result"]["exit_code"].as_i64(),
            executable_path: Some(executable),
            stdout_bytes: status["stdout_bytes"]
                .as_u64()
                .unwrap_or(stdout.len() as u64),
            stderr_bytes: status["stderr_bytes"]
                .as_u64()
                .unwrap_or(stderr.len() as u64),
            stdout,
            stderr,
        })
    }
}

struct WorkerMaintenance(crate::adapters::environments::MaintenanceLease);
#[async_trait]
impl crate::application::windows_lab::runner::LabMaintenance for WorkerMaintenance {
    async fn finish(self: Box<Self>) -> Result<()> {
        self.0.finish().await
    }
}
