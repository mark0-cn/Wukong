use super::{
    PageRequest,
    backend::{BackendClient, BackendError, BackendErrorKind},
};
use crate::adapters::telemetry::BackendConfig;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_LOG_LINE_CHARS: usize = 4096;
const MAX_CURSOR_SCAN: usize = 4096;

#[derive(Clone)]
pub(crate) struct Loki {
    client: BackendClient,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct LogStreamLabels {
    pub service_name: Option<String>,
    pub level: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub timestamp_nanos: String,
    pub line: String,
    pub line_truncated: bool,
    pub labels: LogStreamLabels,
    pub attributes: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogBatch {
    pub run_id: String,
    pub trace_id: Option<String>,
    pub entries: Vec<LogEntry>,
    pub next_cursor: Option<String>,
}

impl Loki {
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
        run: &str,
        trace_id: Option<&str>,
        request: PageRequest,
    ) -> Result<(LogBatch, bool), BackendError> {
        let (end, skip) = query_end(&request)?;
        let response_limit = skip
            .checked_add(request.limit + 1)
            .filter(|limit| *limit <= MAX_CURSOR_SCAN)
            .ok_or_else(invalid_cursor)?;
        let start = request
            .range
            .start
            .timestamp_nanos_opt()
            .ok_or_else(invalid_range)?;
        let end_nanos = end.timestamp_nanos_opt().ok_or_else(invalid_range)?;
        let params = [
            ("query", query_template(run, trace_id)),
            ("start", start.to_string()),
            ("end", end_nanos.to_string()),
            ("limit", response_limit.to_string()),
            ("direction", "backward".into()),
        ];
        let value = self
            .client
            .get_json("loki/api/v1/query_range", &params)
            .await?;
        let results = value
            .pointer("/data/result")
            .and_then(Value::as_array)
            .ok_or_else(invalid_response)?;
        let (entries, response_truncated) = map_entries(results, response_limit)?;
        let (entries, next_cursor, page_truncated) = paginate_entries(
            entries,
            request.limit,
            end.timestamp_nanos_opt().ok_or_else(invalid_range)?,
            skip,
            response_truncated,
        )?;
        let line_truncated = entries.iter().any(|entry| entry.line_truncated);
        let truncated = page_truncated || line_truncated;
        Ok((
            LogBatch {
                run_id: run.into(),
                trace_id: trace_id.map(str::to_owned),
                entries,
                next_cursor,
            },
            truncated,
        ))
    }
}

fn map_entries(
    results: &[Value],
    response_limit: usize,
) -> Result<(Vec<LogEntry>, bool), BackendError> {
    let mut entries = Vec::new();
    let mut response_truncated = false;
    for stream in results {
        let labels = stream_labels(stream.get("stream"));
        let values = stream
            .get("values")
            .and_then(Value::as_array)
            .ok_or_else(invalid_response)?;
        // Loki applies limit per stream. Never discard another stream before
        // the merged deterministic ordering and cursor pagination below.
        response_truncated |= values.len() == response_limit;
        for item in values {
            let fields = item.as_array().ok_or_else(invalid_response)?;
            let timestamp_nanos = fields
                .first()
                .and_then(Value::as_str)
                .ok_or_else(invalid_response)?;
            let timestamp = parse_nanos(timestamp_nanos)?;
            let line = fields.get(1).and_then(Value::as_str).unwrap_or_default();
            let line_truncated = line.chars().count() > MAX_LOG_LINE_CHARS;
            entries.push(LogEntry {
                timestamp,
                timestamp_nanos: timestamp_nanos.into(),
                line: line.chars().take(MAX_LOG_LINE_CHARS).collect(),
                line_truncated,
                labels: labels.clone(),
                attributes: fields
                    .get(2)
                    .and_then(Value::as_array)
                    .map(|attributes| safe_attributes(attributes))
                    .unwrap_or_default(),
            });
        }
    }
    Ok((entries, response_truncated))
}

fn paginate_entries(
    mut entries: Vec<LogEntry>,
    limit: usize,
    cursor_nanos: i64,
    skip: usize,
    response_truncated: bool,
) -> Result<(Vec<LogEntry>, Option<String>, bool), BackendError> {
    entries.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.labels.cmp(&right.labels))
            .then_with(|| {
                serde_json::to_string(&left.attributes)
                    .unwrap_or_default()
                    .cmp(&serde_json::to_string(&right.attributes).unwrap_or_default())
            })
    });
    if skip > entries.len() {
        return Err(invalid_cursor());
    }
    let has_more = response_truncated || entries.len() > skip + limit;
    entries = entries.into_iter().skip(skip).take(limit).collect();
    let next_cursor = if has_more {
        let last = entries.last().ok_or_else(invalid_response)?;
        let last_nanos = last
            .timestamp_nanos
            .parse::<i64>()
            .map_err(|_| invalid_response())?;
        let at_boundary = entries
            .iter()
            .filter(|entry| entry.timestamp_nanos == last.timestamp_nanos)
            .count();
        let next_skip = if last_nanos == cursor_nanos {
            skip + at_boundary
        } else {
            at_boundary
        };
        Some(format!("{last_nanos}:{next_skip}"))
    } else {
        None
    };
    Ok((entries, next_cursor, has_more))
}

pub(super) fn query_template(run: &str, trace_id: Option<&str>) -> String {
    let mut query = format!("{{service_name=~\".+\"}} | run_id=\"{run}\"");
    if let Some(trace_id) = trace_id {
        query.push_str(&format!(" | trace_id=\"{trace_id}\""));
    }
    query
}

pub(super) fn validate_cursor(
    cursor: Option<&str>,
    range: &super::TimeRange,
) -> anyhow::Result<()> {
    let request = PageRequest {
        range: range.clone(),
        limit: 1,
        cursor: cursor.map(str::to_owned),
    };
    query_end(&request)
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!(error.message))
}

fn query_end(request: &PageRequest) -> Result<(DateTime<Utc>, usize), BackendError> {
    let Some(cursor) = request.cursor.as_deref() else {
        return Ok((request.range.end, 0));
    };
    let (nanos, skip) = cursor.split_once(':').ok_or_else(invalid_cursor)?;
    let nanos = nanos.parse::<i64>().map_err(|_| invalid_cursor())?;
    let skip = skip.parse::<usize>().map_err(|_| invalid_cursor())?;
    if skip >= MAX_CURSOR_SCAN {
        return Err(invalid_cursor());
    }
    let end = parse_nanos(&nanos.to_string())?;
    if end < request.range.start || end > request.range.end {
        return Err(invalid_cursor());
    }
    Ok((end, skip))
}

fn stream_labels(value: Option<&Value>) -> LogStreamLabels {
    let get = |name: &str| {
        value
            .and_then(|labels| labels.get(name))
            .and_then(Value::as_str)
            .map(|value| value.chars().take(128).collect())
    };
    LogStreamLabels {
        service_name: get("service_name"),
        level: get("level"),
        target: get("target"),
    }
}

fn safe_attributes(attributes: &[Value]) -> BTreeMap<String, Value> {
    attributes
        .iter()
        .filter_map(|attribute| {
            let pair = attribute.as_array()?;
            let key = pair.first()?.as_str()?;
            if !matches!(
                key,
                "run_id"
                    | "trace_id"
                    | "span_id"
                    | "agent_id"
                    | "generation"
                    | "turn"
                    | "call_id"
                    | "worker_request_id"
                    | "job_id"
                    | "level"
                    | "target"
                    | "outcome"
                    | "error.type"
            ) {
                return None;
            }
            let value = pair.get(1)?.as_str()?.chars().take(256).collect();
            Some((key.into(), Value::String(value)))
        })
        .take(32)
        .collect()
}

fn parse_nanos(value: &str) -> Result<DateTime<Utc>, BackendError> {
    let nanos = value.parse::<i64>().map_err(|_| invalid_response())?;
    super::timestamp_from_nanos(nanos).ok_or_else(invalid_response)
}

fn invalid_cursor() -> BackendError {
    BackendError {
        kind: BackendErrorKind::InvalidRequest,
        message: "log cursor is invalid".into(),
        status: None,
    }
}

fn invalid_range() -> BackendError {
    BackendError {
        kind: BackendErrorKind::InvalidRequest,
        message: "log query range cannot be represented".into(),
        status: None,
    }
}

fn invalid_response() -> BackendError {
    BackendError {
        kind: BackendErrorKind::InvalidResponse,
        message: "backend returned an unexpected Loki response".into(),
        status: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_accept_only_run_and_trace_intent() {
        assert_eq!(
            query_template("run-1", None),
            "{service_name=~\".+\"} | run_id=\"run-1\""
        );
        assert_eq!(
            query_template("run-1", Some(&"a".repeat(32))),
            format!(
                "{{service_name=~\".+\"}} | run_id=\"run-1\" | trace_id=\"{}\"",
                "a".repeat(32)
            )
        );
        assert!(super::super::validate_run_id("run\" | secret=\"x").is_err());
    }

    #[test]
    fn stream_mapping_keeps_only_safe_labels() {
        let labels = stream_labels(Some(&serde_json::json!({
            "service_name":"wukong", "level":"info", "authorization":"secret"
        })));
        assert_eq!(labels.service_name.as_deref(), Some("wukong"));
        assert_eq!(labels.level.as_deref(), Some("info"));
        let attributes = safe_attributes(&[
            serde_json::json!(["run_id", "run-1"]),
            serde_json::json!(["password", "secret"]),
        ]);
        assert_eq!(attributes["run_id"], "run-1");
        assert!(!attributes.contains_key("password"));
    }

    #[test]
    fn multiple_streams_are_merged_before_pagination() {
        let results = vec![
            serde_json::json!({"stream":{"service_name":"older"},"values":[["1000000000","older"]]}),
            serde_json::json!({"stream":{"service_name":"newer"},"values":[["2000000000","newer"]]}),
        ];
        let (entries, response_truncated) = map_entries(&results, 2).unwrap();
        assert!(!response_truncated);
        let range = super::super::TimeRange {
            start: super::super::timestamp_from_nanos(0).unwrap(),
            end: super::super::timestamp_from_nanos(3_000_000_000).unwrap(),
        };
        assert!(validate_cursor(Some("2000000000:1"), &range).is_ok());
        let (first, cursor, truncated) =
            paginate_entries(entries.clone(), 1, 3_000_000_000, 0, false).unwrap();
        assert_eq!(first[0].line, "newer");
        assert_eq!(cursor.as_deref(), Some("2000000000:1"));
        assert!(truncated);
        let (second, cursor, truncated) =
            paginate_entries(entries, 1, 2_000_000_000, 1, false).unwrap();
        assert_eq!(second[0].line, "older");
        assert!(cursor.is_none());
        assert!(!truncated);
    }

    #[test]
    fn pagination_retains_all_entries_sharing_a_boundary_timestamp() {
        let range = super::super::TimeRange {
            start: super::super::timestamp_from_nanos(0).unwrap(),
            end: super::super::timestamp_from_nanos(2_000_000_000).unwrap(),
        };
        assert!(validate_cursor(Some("1000000000:2"), &range).is_ok());
        assert!(validate_cursor(Some("1000000000"), &range).is_err());
        let entry = |line: &str| LogEntry {
            timestamp: parse_nanos("1000000000").unwrap(),
            timestamp_nanos: "1000000000".into(),
            line: line.into(),
            line_truncated: false,
            labels: LogStreamLabels::default(),
            attributes: BTreeMap::new(),
        };
        let source = vec![entry("c"), entry("a"), entry("b")];
        let (first, cursor, truncated) =
            paginate_entries(source.clone(), 2, 2_000_000_000, 0, false).unwrap();
        assert_eq!(
            first
                .iter()
                .map(|entry| entry.line.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(cursor.as_deref(), Some("1000000000:2"));
        assert!(truncated);
        let (second, cursor, truncated) =
            paginate_entries(source, 2, 1_000_000_000, 2, false).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].line, "c");
        assert_eq!(cursor, None);
        assert!(!truncated);
    }
}
