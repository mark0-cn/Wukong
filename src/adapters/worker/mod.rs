//! Credential-free transport to a persistent execution worker.
//!
//! The stdio bridge is only a transport. Its daemon and job runners survive this
//! client being dropped, and a lost transport is replaced transparently on the
//! next call; reconnect uses the same worker state directory.
use anyhow::{Context, Result, anyhow};
use jsonrpsee_core::client::{ClientT, Error as RpcError};
use serde_json::{Value, json};
#[cfg(windows)]
use std::path::PathBuf;
use std::{
    sync::{Arc, RwLock},
    time::Duration,
};
use stdio::StdioChannel;
use tokio::sync::Mutex;

mod launch;
pub mod protocol;
pub mod ssh;
mod stdio;

mod config;
pub use config::*;

#[cfg(test)]
mod tests;

pub(crate) fn default_docker_executable() -> String {
    #[cfg(windows)]
    {
        let candidate = PathBuf::from(
            std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into()),
        )
        .join("Docker/Docker/resources/bin/docker.exe");
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    "docker".into()
}

struct Session {
    channel: StdioChannel,
    poisoned: bool,
    identity: protocol::WorkerIdentity,
}

#[derive(Clone)]
pub struct RuntimeClient {
    session: Arc<Mutex<Session>>,
    start_admission: Arc<Mutex<()>>,
    config: RuntimeConfig,
    identity: Arc<RwLock<protocol::WorkerIdentity>>,
    lease: Option<protocol::EnvironmentLease>,
    telemetry: crate::adapters::telemetry::TelemetryHandle,
}

impl RuntimeClient {
    pub async fn connect(config: RuntimeConfig) -> Result<Self> {
        Self::connect_with_telemetry(
            config,
            crate::adapters::telemetry::TelemetryHandle::disabled(),
        )
        .await
    }

    pub async fn connect_with_telemetry(
        config: RuntimeConfig,
        telemetry: crate::adapters::telemetry::TelemetryHandle,
    ) -> Result<Self> {
        if let TransportConfig::TrustedLocal(local) = &config.transport {
            crate::adapters::files::owned::create_owned_directory(&local.state_dir)?;
        }
        let session = Session::connect(&config, telemetry.clone())
            .await
            .context("start runtime bridge transport")?;
        let identity = session.identity.clone();
        Ok(Self {
            session: Arc::new(Mutex::new(session)),
            start_admission: Arc::new(Mutex::new(())),
            config,
            identity: Arc::new(RwLock::new(identity)),
            lease: None,
            telemetry,
        })
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }
    pub fn identity(&self) -> protocol::WorkerIdentity {
        self.identity
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
    pub(crate) fn with_lease(&self, lease: protocol::EnvironmentLease) -> Self {
        Self {
            lease: Some(lease),
            ..self.clone()
        }
    }
    pub async fn doctor(&self) -> Result<Value> {
        self.call("doctor", json!({})).await
    }

    /// Calls are serialized over stdio. A lost transport is replaced
    /// transparently on the next call: the bridge daemon and job supervisors
    /// live in the shared state directory and survive the stdio client. The
    /// failed request itself is never re-sent, so a timed-out side effect
    /// still follows the durable reconcile path instead of being replayed.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        use tracing::Instrument as _;
        let started = std::time::Instant::now();
        let span = tracing::info_span!(
            "wukong.worker.rpc",
            rpc.method = method,
            worker.transport = self.config.backend().as_str(),
            outcome = tracing::field::Empty,
        );
        let mut retries = 0u64;
        let result = self
            .call_inner(method, params, &mut retries)
            .instrument(span.clone())
            .await;
        let outcome = if result.is_ok() { "ok" } else { "error" };
        span.record("outcome", outcome);
        tracing::info!(
            target: "wukong_telemetry",
            event = "worker_rpc_finished",
            rpc.method = method,
            worker.transport = self.config.backend().as_str(),
            outcome,
            retries,
            duration_seconds = started.elapsed().as_secs_f64(),
        );
        self.telemetry.metrics().worker_rpc(
            method,
            self.config.backend().as_str(),
            outcome,
            retries,
            started.elapsed().as_secs_f64(),
        );
        result
    }

    async fn call_inner(&self, method: &str, params: Value, retries: &mut u64) -> Result<Value> {
        let starts_resource = wukong_worker_protocol::Method::parse(method)?.resources()
            == wukong_worker_protocol::ResourceDemand::Execution;
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(self.config.request_timeout_secs);
        // Tokio's FIFO mutex orders new starts. Observation/close/lease calls do
        // not take this permit, so the current owner can free execution capacity.
        let _admission = if starts_resource {
            Some(
                tokio::time::timeout_at(deadline, self.start_admission.lock())
                    .await
                    .map_err(|_| protocol::WorkerError {
                        kind: protocol::WorkerErrorKind::ResourceBusy,
                        message: "worker capacity queue timed out before dispatch".into(),
                    })?,
            )
        } else {
            None
        };
        loop {
            let result = self.call_once(method, params.clone()).await;
            let capacity_busy = starts_resource
                && result.as_ref().err().is_some_and(|error| {
                    error
                        .downcast_ref::<protocol::WorkerError>()
                        .is_some_and(|error| error.kind == protocol::WorkerErrorKind::ResourceBusy)
                });
            if !capacity_busy || tokio::time::Instant::now() >= deadline {
                return result;
            }
            // Only a worker's explicit pre-effect rejection may be retried.
            // Transport failures and uncertain outcomes retain normal recovery.
            *retries = retries.saturating_add(1);
            tokio::time::sleep_until(
                deadline.min(tokio::time::Instant::now() + Duration::from_millis(250)),
            )
            .await;
            if tokio::time::Instant::now() >= deadline {
                return result;
            }
        }
    }

    async fn call_once(&self, method: &str, mut params: Value) -> Result<Value> {
        if method != "ping" && method != "doctor" {
            params["_binding"] = serde_json::to_value(self.identity())?;
            if let Some(lease) = &self.lease {
                params["_lease"] = serde_json::to_value(lease)?;
            }
        }
        let mut session = self.session.lock().await;
        if session.poisoned {
            *session = Session::connect(&self.config, self.telemetry.clone())
                .await
                .context("runtime transport lost and reconnect failed")?;
        }
        let identity = self.identity();
        if session.identity != identity {
            anyhow::ensure!(
                session.identity.same_installation(&identity),
                "worker installation, package or protocol identity changed; refusing execution in the previous binding"
            );
            *self
                .identity
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = session.identity.clone();
            if method != "ping" && method != "doctor" {
                params["_binding"] = serde_json::to_value(&session.identity)?;
            }
        }
        match request(&session.channel, method, params).await {
            Ok(result) => Ok(result),
            Err(RequestFailure::Worker(error)) => Err(error),
            Err(RequestFailure::Transport { timed_out, error }) => {
                session.poisoned = true;
                session.channel.close_after_failure().await;
                let message = if timed_out {
                    "runtime request timed out; the worker may have executed it — reconcile the durable call before retrying (the next call reconnects automatically)".to_owned()
                } else {
                    let diagnostic = session.channel.diagnostic(
                        "runtime transport failed; the next call reconnects automatically",
                    );
                    format!("{diagnostic}: {error:#}")
                };
                Err(protocol::TransportFailure { timed_out, message }.into())
            }
        }
    }

    pub async fn job_status(&self, job_id: &str) -> Result<protocol::JobStatus> {
        let status: protocol::JobStatus =
            serde_json::from_value(self.call("job.status", json!({"job_id":job_id})).await?)
                .context("invalid worker job status")?;
        anyhow::ensure!(status.job_id == job_id, "worker returned the wrong job");
        Ok(status)
    }

    /// Closes only the transport. To stop work, call job.cancel/debug.close first.
    pub async fn close(&self) -> Result<()> {
        let mut session = self.session.lock().await;
        session.poisoned = true;
        session
            .channel
            .close()
            .await
            .context("close runtime transport")
    }
}

impl Session {
    /// Each fresh channel handshakes before any business request can be sent.
    async fn connect(
        config: &RuntimeConfig,
        telemetry: crate::adapters::telemetry::TelemetryHandle,
    ) -> Result<Self> {
        let mut channel = StdioChannel::spawn(
            launch::command(&config.transport)?,
            config.request_timeout_secs,
            telemetry,
        )?;
        let reply = match request(&channel, "ping", json!({})).await {
            Ok(reply) => reply,
            Err(RequestFailure::Worker(error) | RequestFailure::Transport { error, .. }) => {
                channel.close_after_failure().await;
                return Err(error).context(channel.diagnostic("runtime handshake"));
            }
        };
        let identity: protocol::WorkerIdentity = serde_json::from_value(reply["identity"].clone())
            .context("worker handshake did not establish its identity")?;
        identity.validate()?;
        if let Some(managed) = config.managed_ssh() {
            anyhow::ensure!(
                identity.installation_id == managed.installation_id,
                "SSH worker installation differs from its enrolled native identity"
            );
        }
        Ok(Self {
            channel,
            identity,
            poisoned: false,
        })
    }
}

enum RequestFailure {
    /// The worker answered with an RPC error; the transport stays usable.
    Worker(anyhow::Error),
    /// The stdio channel is unusable and must be replaced.
    Transport {
        timed_out: bool,
        error: anyhow::Error,
    },
}

async fn request(
    channel: &StdioChannel,
    method: &str,
    params: Value,
) -> Result<Value, RequestFailure> {
    let params = params
        .as_object()
        .cloned()
        .ok_or_else(|| RequestFailure::Worker(anyhow!("runtime params must be an object")))?;
    match channel.client.request(method, params).await {
        Ok(result) => Ok(result),
        Err(RpcError::Call(error)) => Err(RequestFailure::Worker(
            protocol::WorkerError::from_rpc(
                format!("runtime {method}: {}", error.message()),
                error.data().map(|data| data.get()),
            )
            .into(),
        )),
        Err(error) => Err(RequestFailure::Transport {
            timed_out: matches!(error, RpcError::RequestTimeout),
            error: error.into(),
        }),
    }
}
