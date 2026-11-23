use super::{
    TimeRange,
    backend::{BackendClient, BackendError, BackendErrorKind},
};
use crate::adapters::telemetry::BackendConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_METRIC_SERIES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricName {
    RunsStarted,
    RunsFinished,
    ActiveRuns,
    RunDurationCount,
    AgentGenerationDurationCount,
    WorkerRpcDurationCount,
    WorkerRpcRetries,
    ModelOperationDurationCount,
    ModelTokenUsageCount,
    EvidenceSyncDurationCount,
    TelemetryDropped,
}

impl MetricName {
    pub(crate) fn expression(self) -> &'static str {
        match self {
            Self::RunsStarted => "wukong_run_started_total",
            Self::RunsFinished => "wukong_run_finished_total",
            Self::ActiveRuns => "wukong_run_active",
            Self::RunDurationCount => "wukong_run_duration_seconds_count",
            Self::AgentGenerationDurationCount => "wukong_agent_generation_duration_seconds_count",
            Self::WorkerRpcDurationCount => "wukong_worker_rpc_duration_seconds_count",
            Self::WorkerRpcRetries => "wukong_worker_rpc_retries_total",
            Self::ModelOperationDurationCount => "gen_ai_client_operation_duration_seconds_count",
            Self::ModelTokenUsageCount => "gen_ai_client_token_usage_tokens_count",
            Self::EvidenceSyncDurationCount => "wukong_evidence_sync_duration_seconds_count",
            Self::TelemetryDropped => "wukong_telemetry_dropped_total",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricPoint {
    pub timestamp: DateTime<Utc>,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricSeries {
    pub labels: BTreeMap<String, String>,
    pub points: Vec<MetricPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricBatch {
    pub metric: MetricName,
    pub expression: &'static str,
    pub scope: &'static str,
    pub step_seconds: u64,
    pub series: Vec<MetricSeries>,
}

#[derive(Clone)]
pub(crate) struct Prometheus {
    client: BackendClient,
}

impl Prometheus {
    pub(crate) fn new(config: BackendConfig, timeout: u64) -> Result<Self, BackendError> {
        Ok(Self {
            client: BackendClient::new(&config, timeout)?,
        })
    }

    pub(crate) fn client(&self) -> &BackendClient {
        &self.client
    }

    pub(crate) async fn query(
        &self,
        metric: MetricName,
        range: TimeRange,
        limit: usize,
    ) -> Result<(MetricBatch, bool), BackendError> {
        let seconds = range
            .end
            .signed_duration_since(range.start)
            .num_seconds()
            .max(1) as u64;
        let step_seconds = seconds.div_ceil(limit as u64).max(1);
        let params = [
            ("query", metric.expression().into()),
            ("start", range.start.to_rfc3339()),
            ("end", range.end.to_rfc3339()),
            ("step", format!("{step_seconds}s")),
        ];
        let value = self.client.get_json("api/v1/query_range", &params).await?;
        map_response(metric, step_seconds, limit, &value)
    }
}

fn map_response(
    metric: MetricName,
    step_seconds: u64,
    limit: usize,
    value: &Value,
) -> Result<(MetricBatch, bool), BackendError> {
    if value.get("status").and_then(Value::as_str) != Some("success")
        || value.pointer("/data/resultType").and_then(Value::as_str) != Some("matrix")
    {
        return Err(invalid_response());
    }
    let results = value
        .pointer("/data/result")
        .and_then(Value::as_array)
        .ok_or_else(invalid_response)?;
    let mut truncated = results.len() > MAX_METRIC_SERIES;
    let mut series = Vec::new();
    for result in results.iter().take(MAX_METRIC_SERIES) {
        let labels = safe_labels(result.get("metric"));
        let values = result
            .get("values")
            .and_then(Value::as_array)
            .ok_or_else(invalid_response)?;
        truncated |= values.len() > limit;
        let mut points = Vec::new();
        for value in values.iter().take(limit) {
            let pair = value.as_array().ok_or_else(invalid_response)?;
            let timestamp = pair
                .first()
                .and_then(Value::as_f64)
                .ok_or_else(invalid_response)?;
            if !timestamp.is_finite() {
                return Err(invalid_response());
            }
            let seconds = timestamp.floor();
            let nanos = ((timestamp - seconds) * 1_000_000_000.0).round();
            if !(0.0..1_000_000_000.0).contains(&nanos) {
                return Err(invalid_response());
            }
            let seconds = seconds
                .to_string()
                .parse::<i64>()
                .map_err(|_| invalid_response())?;
            let nanos = nanos
                .to_string()
                .parse::<u32>()
                .map_err(|_| invalid_response())?;
            let timestamp =
                DateTime::from_timestamp(seconds, nanos).ok_or_else(invalid_response)?;
            let sample = pair
                .get(1)
                .and_then(Value::as_str)
                .ok_or_else(invalid_response)?;
            let sample: f64 = sample.parse().map_err(|_| invalid_response())?;
            if !sample.is_finite() {
                return Err(invalid_response());
            }
            points.push(MetricPoint {
                timestamp,
                value: sample,
            });
        }
        series.push(MetricSeries { labels, points });
    }
    Ok((
        MetricBatch {
            metric,
            expression: metric.expression(),
            scope: "service",
            step_seconds,
            series,
        },
        truncated,
    ))
}

fn safe_labels(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(key, _)| {
            matches!(
                key.as_str(),
                "outcome"
                    | "gen_ai_provider_name"
                    | "gen_ai_token_type"
                    | "rpc_method"
                    | "transport"
                    | "signal"
                    | "reason"
                    | "service_name"
            )
        })
        .filter_map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key.clone(), value.chars().take(128).collect()))
        })
        .take(16)
        .collect()
}

fn invalid_response() -> BackendError {
    BackendError {
        kind: BackendErrorKind::InvalidResponse,
        message: "backend returned an unexpected Prometheus response".into(),
        status: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_allowlist_has_no_arbitrary_expression_input() {
        assert_eq!(MetricName::ActiveRuns.expression(), "wukong_run_active");
        let unknown = serde_json::from_str::<MetricName>("\"up{job=\\\"secret\\\"}\"");
        assert!(unknown.is_err());
        for metric in [
            MetricName::RunsStarted,
            MetricName::RunsFinished,
            MetricName::ActiveRuns,
            MetricName::RunDurationCount,
            MetricName::AgentGenerationDurationCount,
            MetricName::WorkerRpcDurationCount,
            MetricName::WorkerRpcRetries,
            MetricName::ModelOperationDurationCount,
            MetricName::ModelTokenUsageCount,
            MetricName::EvidenceSyncDurationCount,
            MetricName::TelemetryDropped,
        ] {
            assert!(!metric.expression().contains(['{', ' ', '"']));
        }
    }

    #[test]
    fn matrix_mapping_is_bounded_and_filters_labels() {
        let values = (0..3)
            .map(|second| serde_json::json!([second as f64 + 1.0, second.to_string()]))
            .collect::<Vec<_>>();
        let response = serde_json::json!({
            "status":"success",
            "data":{"resultType":"matrix","result":[{
                "metric":{"outcome":"completed","instance":"private-host"},
                "values":values
            }]}
        });
        let (batch, truncated) = map_response(MetricName::RunsFinished, 1, 2, &response).unwrap();
        assert!(truncated);
        assert_eq!(batch.series[0].points.len(), 2);
        assert_eq!(batch.series[0].labels["outcome"], "completed");
        assert!(!batch.series[0].labels.contains_key("instance"));
    }
}
