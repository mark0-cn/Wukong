//! Bounded, credential-free worker telemetry ingress and controller-owned OTLP egress.
//!
//! A worker response is never delayed by this module. Protobuf payloads are
//! decoded and reduced to an explicit safe schema before they leave the host.

use super::{ObservabilityConfig, Signal, TelemetryHandle};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use opentelemetry_proto::tonic::{
    collector::{logs::v1::ExportLogsServiceRequest, trace::v1::ExportTraceServiceRequest},
    common::v1::{AnyValue, KeyValue, any_value},
    logs::v1::LogRecord,
    trace::v1::Span,
};
use prost::Message;
use reqwest::{
    Client, StatusCode, Url,
    header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};
use std::time::Duration;
use tokio::{sync::mpsc, task::JoinHandle};
use wukong_worker_protocol::{MAX_TELEMETRY_BYTES, TelemetryBatch};

const QUEUE_BATCHES: usize = 16;
const MAX_SPANS: usize = 256;
const MAX_LOG_RECORDS: usize = 512;
const MAX_ATTRIBUTES: usize = 64;
const MAX_NAME_BYTES: usize = 128;
const MAX_VALUE_BYTES: usize = 512;

pub(super) struct WorkerRelay {
    sender: Option<mpsc::Sender<TelemetryBatch>>,
    task: JoinHandle<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EnqueueError {
    Full,
    Closed,
}

impl WorkerRelay {
    pub(super) fn start(config: &ObservabilityConfig, telemetry: TelemetryHandle) -> Result<Self> {
        let traces = match target(config, Signal::Traces) {
            Ok(target) => target,
            Err(_) => {
                telemetry.relay_result(
                    "traces",
                    Some("worker traces relay initialization failed".into()),
                );
                None
            }
        };
        let logs = match target(config, Signal::Logs) {
            Ok(target) => target,
            Err(_) => {
                telemetry.relay_result(
                    "logs",
                    Some("worker logs relay initialization failed".into()),
                );
                None
            }
        };
        if traces.is_none() && logs.is_none() {
            bail!("worker relay has no valid enabled OTLP signals");
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .context("build worker telemetry HTTP client")?;
        let (sender, mut receiver) = mpsc::channel::<TelemetryBatch>(QUEUE_BATCHES);
        let task = tokio::spawn(async move {
            while let Some(batch) = receiver.recv().await {
                if let Some(encoded) = batch.traces {
                    if let Some(target) = &traces {
                        export_traces(&client, target, encoded, &telemetry).await;
                    } else {
                        telemetry.dropped("traces", "relay_signal_disabled");
                    }
                }
                if let Some(encoded) = batch.logs {
                    if let Some(target) = &logs {
                        export_logs(&client, target, encoded, &telemetry).await;
                    } else {
                        telemetry.dropped("logs", "relay_signal_disabled");
                    }
                }
            }
        });
        Ok(Self {
            sender: Some(sender),
            task,
        })
    }

    pub(super) fn enqueue(&self, batch: TelemetryBatch) -> Result<(), EnqueueError> {
        self.sender
            .as_ref()
            .ok_or(EnqueueError::Closed)?
            .try_send(batch)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => EnqueueError::Full,
                mpsc::error::TrySendError::Closed(_) => EnqueueError::Closed,
            })
    }

    pub(super) async fn shutdown(&mut self) {
        self.sender.take();
        let _ = (&mut self.task).await;
    }

    pub(super) fn stop(&self) {
        self.task.abort();
    }
}

struct ExportTarget {
    endpoint: Url,
    headers: HeaderMap,
}

fn target(config: &ObservabilityConfig, signal: Signal) -> Result<Option<ExportTarget>> {
    if !config.signal_enabled(signal) || !config.signal_configured(signal) {
        return Ok(None);
    }
    let endpoint = config
        .exporter_endpoint(signal)?
        .context("configured worker relay signal has no endpoint")?;
    let mut headers = HeaderMap::new();
    for (name, value) in config.exporter_headers(signal)? {
        let name =
            HeaderName::from_bytes(name.as_bytes()).context("OTLP header name is invalid")?;
        if matches!(
            name,
            CONTENT_TYPE | reqwest::header::CONTENT_LENGTH | reqwest::header::HOST
        ) {
            bail!("OTLP headers cannot override content type, content length or host");
        }
        let value = HeaderValue::from_str(&value).context("OTLP header value is invalid")?;
        headers.insert(name, value);
    }
    Ok(Some(ExportTarget {
        endpoint: Url::parse(&endpoint).context("OTLP endpoint is invalid")?,
        headers,
    }))
}

async fn export_traces(
    client: &Client,
    target: &ExportTarget,
    encoded: String,
    telemetry: &TelemetryHandle,
) {
    let prepared = decode(&encoded).and_then(|bytes| {
        let request = ExportTraceServiceRequest::decode(bytes.as_slice())
            .context("worker trace protobuf is invalid")?;
        sanitize_traces(request)
    });
    match prepared {
        Ok(request) => {
            send(client, target, request, "traces", telemetry).await;
        }
        Err(_) => telemetry.dropped("traces", "invalid_worker_payload"),
    }
}

async fn export_logs(
    client: &Client,
    target: &ExportTarget,
    encoded: String,
    telemetry: &TelemetryHandle,
) {
    let prepared = decode(&encoded).and_then(|bytes| {
        let request = ExportLogsServiceRequest::decode(bytes.as_slice())
            .context("worker log protobuf is invalid")?;
        sanitize_logs(request)
    });
    match prepared {
        Ok(request) => send(client, target, request, "logs", telemetry).await,
        Err(_) => telemetry.dropped("logs", "invalid_worker_payload"),
    }
}

fn decode(encoded: &str) -> Result<Vec<u8>> {
    ensure!(
        encoded.len() <= MAX_TELEMETRY_BYTES,
        "worker telemetry payload exceeds its encoded limit"
    );
    STANDARD
        .decode(encoded)
        .context("worker telemetry is not canonical base64")
}

async fn send<M: Message>(
    client: &Client,
    target: &ExportTarget,
    message: M,
    signal: &'static str,
    telemetry: &TelemetryHandle,
) {
    let result = client
        .post(target.endpoint.clone())
        .headers(target.headers.clone())
        .header(CONTENT_TYPE, "application/x-protobuf")
        .body(message.encode_to_vec())
        .send()
        .await;
    match result {
        Ok(response) if response.status().is_success() => telemetry.relay_result(signal, None),
        Ok(response) => {
            telemetry.dropped(signal, "relay_http_status");
            telemetry.relay_result(signal, Some(status_diagnostic(response.status())));
        }
        Err(_) => {
            telemetry.dropped(signal, "relay_transport");
            telemetry.relay_result(
                signal,
                Some(format!("worker {signal} export transport failed")),
            );
        }
    }
}

fn status_diagnostic(status: StatusCode) -> String {
    format!("worker telemetry backend returned HTTP {}", status.as_u16())
}

fn sanitize_traces(mut request: ExportTraceServiceRequest) -> Result<ExportTraceServiceRequest> {
    ensure!(
        request.resource_spans.len() <= 16,
        "too many trace resources"
    );
    let mut count = 0usize;
    for resource in &mut request.resource_spans {
        ensure!(resource.scope_spans.len() <= 32, "too many trace scopes");
        resource.schema_url.clear();
        if let Some(resource) = &mut resource.resource {
            resource.attributes = safe_attributes(std::mem::take(&mut resource.attributes), true);
        }
        for scope in &mut resource.scope_spans {
            // Scope identity is free-form OTLP metadata, not part of the safe
            // Worker contract. Resource SDK identity is allowlisted separately.
            scope.scope = None;
            scope.schema_url.clear();
            count = count.saturating_add(scope.spans.len());
            ensure!(count <= MAX_SPANS, "too many worker spans");
            for span in &mut scope.spans {
                sanitize_span(span)?;
            }
        }
    }
    Ok(request)
}

fn sanitize_span(span: &mut Span) -> Result<()> {
    ensure!(valid_id(&span.trace_id, 16), "invalid worker trace ID");
    ensure!(valid_id(&span.span_id, 8), "invalid worker span ID");
    ensure!(
        span.parent_span_id.is_empty() || valid_id(&span.parent_span_id, 8),
        "invalid worker parent span ID"
    );
    span.name = safe_text(&span.name, MAX_NAME_BYTES);
    ensure!(!span.name.is_empty(), "worker span name is empty");
    span.trace_state = safe_text(&span.trace_state, MAX_VALUE_BYTES);
    span.attributes = safe_attributes(std::mem::take(&mut span.attributes), false);
    // Event bodies, exception stacks and links are not part of the worker
    // telemetry contract. Their absence cannot affect the business response.
    span.events.clear();
    span.links.clear();
    if let Some(status) = &mut span.status {
        status.message.clear();
    }
    Ok(())
}

fn sanitize_logs(mut request: ExportLogsServiceRequest) -> Result<ExportLogsServiceRequest> {
    ensure!(request.resource_logs.len() <= 16, "too many log resources");
    let mut count = 0usize;
    for resource in &mut request.resource_logs {
        ensure!(resource.scope_logs.len() <= 32, "too many log scopes");
        resource.schema_url.clear();
        if let Some(resource) = &mut resource.resource {
            resource.attributes = safe_attributes(std::mem::take(&mut resource.attributes), true);
        }
        for scope in &mut resource.scope_logs {
            scope.scope = None;
            scope.schema_url.clear();
            count = count.saturating_add(scope.log_records.len());
            ensure!(count <= MAX_LOG_RECORDS, "too many worker log records");
            for record in &mut scope.log_records {
                sanitize_log(record);
            }
        }
    }
    Ok(request)
}

fn sanitize_log(record: &mut LogRecord) {
    record.body = None;
    record.event_name.clear();
    record.attributes = safe_attributes(std::mem::take(&mut record.attributes), false);
    record.severity_text = safe_text(&record.severity_text, 16);
    if !record.trace_id.is_empty() && !valid_id(&record.trace_id, 16) {
        record.trace_id.clear();
        record.span_id.clear();
    }
    if !record.span_id.is_empty() && !valid_id(&record.span_id, 8) {
        record.span_id.clear();
    }
}

fn valid_id(id: &[u8], length: usize) -> bool {
    id.len() == length && id.iter().any(|byte| *byte != 0)
}

fn safe_attributes(attributes: Vec<KeyValue>, resource: bool) -> Vec<KeyValue> {
    attributes
        .into_iter()
        .take(MAX_ATTRIBUTES)
        .filter_map(|mut attribute| {
            if !allowed_attribute(&attribute.key, resource) {
                return None;
            }
            attribute.key = safe_text(&attribute.key, 64);
            attribute.value = attribute.value.and_then(safe_value);
            attribute.value.is_some().then_some(attribute)
        })
        .collect()
}

fn allowed_attribute(key: &str, resource: bool) -> bool {
    if resource {
        return matches!(
            key,
            "service.name"
                | "service.version"
                | "telemetry.sdk.name"
                | "telemetry.sdk.language"
                | "telemetry.sdk.version"
                | "os.type"
                | "process.runtime.name"
                | "process.runtime.version"
        );
    }
    matches!(
        key,
        "wukong.event"
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
    )
}

fn safe_value(mut value: AnyValue) -> Option<AnyValue> {
    value.value = match value.value? {
        any_value::Value::StringValue(value) => Some(any_value::Value::StringValue(safe_text(
            &value,
            MAX_VALUE_BYTES,
        ))),
        value @ (any_value::Value::BoolValue(_)
        | any_value::Value::IntValue(_)
        | any_value::Value::DoubleValue(_)) => Some(value),
        _ => None,
    };
    Some(value)
}

fn safe_text(value: &str, limit: usize) -> String {
    let mut result = String::with_capacity(value.len().min(limit));
    for character in value.chars() {
        if result.len() >= limit {
            break;
        }
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        let mut encoded = [0; 4];
        let encoded = character.encode_utf8(&mut encoded);
        if result.len() + encoded.len() > limit {
            break;
        }
        result.push_str(encoded);
    }
    result.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::{
        common::v1::InstrumentationScope,
        resource::v1::Resource,
        trace::v1::{ResourceSpans, ScopeSpans, Status},
    };

    fn string_value(value: &str) -> Option<AnyValue> {
        Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.into())),
        })
    }

    fn span() -> Span {
        Span {
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            name: "worker.dispatch".into(),
            ..Span::default()
        }
    }

    #[test]
    fn sanitization_preserves_correlation_and_removes_sensitive_content() -> Result<()> {
        let mut span = span();
        span.attributes = vec![
            KeyValue {
                key: "run_id".into(),
                value: string_value("run-1"),
                ..KeyValue::default()
            },
            KeyValue {
                key: "tool.arguments".into(),
                value: string_value("secret-sentinel"),
                ..KeyValue::default()
            },
            KeyValue {
                key: "exception.message".into(),
                value: string_value("C:\\private\\target"),
                ..KeyValue::default()
            },
        ];
        span.events
            .push(opentelemetry_proto::tonic::trace::v1::span::Event {
                name: "secret-sentinel".into(),
                ..Default::default()
            });
        span.status = Some(Status {
            message: "secret-sentinel".into(),
            ..Status::default()
        });
        let request = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "process.command_args".into(),
                        value: string_value("secret-sentinel"),
                        ..KeyValue::default()
                    }],
                    ..Resource::default()
                }),
                scope_spans: vec![ScopeSpans {
                    scope: Some(InstrumentationScope {
                        name: "secret-sentinel".into(),
                        version: "secret-sentinel".into(),
                        attributes: vec![KeyValue {
                            key: "authorization".into(),
                            value: string_value("secret-sentinel"),
                            ..KeyValue::default()
                        }],
                        ..InstrumentationScope::default()
                    }),
                    spans: vec![span],
                    schema_url: "https://untrusted.example/schema".into(),
                }],
                schema_url: "https://untrusted.example/schema".into(),
            }],
        };
        let encoded = sanitize_traces(request)?.encode_to_vec();
        let serialized = String::from_utf8_lossy(&encoded);
        assert!(!serialized.contains("secret-sentinel"));
        assert!(!serialized.contains("untrusted.example"));
        let clean = ExportTraceServiceRequest::decode(encoded.as_slice())?;
        assert!(clean.resource_spans[0].scope_spans[0].scope.is_none());
        let span = &clean.resource_spans[0].scope_spans[0].spans[0];
        assert_eq!(span.attributes.len(), 1);
        assert_eq!(span.attributes[0].key, "run_id");
        assert!(span.events.is_empty());
        assert_eq!(
            span.status.as_ref().map(|status| status.message.as_str()),
            Some("")
        );
        Ok(())
    }

    #[test]
    fn log_sanitization_removes_scope_body_and_event_name() -> Result<()> {
        use opentelemetry_proto::tonic::logs::v1::{ResourceLogs, ScopeLogs};

        let request = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                scope_logs: vec![ScopeLogs {
                    scope: Some(InstrumentationScope {
                        name: "secret-sentinel".into(),
                        ..InstrumentationScope::default()
                    }),
                    log_records: vec![LogRecord {
                        body: string_value("secret-sentinel"),
                        event_name: "secret-sentinel".into(),
                        severity_text: "INFO".into(),
                        ..LogRecord::default()
                    }],
                    ..ScopeLogs::default()
                }],
                ..ResourceLogs::default()
            }],
        };
        let clean = sanitize_logs(request)?;
        let scope = &clean.resource_logs[0].scope_logs[0];
        assert!(scope.scope.is_none());
        let record = &scope.log_records[0];
        assert!(record.body.is_none());
        assert!(record.event_name.is_empty());
        assert_eq!(record.severity_text, "INFO");
        Ok(())
    }

    #[test]
    fn invalid_trace_identity_rejects_the_whole_side_channel_batch() {
        let request = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans: vec![Span::default()],
                    ..ScopeSpans::default()
                }],
                ..ResourceSpans::default()
            }],
        };
        assert!(sanitize_traces(request).is_err());
    }

    #[tokio::test]
    async fn http_export_uses_standard_protobuf_and_controller_credentials() -> Result<()> {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut received = Vec::new();
            let mut buffer = [0u8; 4096];
            let header_end = loop {
                let read = socket.read(&mut buffer).await?;
                if read == 0 {
                    bail!("connection ended before HTTP headers");
                }
                received.extend_from_slice(&buffer[..read]);
                if let Some(position) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            let headers = String::from_utf8_lossy(&received[..header_end]).to_ascii_lowercase();
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .context("missing content length")?
                .trim()
                .parse::<usize>()?;
            while received.len() < header_end + length {
                let read = socket.read(&mut buffer).await?;
                if read == 0 {
                    bail!("connection ended before HTTP body");
                }
                received.extend_from_slice(&buffer[..read]);
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await?;
            Ok::<_, anyhow::Error>((headers, received[header_end..header_end + length].to_vec()))
        });
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer controller-test-token"),
        );
        let target = ExportTarget {
            endpoint: Url::parse(&format!("http://{address}/v1/traces"))?,
            headers,
        };
        let request = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans: vec![span()],
                    ..ScopeSpans::default()
                }],
                ..ResourceSpans::default()
            }],
        };
        let telemetry = crate::adapters::telemetry::TelemetryRuntime::new();
        send(
            &Client::new(),
            &target,
            request,
            "traces",
            &telemetry.handle(),
        )
        .await;
        let (headers, body) = server.await??;
        assert!(headers.contains("authorization: bearer controller-test-token"));
        assert!(headers.contains("content-type: application/x-protobuf"));
        assert!(headers.starts_with("post /v1/traces http/1.1"));
        let decoded = ExportTraceServiceRequest::decode(body.as_slice())?;
        assert_eq!(
            decoded.resource_spans[0].scope_spans[0].spans[0].span_id,
            vec![2; 8]
        );
        assert_eq!(
            telemetry
                .handle()
                .status()
                .worker_relay_status
                .traces
                .healthy,
            Some(true)
        );
        Ok(())
    }

    #[test]
    fn decoder_rejects_oversized_and_malformed_payloads() {
        assert!(decode(&"A".repeat(MAX_TELEMETRY_BYTES + 1)).is_err());
        assert!(decode("not base64!").is_err());
    }
}
