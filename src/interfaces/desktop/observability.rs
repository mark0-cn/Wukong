//! Fine-grained async Tauri commands over the native observability facade.
//!
//! The provider is injected by the desktop host so every command validates the
//! current host epoch and uses that same epoch's configuration and Store. No
//! query configuration or Store handle can be supplied by the webview.

use crate::{
    adapters::{
        observability::{
            self, GrafanaTarget, LocalActivity, LogBatch, MetricBatch, MetricName, Observability,
            ObservabilityStatus, PageRequest, RunObservability, RunRequest, Signal, TimeRange,
            Trace, TraceList,
        },
        telemetry::{QueryConfig, TelemetryStatus},
    },
    storage::Store,
};
use anyhow::Result;
use std::sync::Arc;
use tauri::State;

type Reply<T> = std::result::Result<T, crate::i18n::Message>;
type Provider =
    dyn Fn(u64) -> Result<(QueryConfig, Store, TelemetryStatus)> + Send + Sync + 'static;

#[derive(Debug, serde::Serialize)]
pub(super) struct DesktopObservabilityStatus {
    #[serde(flatten)]
    pub query: ObservabilityStatus,
    pub exporter: TelemetryStatus,
}

/// Host-owned access to the current epoch's read model. The host provider must
/// perform the same epoch/closing validation as other desktop commands.
pub(crate) struct ObservabilityState {
    provider: Arc<Provider>,
}

impl ObservabilityState {
    pub(crate) fn new(
        provider: impl Fn(u64) -> Result<(QueryConfig, Store, TelemetryStatus)> + Send + Sync + 'static,
    ) -> Self {
        Self {
            provider: Arc::new(provider),
        }
    }

    fn context(&self, epoch: u64) -> Result<(Observability, Store, TelemetryStatus)> {
        let (config, store, exporter) = (self.provider)(epoch)?;
        Ok((Observability::new(config)?, store, exporter))
    }
}

fn failure(error: impl Into<anyhow::Error>) -> crate::i18n::Message {
    crate::i18n::Message::from_error(error.into())
}

async fn require_run(store: Store, run: String) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || observability::require_run(&store, &run)).await?
}

#[tauri::command]
pub(super) async fn observability_status(
    state: State<'_, ObservabilityState>,
    epoch: u64,
) -> Reply<DesktopObservabilityStatus> {
    let (observability, _store, exporter) = state.context(epoch).map_err(failure)?;
    Ok(DesktopObservabilityStatus {
        query: observability.status(),
        exporter,
    })
}

#[tauri::command]
pub(super) async fn observability_run(
    state: State<'_, ObservabilityState>,
    epoch: u64,
    request: RunRequest,
) -> Reply<RunObservability> {
    let (observability, store, _exporter) = state.context(epoch).map_err(failure)?;
    observability.run(store, request).await.map_err(failure)
}

#[tauri::command]
pub(super) async fn observability_local_activity(
    state: State<'_, ObservabilityState>,
    epoch: u64,
    run: String,
    cursor: Option<String>,
    limit: usize,
) -> Reply<LocalActivity> {
    let (_observability, store, _exporter) = state.context(epoch).map_err(failure)?;
    tauri::async_runtime::spawn_blocking(move || {
        let after = cursor
            .as_deref()
            .unwrap_or("0")
            .parse::<i64>()
            .map_err(|_| anyhow::anyhow!("local activity cursor is invalid"))?;
        observability::local_activity(&store, &run, after, limit)
    })
    .await
    .map_err(failure)?
    .map_err(failure)
}

#[tauri::command]
pub(super) async fn observability_traces(
    state: State<'_, ObservabilityState>,
    epoch: u64,
    run: String,
    request: PageRequest,
) -> Reply<Signal<TraceList>> {
    let (observability, store, _exporter) = state.context(epoch).map_err(failure)?;
    require_run(store, run.clone()).await.map_err(failure)?;
    observability
        .search_traces(&run, request)
        .await
        .map_err(failure)
}

#[tauri::command]
pub(super) async fn observability_trace(
    state: State<'_, ObservabilityState>,
    epoch: u64,
    run: String,
    trace_id: String,
) -> Reply<Signal<Trace>> {
    let (observability, store, _exporter) = state.context(epoch).map_err(failure)?;
    require_run(store, run.clone()).await.map_err(failure)?;
    observability.trace(&run, &trace_id).await.map_err(failure)
}

#[tauri::command]
pub(super) async fn observability_logs(
    state: State<'_, ObservabilityState>,
    epoch: u64,
    run: String,
    trace_id: Option<String>,
    request: PageRequest,
) -> Reply<Signal<LogBatch>> {
    let (observability, store, _exporter) = state.context(epoch).map_err(failure)?;
    require_run(store, run.clone()).await.map_err(failure)?;
    observability
        .logs(&run, trace_id.as_deref(), request)
        .await
        .map_err(failure)
}

#[tauri::command]
pub(super) async fn observability_metrics(
    state: State<'_, ObservabilityState>,
    epoch: u64,
    run: String,
    metric: MetricName,
    range: TimeRange,
    limit: usize,
) -> Reply<Signal<MetricBatch>> {
    let (observability, store, _exporter) = state.context(epoch).map_err(failure)?;
    require_run(store, run.clone()).await.map_err(failure)?;
    observability
        .metrics(run, metric, range, limit)
        .await
        .map_err(failure)
}

#[tauri::command]
pub(super) async fn observability_grafana_link(
    state: State<'_, ObservabilityState>,
    epoch: u64,
    target: GrafanaTarget,
    range: TimeRange,
) -> Reply<Option<String>> {
    let (observability, store, _exporter) = state.context(epoch).map_err(failure)?;
    let run = match &target {
        GrafanaTarget::Traces { run } | GrafanaTarget::Logs { run, .. } => Some(run.clone()),
        GrafanaTarget::Metrics { .. } => None,
    };
    if let Some(run) = run {
        require_run(store, run).await.map_err(failure)?;
    }
    observability.grafana_link(target, range).map_err(failure)
}
