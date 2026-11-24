use super::{
    PageRequest,
    backend::{BackendClient, BackendError, BackendErrorKind},
};
use crate::adapters::telemetry::BackendConfig;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_CURSOR_SCAN: usize = 4096;

#[derive(Clone)]
pub(crate) struct Tempo {
    client: BackendClient,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceSummary {
    pub trace_id: String,
    pub root_service_name: Option<String>,
    pub root_trace_name: Option<String>,
    pub start_time: Option<DateTime<Utc>>,
    pub duration_ms: u64,
    pub span_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceList {
    pub run_id: String,
    pub traces: Vec<TraceSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceSpan {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub duration_nanos: Option<u64>,
    pub status: Option<String>,
    pub attributes: BTreeMap<String, Value>,
    pub resource: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Trace {
    pub trace_id: String,
    pub spans: Vec<TraceSpan>,
}

impl Trace {
    pub(super) fn belongs_to(&self, run: &str) -> bool {
        self.spans.iter().any(|span| {
            span.attributes
                .get("run_id")
                .and_then(Value::as_str)
                .is_some_and(|value| value == run)
        })
    }
}

impl Tempo {
    pub(crate) fn new(config: BackendConfig, timeout: u64) -> Result<Self, BackendError> {
        Ok(Self {
            client: BackendClient::new(&config, timeout)?,
        })
    }

    pub(crate) fn client(&self) -> &BackendClient {
        &self.client
    }

    pub(crate) async fn search(
        &self,
        run: &str,
        request: PageRequest,
    ) -> Result<(TraceList, bool), BackendError> {
        let cursor = parse_cursor(&request)?;
        let response_limit = if cursor.is_some() {
            MAX_CURSOR_SCAN
        } else {
            request.limit + 1
        };
        let end = cursor
            .as_ref()
            .map_or(request.range.end.timestamp(), |cursor| {
                cursor.nanos.div_euclid(1_000_000_000)
            });
        let params = [
            ("q", run_query(run)),
            ("start", request.range.start.timestamp().to_string()),
            ("end", end.to_string()),
            ("limit", response_limit.to_string()),
        ];
        let value = self.client.get_json("api/search", &params).await?;
        let traces = value
            .get("traces")
            .and_then(Value::as_array)
            .ok_or_else(invalid_response)?;
        if traces.len() > response_limit {
            return Err(invalid_response());
        }
        let response_truncated = traces.len() == response_limit;
        let mapped = traces
            .iter()
            .map(map_summary)
            .collect::<Result<Vec<_>, _>>()?;
        let (traces, next_cursor, truncated) =
            paginate_traces(mapped, &request, cursor.as_ref(), response_truncated)?;
        Ok((
            TraceList {
                run_id: run.into(),
                traces,
                next_cursor,
            },
            truncated,
        ))
    }

    pub(crate) async fn trace(&self, trace_id: &str) -> Result<(Trace, bool), BackendError> {
        let value = self
            .client
            .get_json(&format!("api/traces/{trace_id}"), &[])
            .await?;
        map_trace(trace_id, &value)
    }
}

pub(super) fn run_query(run: &str) -> String {
    // run IDs are restricted to ASCII alphanumerics, '_' and '-' by the facade.
    format!("{{ .run_id = \"{run}\" }}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TraceCursor {
    nanos: i64,
    trace_id: String,
}

fn parse_cursor(request: &PageRequest) -> Result<Option<TraceCursor>, BackendError> {
    let Some(cursor) = request.cursor.as_deref() else {
        return Ok(None);
    };
    let (nanos, trace_id) = cursor.split_once(':').ok_or_else(invalid_cursor)?;
    let nanos = nanos.parse::<i64>().map_err(|_| invalid_cursor())?;
    if trace_id.len() != 32 || !trace_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_cursor());
    }
    let timestamp = super::timestamp_from_nanos(nanos).ok_or_else(invalid_cursor)?;
    if timestamp < request.range.start || timestamp > request.range.end {
        return Err(invalid_cursor());
    }
    Ok(Some(TraceCursor {
        nanos,
        trace_id: trace_id.to_ascii_lowercase(),
    }))
}

pub(super) fn validate_cursor(
    cursor: Option<&str>,
    range: &super::TimeRange,
) -> anyhow::Result<()> {
    parse_cursor(&PageRequest {
        range: range.clone(),
        limit: 1,
        cursor: cursor.map(str::to_owned),
    })
    .map(|_| ())
    .map_err(|error| anyhow::anyhow!(error.message))
}

fn paginate_traces(
    mut traces: Vec<TraceSummary>,
    request: &PageRequest,
    cursor: Option<&TraceCursor>,
    response_truncated: bool,
) -> Result<(Vec<TraceSummary>, Option<String>, bool), BackendError> {
    traces.retain(|trace| {
        trace
            .start_time
            .is_some_and(|start| start >= request.range.start && start <= request.range.end)
    });
    traces.sort_by(|left, right| {
        right
            .start_time
            .cmp(&left.start_time)
            .then_with(|| left.trace_id.cmp(&right.trace_id))
    });
    if let Some(cursor) = cursor {
        traces.retain(|trace| {
            let nanos = trace
                .start_time
                .and_then(|start| start.timestamp_nanos_opt())
                .unwrap_or(i64::MIN);
            nanos < cursor.nanos
                || (nanos == cursor.nanos && trace.trace_id.as_str() > cursor.trace_id.as_str())
        });
    }
    let has_more = response_truncated || traces.len() > request.limit;
    if has_more && traces.is_empty() {
        // A full overlap page with no older entry cannot advance safely.
        return Err(invalid_cursor());
    }
    traces.truncate(request.limit);
    let next_cursor = if has_more {
        let last = traces.last().ok_or_else(invalid_response)?;
        let nanos = last
            .start_time
            .and_then(|start| start.timestamp_nanos_opt())
            .ok_or_else(invalid_response)?;
        Some(format!("{nanos}:{}", last.trace_id))
    } else {
        None
    };
    Ok((traces, next_cursor, has_more))
}

fn invalid_cursor() -> BackendError {
    BackendError {
        kind: BackendErrorKind::InvalidRequest,
        message: "trace cursor is invalid".into(),
        status: None,
    }
}

fn invalid_response() -> BackendError {
    BackendError {
        kind: BackendErrorKind::InvalidResponse,
        message: "backend returned an unexpected Tempo response".into(),
        status: None,
    }
}

fn map_summary(value: &Value) -> Result<TraceSummary, BackendError> {
    let trace_id = value
        .get("traceID")
        .and_then(Value::as_str)
        .filter(|id| id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .ok_or_else(invalid_response)?;
    let start_time = Some(parse_nanos(
        value
            .get("startTimeUnixNano")
            .and_then(Value::as_str)
            .ok_or_else(invalid_response)?,
    )?);
    let span_count = value
        .get("spanSets")
        .and_then(Value::as_array)
        .map(|sets| {
            sets.iter()
                .filter_map(|set| set.get("spans").and_then(Value::as_array))
                .map(Vec::len)
                .sum()
        })
        .or_else(|| {
            value
                .get("spanSet")
                .and_then(|set| set.get("spans"))
                .and_then(Value::as_array)
                .map(Vec::len)
        })
        .unwrap_or(0);
    Ok(TraceSummary {
        trace_id,
        root_service_name: safe_optional_string(value.get("rootServiceName")),
        root_trace_name: safe_optional_string(value.get("rootTraceName")),
        start_time,
        duration_ms: value.get("durationMs").and_then(Value::as_u64).unwrap_or(0),
        span_count,
    })
}

fn map_trace(trace_id: &str, value: &Value) -> Result<(Trace, bool), BackendError> {
    let root = value.get("trace").unwrap_or(value);
    let batches = root
        .get("batches")
        .and_then(Value::as_array)
        .ok_or_else(invalid_response)?;
    let mut spans = Vec::new();
    let mut truncated = false;
    for batch in batches {
        let resource = attributes(batch.pointer("/resource/attributes"));
        let scopes = batch
            .get("scopeSpans")
            .and_then(Value::as_array)
            .ok_or_else(invalid_response)?;
        for scope in scopes {
            let items = scope
                .get("spans")
                .and_then(Value::as_array)
                .ok_or_else(invalid_response)?;
            for span in items {
                if spans.len() == super::MAX_QUERY_LIMIT {
                    truncated = true;
                    continue;
                }
                spans.push(map_span(trace_id, span, resource.clone())?);
            }
        }
    }
    Ok((
        Trace {
            trace_id: trace_id.into(),
            spans,
        },
        truncated,
    ))
}

fn map_span(
    fallback_trace_id: &str,
    value: &Value,
    resource: BTreeMap<String, Value>,
) -> Result<TraceSpan, BackendError> {
    let trace_id = match value.get("traceId").and_then(Value::as_str) {
        Some(id) => decode_otlp_id(id, 16).ok_or_else(invalid_response)?,
        None => fallback_trace_id.to_ascii_lowercase(),
    };
    if trace_id != fallback_trace_id {
        return Err(invalid_response());
    }
    let span_id = value
        .get("spanId")
        .and_then(Value::as_str)
        .ok_or_else(invalid_response)
        .and_then(|id| decode_otlp_id(id, 8).ok_or_else(invalid_response))?;
    let parent_span_id = value
        .get("parentSpanId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(|id| decode_otlp_id(id, 8).ok_or_else(invalid_response))
        .transpose()?;
    let start = value
        .get("startTimeUnixNano")
        .and_then(Value::as_str)
        .map(parse_nanos)
        .transpose()?;
    let end = value
        .get("endTimeUnixNano")
        .and_then(Value::as_str)
        .map(parse_nanos)
        .transpose()?;
    let duration_nanos = start.zip(end).and_then(|(start, end)| {
        let start = start.timestamp_nanos_opt()?;
        let end = end.timestamp_nanos_opt()?;
        u64::try_from(end.saturating_sub(start)).ok()
    });
    let status = value.pointer("/status/code").and_then(|code| match code {
        Value::Number(number) => number.as_u64().map(|code| match code {
            1 => "ok".into(),
            2 => "error".into(),
            _ => "unset".into(),
        }),
        Value::String(code) => Some(code.chars().take(32).collect()),
        _ => None,
    });
    Ok(TraceSpan {
        trace_id,
        span_id,
        parent_span_id,
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unnamed")
            .chars()
            .take(256)
            .collect(),
        start_time: start,
        end_time: end,
        duration_nanos,
        status,
        attributes: attributes(value.get("attributes")),
        resource,
    })
}

fn decode_otlp_id(value: &str, length: usize) -> Option<String> {
    if value.len() == length * 2 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some(value.to_ascii_lowercase());
    }
    let decoded = STANDARD.decode(value).ok()?;
    if decoded.len() != length {
        return None;
    }
    Some(decoded.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn attributes(value: Option<&Value>) -> BTreeMap<String, Value> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|attribute| {
            let key = attribute.get("key")?.as_str()?;
            if !allowed_attribute(key) {
                return None;
            }
            let value = attribute.get("value")?;
            let value = ["stringValue", "intValue", "doubleValue", "boolValue"]
                .into_iter()
                .find_map(|name| value.get(name).cloned())?;
            let value = match value {
                Value::String(text) => Value::String(text.chars().take(256).collect()),
                value => value,
            };
            Some((key.into(), value))
        })
        .take(32)
        .collect()
}

fn allowed_attribute(key: &str) -> bool {
    matches!(
        key,
        "service.name"
            | "service.version"
            | "telemetry.sdk.name"
            | "telemetry.sdk.language"
            | "telemetry.sdk.version"
            | "wukong.event"
            | "wukong.operation"
            | "wukong.worker"
            | "wukong.transport"
            | "run_id"
            | "agent_id"
            | "generation"
            | "turn"
            | "call_id"
            | "worker_request_id"
            | "job_id"
            | "rpc.system"
            | "rpc.method"
            | "worker.transport"
            | "worker.os"
            | "worker.arch"
            | "resource.kind"
            | "outcome"
            | "error.type"
            | "code.function.name"
            | "gen_ai.operation.name"
            | "gen_ai.provider.name"
            | "gen_ai.request.model"
            | "gen_ai.response.model"
            | "gen_ai.usage.input_tokens"
            | "gen_ai.usage.output_tokens"
    )
}

fn safe_optional_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(|value| value.chars().take(256).collect())
}

fn parse_nanos(value: &str) -> Result<DateTime<Utc>, BackendError> {
    let nanos = value.parse::<i64>().map_err(|_| invalid_response())?;
    super::timestamp_from_nanos(nanos).ok_or_else(invalid_response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn run_template_is_fixed_and_identifier_cannot_inject_traceql() {
        assert_eq!(run_query("run-1"), "{ .run_id = \"run-1\" }");
        assert!(super::super::validate_run_id("run\" } { .secret = \"x").is_err());
    }

    #[test]
    fn same_second_trace_pages_advance_without_loss_or_duplicates() {
        let timestamp = parse_nanos("1000000000").unwrap();
        let summary = |id: char| TraceSummary {
            trace_id: id.to_string().repeat(32),
            root_service_name: None,
            root_trace_name: None,
            start_time: Some(timestamp),
            duration_ms: 1,
            span_count: 1,
        };
        let source = vec![summary('c'), summary('a'), summary('b')];
        let range = super::super::TimeRange {
            start: super::super::timestamp_from_nanos(0).unwrap(),
            end: super::super::timestamp_from_nanos(2_000_000_000).unwrap(),
        };
        let first_request = PageRequest {
            range: range.clone(),
            limit: 2,
            cursor: None,
        };
        let (first, cursor, truncated) =
            paginate_traces(source.clone(), &first_request, None, false).unwrap();
        assert_eq!(
            first
                .iter()
                .map(|trace| &trace.trace_id)
                .collect::<Vec<_>>(),
            [&"a".repeat(32), &"b".repeat(32)]
        );
        assert_eq!(
            cursor.as_deref(),
            Some(format!("1000000000:{}", "b".repeat(32)).as_str())
        );
        assert!(truncated);
        let second_request = PageRequest {
            range,
            limit: 2,
            cursor,
        };
        let cursor = parse_cursor(&second_request).unwrap();
        let (second, next, truncated) =
            paginate_traces(source, &second_request, cursor.as_ref(), false).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].trace_id, "c".repeat(32));
        assert!(next.is_none());
        assert!(!truncated);
        assert!(validate_cursor(Some("1000000000"), &second_request.range).is_err());
    }

    #[test]
    fn otlp_trace_mapping_filters_attributes_and_computes_duration() {
        let trace_id = "a".repeat(32);
        let value = json!({"batches":[{
            "resource":{"attributes":[{"key":"service.name","value":{"stringValue":"wukong"}}]},
            "scopeSpans":[{"spans":[{
                "traceId":trace_id,
                "spanId":"0123456789abcdef",
                "parentSpanId":"",
                "name":"invoke_agent",
                "startTimeUnixNano":"1000000000",
                "endTimeUnixNano":"2500000000",
                "attributes":[
                    {"key":"run_id","value":{"stringValue":"run-1"}},
                    {"key":"authorization","value":{"stringValue":"secret"}}
                ],
                "status":{"code":2}
            }]}]
        }]});
        let (trace, truncated) = map_trace(&trace_id, &value).unwrap();
        assert!(!truncated);
        assert_eq!(trace.spans[0].duration_nanos, Some(1_500_000_000));
        assert_eq!(trace.spans[0].status.as_deref(), Some("error"));
        assert_eq!(trace.spans[0].attributes["run_id"], "run-1");
        assert!(!trace.spans[0].attributes.contains_key("authorization"));
        assert!(trace.belongs_to("run-1"));
        assert!(!trace.belongs_to("other"));
    }

    #[test]
    fn tempo_otlp_json_decodes_base64_trace_and_span_ids() {
        let trace_id = "e0e8a66b25927775fe6489edf40e25fe";
        let value = json!({"batches":[{
            "resource":{"attributes":[]},
            "scopeSpans":[{"spans":[{
                "traceId":"4OimayWSd3X+ZInt9A4l/g==",
                "spanId":"dWdx9+QBG8A=",
                "parentSpanId":"AAAAAAAAAAA=",
                "name":"wukong.assessment",
                "startTimeUnixNano":"1795408206714237000",
                "endTimeUnixNano":"1795408206714527000",
                "attributes":[{"key":"run_id","value":{"stringValue":"run-1"}}],
                "status":{}
            }]}]
        }]});
        let (trace, truncated) = map_trace(trace_id, &value).unwrap();
        assert!(!truncated);
        assert_eq!(trace.spans[0].trace_id, trace_id);
        assert_eq!(trace.spans[0].span_id, "756771f7e4011bc0");
        assert_eq!(
            trace.spans[0].parent_span_id.as_deref(),
            Some("0000000000000000")
        );
        assert_eq!(trace.spans[0].duration_nanos, Some(290_000));
    }
}
