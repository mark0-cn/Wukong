//! Process-owned telemetry assembly and its bounded lifecycle.

mod config;
mod metrics;
mod relay;

pub use config::{BackendConfig, ObservabilityConfig, QueryConfig, Signal};
pub use metrics::{Metrics, RunMetricGuard};

use opentelemetry::{KeyValue, global, trace::TracerProvider as _};
use opentelemetry_otlp::{Protocol, WithExportConfig as _, WithHttpConfig as _};
use opentelemetry_sdk::{
    Resource,
    logs::SdkLoggerProvider,
    metrics::{PeriodicReader, SdkMeterProvider},
    propagation::TraceContextPropagator,
    trace::SdkTracerProvider,
};
use serde::Serialize;
use std::{
    path::Path,
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::{Duration, SystemTime},
};
use tracing_subscriber::{Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _};

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DeliveryHealth {
    pub healthy: Option<bool>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct WorkerRelayStatus {
    pub traces: DeliveryHealth,
    pub logs: DeliveryHealth,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct TelemetryStatus {
    pub activated: bool,
    pub traces: bool,
    pub metrics: bool,
    pub logs: bool,
    pub local_jsonl: bool,
    pub worker_relay: bool,
    pub worker_relay_status: WorkerRelayStatus,
    pub last_error: Option<String>,
}

#[derive(Default)]
struct RuntimeState {
    activated: bool,
    tracer: Option<SdkTracerProvider>,
    meter: Option<SdkMeterProvider>,
    logger: Option<SdkLoggerProvider>,
    log_writer: Option<tracing_appender::non_blocking::WorkerGuard>,
    relay: Option<relay::WorkerRelay>,
    shutdown_timeout_secs: u64,
    status: TelemetryStatus,
}

struct ShutdownParts {
    tracer: Option<SdkTracerProvider>,
    meter: Option<SdkMeterProvider>,
    logger: Option<SdkLoggerProvider>,
    writer: Option<tracing_appender::non_blocking::WorkerGuard>,
    relay: Option<relay::WorkerRelay>,
    timeout: Duration,
}

struct Inner {
    state: Mutex<RuntimeState>,
    metrics: OnceLock<Metrics>,
}

/// Main-owned telemetry lifetime. Clones handed to subsystems cannot shut it down.
pub struct TelemetryRuntime {
    inner: Arc<Inner>,
}

#[derive(Clone)]
pub struct TelemetryHandle {
    inner: Arc<Inner>,
}

impl Default for TelemetryRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl TelemetryRuntime {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(RuntimeState::default()),
                metrics: OnceLock::new(),
            }),
        }
    }

    pub fn handle(&self) -> TelemetryHandle {
        TelemetryHandle {
            inner: self.inner.clone(),
        }
    }

    /// Activate once after configuration is loaded. Failures are isolated by signal.
    pub fn activate(&self, config: &crate::config::AppConfig) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.activated {
            return;
        }
        state.activated = true;
        state.status.activated = true;
        state.shutdown_timeout_secs = config.observability.shutdown_timeout_secs;
        global::set_text_map_propagator(TraceContextPropagator::new());

        let observability = &config.observability;
        let resource = Resource::builder_empty()
            .with_service_name(
                std::env::var("OTEL_SERVICE_NAME")
                    .unwrap_or_else(|_| observability.service_name.clone()),
            )
            .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
            .build();

        if observability.traces && observability.signal_configured(Signal::Traces) {
            match build_tracer(observability, resource.clone()) {
                Ok(provider) => {
                    global::set_tracer_provider(provider.clone());
                    state.tracer = Some(provider);
                    state.status.traces = true;
                }
                Err(_) => {
                    state.status.last_error = Some("traces exporter initialization failed".into())
                }
            }
        }
        if observability.metrics && observability.signal_configured(Signal::Metrics) {
            match build_meter(observability, resource.clone()) {
                Ok(provider) => {
                    global::set_meter_provider(provider.clone());
                    state.meter = Some(provider);
                    state.status.metrics = true;
                }
                Err(_) => {
                    state.status.last_error = Some("metrics exporter initialization failed".into())
                }
            }
        }
        if observability.logs && observability.signal_configured(Signal::Logs) {
            match build_logger(observability, resource) {
                Ok(provider) => {
                    state.logger = Some(provider);
                    state.status.logs = true;
                }
                Err(_) => {
                    state.status.last_error = Some("logs exporter initialization failed".into())
                }
            }
        }
        if observability.local_jsonl {
            match jsonl_writer(&config.state_dir, observability.log_retention_days) {
                Ok((writer, guard)) => {
                    state.log_writer = Some(guard);
                    state.status.local_jsonl = true;
                    install_subscriber(&mut state, Some(writer));
                }
                Err(_) => {
                    state.status.last_error = Some("local JSONL initialization failed".into());
                    install_subscriber(&mut state, None);
                }
            }
        } else {
            install_subscriber(&mut state, None);
        }
        self.inner.metrics.get_or_init(Metrics::new);
        if observability.export_configured() && (observability.traces || observability.logs) {
            if tokio::runtime::Handle::try_current().is_ok() {
                // Relay initialization reports per-signal failures through the
                // same handle; do not hold the non-reentrant state lock across it.
                drop(state);
                let started = relay::WorkerRelay::start(observability, self.handle());
                state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match started {
                    Ok(relay) => {
                        state.relay = Some(relay);
                        state.status.worker_relay = true;
                    }
                    Err(_) => {
                        state.status.last_error =
                            Some("worker telemetry relay initialization failed".into());
                    }
                }
            } else {
                state.status.last_error =
                    Some("worker telemetry relay requires an async runtime".into());
            }
        }
    }

    /// Drain relay and providers within one total deadline. Export remains
    /// best-effort: a stuck backend can never hold process exit indefinitely.
    pub async fn shutdown(&self) {
        let mut parts = self.take_shutdown();
        let deadline = tokio::time::Instant::now() + parts.timeout;
        if let Some(relay) = parts.relay.as_mut()
            && tokio::time::timeout_at(deadline, relay.shutdown())
                .await
                .is_err()
        {
            relay.stop();
        }
        flush_blocking(parts, deadline).await;
    }

    fn take_shutdown(&self) -> ShutdownParts {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ShutdownParts {
            tracer: state.tracer.take(),
            meter: state.meter.take(),
            logger: state.logger.take(),
            writer: state.log_writer.take(),
            relay: state.relay.take(),
            timeout: Duration::from_secs(state.shutdown_timeout_secs.clamp(1, 30)),
        }
    }

    fn shutdown_sync(&self) {
        let parts = self.take_shutdown();
        if let Some(relay) = &parts.relay {
            relay.stop();
        }
        flush_on_thread(parts);
    }
}

async fn flush_blocking(parts: ShutdownParts, deadline: tokio::time::Instant) {
    if parts.tracer.is_none()
        && parts.meter.is_none()
        && parts.logger.is_none()
        && parts.writer.is_none()
    {
        return;
    }
    let mut task = tokio::task::spawn_blocking(move || flush_providers(parts));
    if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
        task.abort();
    }
}

fn flush_on_thread(parts: ShutdownParts) {
    if parts.tracer.is_none()
        && parts.meter.is_none()
        && parts.logger.is_none()
        && parts.writer.is_none()
    {
        return;
    }
    let timeout = parts.timeout;
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        flush_providers(parts);
        let _ = sender.send(());
    });
    let _ = receiver.recv_timeout(timeout);
}

fn flush_providers(parts: ShutdownParts) {
    if let Some(provider) = parts.tracer {
        let _ = provider.shutdown();
    }
    if let Some(provider) = parts.meter {
        let _ = provider.shutdown();
    }
    if let Some(provider) = parts.logger {
        let _ = provider.shutdown();
    }
    drop(parts.writer);
}

impl Drop for TelemetryRuntime {
    fn drop(&mut self) {
        self.shutdown_sync();
    }
}

impl TelemetryHandle {
    pub fn disabled() -> Self {
        TelemetryRuntime::new().handle()
    }

    pub fn metrics(&self) -> &Metrics {
        self.inner.metrics.get_or_init(Metrics::new)
    }

    pub fn status(&self) -> TelemetryStatus {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .status
            .clone()
    }

    pub fn dropped(&self, signal: &str, reason: &str) {
        self.metrics().dropped(signal, reason, 1);
    }

    pub fn enqueue_worker_batch(&self, batch: wukong_worker_protocol::TelemetryBatch) {
        let has_traces = batch.traces.is_some();
        let has_logs = batch.logs.is_some();
        let result = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .relay
            .as_ref()
            .map_or(Some(relay::EnqueueError::Closed), |relay| {
                relay.enqueue(batch).err()
            });
        if let Some(error) = result {
            let reason = match error {
                relay::EnqueueError::Full => "relay_queue_full",
                relay::EnqueueError::Closed => "relay_unavailable",
            };
            if has_traces {
                self.dropped("traces", reason);
            }
            if has_logs {
                self.dropped("logs", reason);
            }
        }
    }

    pub(crate) fn relay_result(&self, signal: &str, error: Option<String>) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let health = match signal {
            "traces" => &mut state.status.worker_relay_status.traces,
            "logs" => &mut state.status.worker_relay_status.logs,
            _ => return,
        };
        let healthy = error.is_none();
        health.healthy = Some(healthy);
        health.last_error = error.map(|error| error.chars().take(160).collect());
        tracing::info!(
            target: "wukong_telemetry",
            event = "worker_relay_finished",
            signal,
            outcome = if healthy { "ok" } else { "error" },
        );
    }
}

fn build_tracer(
    config: &ObservabilityConfig,
    resource: Resource,
) -> anyhow::Result<SdkTracerProvider> {
    let mut builder = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_headers(config.exporter_headers(Signal::Traces)?);
    if let Some(endpoint) = config.exporter_endpoint(Signal::Traces)? {
        builder = builder.with_endpoint(endpoint);
    }
    Ok(SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(builder.build()?)
        .build())
}

fn build_meter(
    config: &ObservabilityConfig,
    resource: Resource,
) -> anyhow::Result<SdkMeterProvider> {
    let mut builder = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_headers(config.exporter_headers(Signal::Metrics)?);
    if let Some(endpoint) = config.exporter_endpoint(Signal::Metrics)? {
        builder = builder.with_endpoint(endpoint);
    }
    let reader = PeriodicReader::builder(builder.build()?).build();
    Ok(SdkMeterProvider::builder()
        .with_resource(resource)
        .with_reader(reader)
        .build())
}

fn build_logger(
    config: &ObservabilityConfig,
    resource: Resource,
) -> anyhow::Result<SdkLoggerProvider> {
    let mut builder = opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_headers(config.exporter_headers(Signal::Logs)?);
    if let Some(endpoint) = config.exporter_endpoint(Signal::Logs)? {
        builder = builder.with_endpoint(endpoint);
    }
    Ok(SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(builder.build()?)
        .build())
}

fn telemetry_targets() -> tracing_subscriber::filter::Targets {
    tracing_subscriber::filter::Targets::new()
        .with_target("wukong_telemetry", tracing::Level::TRACE)
}

fn install_subscriber(
    state: &mut RuntimeState,
    jsonl: Option<tracing_appender::non_blocking::NonBlocking>,
) {
    let diagnostic_filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
    };
    let stderr = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .with_filter(diagnostic_filter());
    let jsonl = jsonl.map(|writer| {
        tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .with_writer(writer)
            .with_filter(telemetry_targets())
    });
    let traces = state.tracer.as_ref().map(|provider| {
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("wukong"))
            .with_filter(tracing_subscriber::EnvFilter::new(
                "wukong=trace,rig=trace,rig_core=trace,rig_agent=trace,rig_ecs=trace",
            ))
    });
    let logs = state.logger.as_ref().map(|provider| {
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(provider)
            .with_filter(telemetry_targets())
    });
    let result = tracing_subscriber::registry()
        .with(stderr)
        .with(jsonl)
        .with(traces)
        .with(logs)
        .try_init();
    if result.is_err() {
        state.status.last_error = Some("tracing subscriber was already installed".into());
    }
}

fn jsonl_writer(
    state_dir: &Path,
    retention_days: u64,
) -> anyhow::Result<(
    tracing_appender::non_blocking::NonBlocking,
    tracing_appender::non_blocking::WorkerGuard,
)> {
    let directory = state_dir.join("logs");
    std::fs::create_dir_all(&directory)?;
    prune_jsonl(&directory, retention_days);
    let appender = tracing_appender::rolling::daily(directory, "controller.jsonl");
    Ok(
        tracing_appender::non_blocking::NonBlockingBuilder::default()
            .buffered_lines_limit(4096)
            .lossy(true)
            .finish(appender),
    )
}

fn prune_jsonl(directory: &Path, retention_days: u64) {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(retention_days.saturating_mul(86_400)))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let owned = entry.file_name();
        let name = owned.to_string_lossy();
        if !name.starts_with("controller.jsonl")
            || !entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            continue;
        }
        if entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified < cutoff)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_relay_configuration_does_not_deadlock_activation() {
        let directory = tempfile::tempdir().expect("temporary telemetry state");
        let config = crate::config::AppConfig {
            state_dir: directory.path().to_path_buf(),
            observability: ObservabilityConfig {
                otlp_endpoint: Some("https://user:secret@example.com".into()),
                logs: true,
                local_jsonl: false,
                ..ObservabilityConfig::default()
            },
            ..crate::config::AppConfig::default()
        };
        let runtime = TelemetryRuntime::new();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let async_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("activation test runtime");
            async_runtime.block_on(async { runtime.activate(&config) });
            let _ = sender.send(());
        });
        assert!(
            receiver.recv_timeout(Duration::from_secs(2)).is_ok(),
            "telemetry activation deadlocked on relay initialization"
        );
        worker.join().expect("activation thread completes");
    }

    #[test]
    fn jsonl_accepts_only_curated_events_and_never_arbitrary_fields() {
        let directory = tempfile::tempdir().expect("temporary telemetry directory");
        let appender = tracing_appender::rolling::never(directory.path(), "diagnostics.jsonl");
        let (writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
            .lossy(false)
            .finish(appender);
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_writer(writer)
                .with_filter(telemetry_targets()),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                target: "wukong::unreviewed",
                event = "unsafe",
                secret = "sensitive-sentinel",
                target_path = "C:\\private\\challenge",
            );
            tracing::info!(
                target: "wukong_telemetry",
                event = "run_finished",
                run_id = "run-fixture",
                outcome = "ok",
            );
        });
        drop(guard);
        let content = std::fs::read_to_string(directory.path().join("diagnostics.jsonl"))
            .expect("flush curated JSONL");
        assert!(content.contains("run_finished"));
        assert!(!content.contains("sensitive-sentinel"));
        assert!(!content.contains("private"));
    }
}
