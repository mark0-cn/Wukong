//! Native, read-only observability queries. The webview supplies intent, never
//! backend URLs, headers, LogQL, PromQL, or arbitrary local paths.

mod backend;
mod local;
mod loki;
mod prometheus;
mod tempo;

pub use backend::{BackendError, BackendErrorKind};
pub use local::{
    LocalActivity, LocalActivityKind, LocalAgent, LocalEvent, LocalRun, ResourceCounts, ToolCounts,
};
pub use loki::{LogBatch, LogEntry, LogStreamLabels};
pub use prometheus::{MetricBatch, MetricName, MetricPoint, MetricSeries};
pub use tempo::{Trace, TraceList, TraceSpan, TraceSummary};

use crate::{adapters::telemetry::QueryConfig, storage::Store};
use anyhow::{Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const MAX_QUERY_LIMIT: usize = 500;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageRequest {
    pub range: TimeRange,
    pub limit: usize,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRequest {
    pub run: String,
    pub range: TimeRange,
    pub limit: usize,
    pub trace_cursor: Option<String>,
    pub log_cursor: Option<String>,
    pub local_cursor: Option<String>,
    pub metric: MetricName,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackendCapability {
    pub configured: bool,
    pub origin: Option<String>,
    pub credential_available: bool,
}

impl BackendCapability {
    fn unconfigured() -> Self {
        Self {
            configured: false,
            origin: None,
            credential_available: false,
        }
    }

    fn configured(origin: &str, credential_available: bool) -> Self {
        Self {
            configured: true,
            origin: Some(origin.into()),
            credential_available,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BackendCapabilities {
    pub tempo: BackendCapability,
    pub loki: BackendCapability,
    pub prometheus: BackendCapability,
    pub grafana: BackendCapability,
}

#[derive(Debug, Clone, Serialize)]
pub struct ObservabilityStatus {
    pub capabilities: BackendCapabilities,
    pub queried_at: DateTime<Utc>,
}

/// Every external panel has independent lifecycle metadata. A failed backend is
/// represented here rather than returned as a command-wide error.
#[derive(Debug, Clone, Serialize)]
pub struct Signal<T> {
    pub configured: bool,
    pub partial: bool,
    pub truncated: bool,
    pub stale: bool,
    pub queried_at: DateTime<Utc>,
    pub error: Option<BackendError>,
    pub data: Option<T>,
}

impl<T> Signal<T> {
    fn unconfigured() -> Self {
        Self {
            configured: false,
            partial: false,
            truncated: false,
            stale: false,
            queried_at: Utc::now(),
            error: None,
            data: None,
        }
    }

    fn failure(error: BackendError) -> Self {
        Self {
            configured: true,
            partial: true,
            truncated: false,
            stale: false,
            queried_at: Utc::now(),
            error: Some(error),
            data: None,
        }
    }

    fn success(data: T, truncated: bool) -> Self {
        Self {
            configured: true,
            partial: false,
            truncated,
            stale: false,
            queried_at: Utc::now(),
            error: None,
            data: Some(data),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RunObservability {
    pub local: LocalActivity,
    pub traces: Signal<TraceList>,
    pub logs: Signal<LogBatch>,
    /// Metrics are service-scoped because business identifiers are deliberately
    /// not metric labels. The run argument only validates the desktop context.
    pub metrics: Signal<MetricBatch>,
}

pub fn local_activity(store: &Store, run: &str, after: i64, limit: usize) -> Result<LocalActivity> {
    local::query(store, run, after, limit)
}

pub fn require_run(store: &Store, run: &str) -> Result<()> {
    validate_run_id(run)?;
    let record = store
        .get_record::<crate::domain::RunRecord>(run)?
        .ok_or_else(|| anyhow::anyhow!("run does not exist"))?;
    ensure!(record.id == run, "run ownership mismatch");
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum GrafanaTarget {
    Traces {
        run: String,
    },
    Logs {
        run: String,
        trace_id: Option<String>,
    },
    Metrics {
        metric: MetricName,
    },
}

#[derive(Clone)]
pub struct Observability {
    config: QueryConfig,
    tempo: Option<tempo::Tempo>,
    loki: Option<loki::Loki>,
    prometheus: Option<prometheus::Prometheus>,
    grafana: Option<reqwest::Url>,
}

impl Observability {
    pub fn new(config: QueryConfig) -> Result<Self> {
        ensure!(
            (60..=31 * 86_400).contains(&config.max_range_secs()),
            "observability query range limit must be 60 seconds–31 days"
        );
        let timeout = config.timeout_secs();
        ensure!(
            (1..=60).contains(&timeout),
            "query timeout must be 1–60 seconds"
        );
        let tempo = config
            .tempo
            .as_ref()
            .map(|backend| tempo::Tempo::new(backend.clone(), timeout))
            .transpose()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        let loki = config
            .loki
            .as_ref()
            .map(|backend| loki::Loki::new(backend.clone(), timeout))
            .transpose()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        let prometheus = config
            .prometheus
            .as_ref()
            .map(|backend| prometheus::Prometheus::new(backend.clone(), timeout))
            .transpose()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        let grafana = config
            .grafana_url
            .as_deref()
            .map(backend::validated_endpoint)
            .transpose()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        Ok(Self {
            config,
            tempo,
            loki,
            prometheus,
            grafana,
        })
    }

    pub fn status(&self) -> ObservabilityStatus {
        let capability = |client: Option<&backend::BackendClient>, authenticated| {
            client.map_or_else(BackendCapability::unconfigured, |client| {
                BackendCapability::configured(client.origin(), authenticated)
            })
        };
        ObservabilityStatus {
            capabilities: BackendCapabilities {
                tempo: capability(
                    self.tempo.as_ref().map(tempo::Tempo::client),
                    self.config
                        .tempo
                        .as_ref()
                        .is_some_and(backend::BackendClient::authenticated),
                ),
                loki: capability(
                    self.loki.as_ref().map(loki::Loki::client),
                    self.config
                        .loki
                        .as_ref()
                        .is_some_and(backend::BackendClient::authenticated),
                ),
                prometheus: capability(
                    self.prometheus.as_ref().map(prometheus::Prometheus::client),
                    self.config
                        .prometheus
                        .as_ref()
                        .is_some_and(backend::BackendClient::authenticated),
                ),
                grafana: self
                    .grafana
                    .as_ref()
                    .map_or_else(BackendCapability::unconfigured, |url| {
                        BackendCapability::configured(&url.origin().ascii_serialization(), false)
                    }),
            },
            queried_at: Utc::now(),
        }
    }

    pub async fn run(&self, store: Store, request: RunRequest) -> Result<RunObservability> {
        validate_run_id(&request.run)?;
        validate_limit(request.limit)?;
        self.validate_range(&request.range)?;
        let validation_store = store.clone();
        let validation_run = request.run.clone();
        tokio::task::spawn_blocking(move || require_run(&validation_store, &validation_run))
            .await??;
        let local_cursor = request
            .local_cursor
            .as_deref()
            .map(parse_local_cursor)
            .transpose()?
            .unwrap_or(0);
        let run = request.run.clone();
        let limit = request.limit;
        let local =
            tokio::task::spawn_blocking(move || local::query(&store, &run, local_cursor, limit));
        let traces = self.search_traces(
            &request.run,
            PageRequest {
                range: request.range.clone(),
                limit: request.limit,
                cursor: request.trace_cursor,
            },
        );
        let logs = self.logs(
            &request.run,
            None,
            PageRequest {
                range: request.range.clone(),
                limit: request.limit,
                cursor: request.log_cursor,
            },
        );
        let metrics = self.metrics(
            request.run.clone(),
            request.metric,
            request.range,
            request.limit,
        );
        let (local, traces, logs, metrics) = tokio::join!(local, traces, logs, metrics);
        Ok(RunObservability {
            local: local??,
            traces: traces?,
            logs: logs?,
            metrics: metrics?,
        })
    }

    pub async fn search_traces(
        &self,
        run: &str,
        request: PageRequest,
    ) -> Result<Signal<TraceList>> {
        validate_run_id(run)?;
        validate_limit(request.limit)?;
        self.validate_range(&request.range)?;
        tempo::validate_cursor(request.cursor.as_deref(), &request.range)?;
        let Some(client) = &self.tempo else {
            return Ok(Signal::unconfigured());
        };
        Ok(match client.search(run, request).await {
            Ok((traces, truncated)) => Signal::success(traces, truncated),
            Err(error) => Signal::failure(error),
        })
    }

    pub async fn trace(&self, run: &str, trace_id: &str) -> Result<Signal<Trace>> {
        validate_run_id(run)?;
        let trace_id = validate_trace_id(trace_id)?;
        let Some(client) = &self.tempo else {
            return Ok(Signal::unconfigured());
        };
        Ok(match client.trace(&trace_id).await {
            Ok((trace, truncated)) if trace.belongs_to(run) => Signal::success(trace, truncated),
            Ok(_) => Signal::failure(BackendError {
                kind: BackendErrorKind::NotFound,
                message: "trace does not belong to the requested run".into(),
                status: None,
            }),
            Err(error) => Signal::failure(error),
        })
    }

    pub async fn logs(
        &self,
        run: &str,
        trace_id: Option<&str>,
        request: PageRequest,
    ) -> Result<Signal<LogBatch>> {
        validate_run_id(run)?;
        validate_limit(request.limit)?;
        self.validate_range(&request.range)?;
        loki::validate_cursor(request.cursor.as_deref(), &request.range)?;
        let trace_id = trace_id.map(validate_trace_id).transpose()?;
        let Some(client) = &self.loki else {
            return Ok(Signal::unconfigured());
        };
        Ok(
            match client.query(run, trace_id.as_deref(), request).await {
                Ok((logs, truncated)) => Signal::success(logs, truncated),
                Err(error) => Signal::failure(error),
            },
        )
    }

    pub async fn metrics(
        &self,
        run: String,
        metric: MetricName,
        range: TimeRange,
        limit: usize,
    ) -> Result<Signal<MetricBatch>> {
        validate_run_id(&run)?;
        validate_limit(limit)?;
        self.validate_range(&range)?;
        let Some(client) = &self.prometheus else {
            return Ok(Signal::unconfigured());
        };
        Ok(match client.query(metric, range, limit).await {
            Ok((metrics, truncated)) => Signal::success(metrics, truncated),
            Err(error) => Signal::failure(error),
        })
    }

    /// Return a validated URL only. Opening it remains a separate, explicit UI action.
    pub fn grafana_link(&self, target: GrafanaTarget, range: TimeRange) -> Result<Option<String>> {
        self.validate_range(&range)?;
        let Some(base) = &self.grafana else {
            return Ok(None);
        };
        let (datasource, query_field, query) = match target {
            GrafanaTarget::Traces { run } => {
                validate_run_id(&run)?;
                ("tempo", "query", tempo::run_query(&run))
            }
            GrafanaTarget::Logs { run, trace_id } => {
                validate_run_id(&run)?;
                let trace_id = trace_id.as_deref().map(validate_trace_id).transpose()?;
                (
                    "loki",
                    "expr",
                    loki::query_template(&run, trace_id.as_deref()),
                )
            }
            GrafanaTarget::Metrics { metric } => ("prometheus", "expr", metric.expression().into()),
        };
        let mut query_object = serde_json::Map::new();
        query_object.insert(query_field.into(), query.into());
        query_object.insert("refId".into(), "A".into());
        let left = serde_json::json!({
            "datasource": datasource,
            "queries": [serde_json::Value::Object(query_object)],
            "range": {"from": range.start.to_rfc3339(), "to": range.end.to_rfc3339()}
        });
        let mut url = base.clone();
        let base_path = base.path().trim_end_matches('/');
        url.set_path(&format!("{base_path}/explore"));
        url.query_pairs_mut()
            .append_pair("left", &serde_json::to_string(&left)?);
        Ok(Some(url.into()))
    }

    fn validate_range(&self, range: &TimeRange) -> Result<()> {
        ensure!(
            range.start < range.end,
            "query range start must precede end"
        );
        ensure!(
            range.end.signed_duration_since(range.start)
                <= chrono::Duration::seconds(self.config.max_range_secs() as i64),
            "query range exceeds the configured maximum"
        );
        Ok(())
    }
}

pub(crate) fn timestamp_from_nanos(nanos: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(
        nanos.div_euclid(1_000_000_000),
        u32::try_from(nanos.rem_euclid(1_000_000_000)).ok()?,
    )
}

pub(crate) fn validate_limit(limit: usize) -> Result<()> {
    ensure!(
        (1..=MAX_QUERY_LIMIT).contains(&limit),
        "query limit must be 1–500"
    );
    Ok(())
}

pub(crate) fn validate_run_id(run: &str) -> Result<()> {
    ensure!(
        !run.is_empty()
            && run.len() <= 128
            && run
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
        "run identifier is invalid"
    );
    Ok(())
}

pub(crate) fn validate_trace_id(trace_id: &str) -> Result<String> {
    ensure!(
        trace_id.len() == 32 && trace_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "trace identifier must contain 32 hexadecimal characters"
    );
    Ok(trace_id.to_ascii_lowercase())
}

fn parse_local_cursor(cursor: &str) -> Result<i64> {
    let value = cursor.parse::<i64>()?;
    ensure!(value >= 0, "local activity cursor must be non-negative");
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range() -> TimeRange {
        TimeRange {
            start: "2026-01-01T00:00:00Z".parse().unwrap(),
            end: "2026-01-01T00:10:00Z".parse().unwrap(),
        }
    }

    #[test]
    fn invalid_ranges_limits_and_ids_are_rejected() {
        let observability = Observability::new(QueryConfig::default()).unwrap();
        assert!(
            observability
                .validate_range(&TimeRange {
                    start: range().end,
                    end: range().start,
                })
                .is_err()
        );
        assert!(validate_limit(0).is_err());
        assert!(validate_limit(501).is_err());
        assert!(validate_run_id("run';drop").is_err());
        assert!(validate_trace_id("not-a-trace").is_err());
        assert!(tempo::validate_cursor(Some("not-a-cursor"), &range()).is_err());
        assert!(tempo::validate_cursor(Some(&i64::MAX.to_string()), &range()).is_err());
    }

    #[test]
    fn unconfigured_signals_are_explicit_not_failures() {
        let observability = Observability::new(QueryConfig::default()).unwrap();
        let status = observability.status();
        assert!(!status.capabilities.tempo.configured);
        let signal: Signal<()> = Signal::unconfigured();
        assert!(!signal.configured && !signal.partial && signal.error.is_none());
    }

    #[test]
    fn grafana_links_use_each_datasources_query_field() {
        let observability = Observability::new(QueryConfig {
            grafana_url: Some("http://127.0.0.1:3000".into()),
            ..QueryConfig::default()
        })
        .unwrap();
        for (target, field) in [
            (
                GrafanaTarget::Logs {
                    run: "run-1".into(),
                    trace_id: None,
                },
                "expr",
            ),
            (
                GrafanaTarget::Metrics {
                    metric: MetricName::RunsStarted,
                },
                "expr",
            ),
            (
                GrafanaTarget::Traces {
                    run: "run-1".into(),
                },
                "query",
            ),
        ] {
            let link = observability
                .grafana_link(target, range())
                .unwrap()
                .unwrap();
            let url = reqwest::Url::parse(&link).unwrap();
            let left = url
                .query_pairs()
                .find(|(key, _)| key == "left")
                .map(|(_, value)| value.into_owned())
                .unwrap();
            let left: serde_json::Value = serde_json::from_str(&left).unwrap();
            assert!(left["queries"][0][field].is_string());
        }
    }

    #[tokio::test]
    async fn one_backend_failure_does_not_change_other_signals() {
        let config = QueryConfig {
            tempo: Some(crate::adapters::telemetry::BackendConfig {
                url: "https://tempo.example.com".into(),
                credential_env: Some("WUKONG_TEST_INTENTIONALLY_MISSING_CREDENTIAL".into()),
            }),
            ..Default::default()
        };
        let observability = Observability::new(config).unwrap();
        let traces = observability.search_traces(
            "run-1",
            PageRequest {
                range: range(),
                limit: 10,
                cursor: None,
            },
        );
        let logs = observability.logs(
            "run-1",
            None,
            PageRequest {
                range: range(),
                limit: 10,
                cursor: None,
            },
        );
        let metrics = observability.metrics("run-1".into(), MetricName::ActiveRuns, range(), 10);
        let (traces, logs, metrics) = tokio::join!(traces, logs, metrics);
        let traces = traces.unwrap();
        assert!(traces.partial);
        assert_eq!(
            traces.error.as_ref().map(|error| error.kind),
            Some(BackendErrorKind::Authentication)
        );
        assert!(!logs.unwrap().configured);
        assert!(!metrics.unwrap().configured);
    }
}
