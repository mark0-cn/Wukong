//! Typed transport-only telemetry envelopes. Business members remain raw JSON.

use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

pub const TELEMETRY_FIELD: &str = "_wukong_telemetry";
pub const MAX_TELEMETRY_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceContext {
    #[schemars(length(min = 55, max = 55))]
    pub traceparent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 512))]
    pub tracestate: Option<String>,
}

impl TraceContext {
    pub fn is_valid(&self) -> bool {
        valid_traceparent(&self.traceparent)
            && self.tracestate.as_ref().is_none_or(|state| {
                state.len() <= 512
                    && !state.contains(['\r', '\n'])
                    && state
                        .bytes()
                        .all(|byte| byte.is_ascii_graphic() || byte == b' ')
            })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TelemetryBatch {
    /// Base64-encoded OTLP ExportTraceServiceRequest protobuf.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traces: Option<String>,
    /// Base64-encoded OTLP ExportLogsServiceRequest protobuf.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logs: Option<String>,
}

impl TelemetryBatch {
    pub fn is_empty(&self) -> bool {
        self.traces.is_none() && self.logs.is_none()
    }

    pub fn is_bounded(&self) -> bool {
        serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= MAX_TELEMETRY_BYTES)
            && [&self.traces, &self.logs]
                .into_iter()
                .flatten()
                .all(|payload| {
                    payload.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')
                    })
                })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeStatus {
    Attached,
    Unchanged,
    Oversize,
    Invalid,
}

#[derive(Debug)]
pub struct EnvelopeTransform {
    pub message: String,
    pub status: EnvelopeStatus,
}

#[derive(Debug)]
pub struct ExtractedTelemetry {
    pub message: String,
    pub batch: Option<TelemetryBatch>,
    pub dropped: bool,
}

#[derive(Debug)]
pub struct ExtractedTraceContext {
    pub message: String,
    pub context: Option<TraceContext>,
    pub dropped: bool,
}

type Fields = BTreeMap<String, Box<RawValue>>;

pub fn attach_trace_context(
    request: &str,
    context: Option<&TraceContext>,
) -> Result<EnvelopeTransform> {
    let Some(context) = context else {
        return Ok(EnvelopeTransform {
            message: request.to_owned(),
            status: EnvelopeStatus::Unchanged,
        });
    };
    if !context.is_valid() {
        return Ok(EnvelopeTransform {
            message: request.to_owned(),
            status: EnvelopeStatus::Invalid,
        });
    }
    let mut fields = decode_fields(request)?;
    let context = RawValue::from_string(serde_json::to_string(context)?)?;
    fields.insert(TELEMETRY_FIELD.into(), context);
    attach_fields(request, fields)
}

pub fn extract_trace_context(request: &str) -> Result<ExtractedTraceContext> {
    let mut fields = decode_fields(request)?;
    let Some(raw) = fields.remove(TELEMETRY_FIELD) else {
        return Ok(ExtractedTraceContext {
            message: request.to_owned(),
            context: None,
            dropped: false,
        });
    };
    let context = serde_json::from_str::<TraceContext>(raw.get())
        .ok()
        .filter(TraceContext::is_valid);
    let dropped = context.is_none();
    Ok(ExtractedTraceContext {
        message: serde_json::to_string(&fields)?,
        context,
        dropped,
    })
}

pub fn extract_response_telemetry(response: &str) -> Result<ExtractedTelemetry> {
    let mut fields = decode_fields(response)?;
    let Some(raw) = fields.remove(TELEMETRY_FIELD) else {
        return Ok(ExtractedTelemetry {
            message: response.to_owned(),
            batch: None,
            dropped: false,
        });
    };
    let batch = serde_json::from_str::<TelemetryBatch>(raw.get())
        .ok()
        .filter(|batch| !batch.is_empty() && batch.is_bounded());
    let dropped = batch.is_none();
    Ok(ExtractedTelemetry {
        message: serde_json::to_string(&fields)?,
        batch,
        dropped,
    })
}

pub fn attach_response_telemetry(
    response: &str,
    batch: &TelemetryBatch,
) -> Result<EnvelopeTransform> {
    if batch.is_empty() {
        return Ok(EnvelopeTransform {
            message: response.to_owned(),
            status: EnvelopeStatus::Unchanged,
        });
    }
    if !batch.is_bounded() {
        return Ok(EnvelopeTransform {
            message: response.to_owned(),
            status: EnvelopeStatus::Oversize,
        });
    }
    let mut fields = decode_fields(response)?;
    let batch = RawValue::from_string(serde_json::to_string(batch)?)?;
    fields.insert(TELEMETRY_FIELD.into(), batch);
    attach_fields(response, fields)
}

fn attach_fields(original: &str, fields: Fields) -> Result<EnvelopeTransform> {
    let message = serde_json::to_string(&fields)?;
    if message.len() > super::MAX_RPC_BYTES {
        Ok(EnvelopeTransform {
            message: original.to_owned(),
            status: EnvelopeStatus::Oversize,
        })
    } else {
        Ok(EnvelopeTransform {
            message,
            status: EnvelopeStatus::Attached,
        })
    }
}

fn decode_fields(message: &str) -> Result<Fields> {
    serde_json::from_str(message).context("JSON-RPC envelope must be an object")
}

fn valid_traceparent(value: &str) -> bool {
    let parts: Vec<_> = value.split('-').collect();
    if parts.len() != 4
        || parts[0].len() != 2
        || parts[1].len() != 32
        || parts[2].len() != 16
        || parts[3].len() != 2
        || !parts.iter().all(|part| {
            part.bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
    {
        return false;
    }
    parts[0] != "ff"
        && parts[1].bytes().any(|byte| byte != b'0')
        && parts[2].bytes().any(|byte| byte != b'0')
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn request_injection_preserves_business_members() {
        let request =
            r#"{"jsonrpc":"2.0","id":7,"method":"file.put","params":{"text":"  unchanged\n"}}"#;
        let context = TraceContext {
            traceparent: PARENT.into(),
            tracestate: None,
        };
        let injected = attach_trace_context(request, Some(&context)).unwrap();
        assert_eq!(injected.status, EnvelopeStatus::Attached);
        let before: Fields = serde_json::from_str(request).unwrap();
        let after: Fields = serde_json::from_str(&injected.message).unwrap();
        for name in ["jsonrpc", "id", "method", "params"] {
            assert_eq!(before[name].get(), after[name].get());
        }
    }

    #[test]
    fn invalid_telemetry_never_replaces_business_messages() {
        let request = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let invalid = TraceContext {
            traceparent: "invalid".into(),
            tracestate: None,
        };
        assert_eq!(
            attach_trace_context(request, Some(&invalid))
                .unwrap()
                .status,
            EnvelopeStatus::Invalid
        );
        let oversized_state = TraceContext {
            traceparent: PARENT.into(),
            tracestate: Some(format!("a={}", "x".repeat(513))),
        };
        let transformed = attach_trace_context(request, Some(&oversized_state)).unwrap();
        assert_eq!(transformed.message, request);
        assert_eq!(transformed.status, EnvelopeStatus::Invalid);
    }

    #[test]
    fn valid_context_cannot_push_a_maximum_size_request_over_the_limit() {
        let prefix = r#"{"jsonrpc":"2.0","id":1,"method":"file.put","params":{"data":""#;
        let suffix = r#""}}"#;
        let request = format!(
            "{prefix}{}{suffix}",
            "x".repeat(super::super::MAX_RPC_BYTES - prefix.len() - suffix.len())
        );
        assert_eq!(request.len(), super::super::MAX_RPC_BYTES);
        let context = TraceContext {
            traceparent: PARENT.into(),
            tracestate: Some("vendor=opaque".into()),
        };
        let transformed = attach_trace_context(&request, Some(&context)).unwrap();
        assert_eq!(transformed.status, EnvelopeStatus::Oversize);
        assert_eq!(transformed.message, request);
    }

    #[test]
    fn response_batch_is_removed_before_business_dispatch() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        let batch = TelemetryBatch {
            traces: Some("AA==".into()),
            logs: None,
        };
        let attached = attach_response_telemetry(response, &batch).unwrap();
        let extracted = extract_response_telemetry(&attached.message).unwrap();
        assert_eq!(extracted.batch, Some(batch));
        let value: serde_json::Value = serde_json::from_str(&extracted.message).unwrap();
        assert_eq!(value["result"]["ok"], true);
        assert!(value.get(TELEMETRY_FIELD).is_none());
    }
}
