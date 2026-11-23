//! Process-owned trace assembly. Completed spans stay in a bounded local queue;
//! the RPC envelope is the only export path and no network exporter is configured.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use opentelemetry::{KeyValue, propagation::TextMapPropagator as _, trace::TracerProvider as _};
use opentelemetry_proto::{
    tonic::collector::trace::v1::ExportTraceServiceRequest,
    transform::{
        common::tonic::ResourceAttributesWithSchema,
        trace::tonic::group_spans_by_resource_and_scope,
    },
};
use opentelemetry_sdk::{
    Resource,
    propagation::TraceContextPropagator,
    trace::{SdkTracerProvider, SpanData, SpanExporter},
};
use prost::Message as _;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::{Dispatch, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::{Layer as _, layer::SubscriberExt as _};
use wukong_worker_protocol::{TelemetryBatch, TraceContext};

const MAX_QUEUE_BYTES: usize = 256 * 1024;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const SPAN_TARGET: &str = "wukong_windows_worker::rpc";

#[derive(Clone)]
pub struct Telemetry {
    queue: Arc<Queue>,
    dispatch: Dispatch,
}

/// The process entry point exclusively owns provider shutdown. RPC clients only
/// receive a cloneable handle and cannot flush or stop the process provider.
pub struct TelemetryGuard {
    provider: Option<SdkTracerProvider>,
}

impl Telemetry {
    /// Assemble one local trace pipeline. No endpoint, credential or environment
    /// variable is read; the fixed resource contains only package identity.
    pub fn new() -> (Self, TelemetryGuard) {
        let queue = Arc::new(Queue::default());
        let exporter = QueueExporter {
            queue: queue.clone(),
            resource: ResourceAttributesWithSchema::default(),
        };
        let provider = SdkTracerProvider::builder()
            .with_resource(
                Resource::builder_empty()
                    .with_service_name("wukong-windows-worker")
                    .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
                    .build(),
            )
            .with_simple_exporter(exporter)
            .build();
        let layer = tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("wukong-windows-worker"))
            .with_location(false)
            .with_threads(false)
            .with_target(false)
            .with_tracked_inactivity(false)
            .with_filter(
                tracing_subscriber::filter::Targets::new()
                    .with_target(SPAN_TARGET, tracing::Level::TRACE),
            );
        let dispatch = Dispatch::new(tracing_subscriber::registry().with(layer));
        (
            Self { queue, dispatch },
            TelemetryGuard {
                provider: Some(provider),
            },
        )
    }

    /// Preserve the existing business RPC path for callers that do not assemble
    /// telemetry, notably unrelated native unit tests.
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self {
            queue: Arc::new(Queue::default()),
            dispatch: Dispatch::new(tracing_subscriber::registry()),
        }
    }

    pub(crate) fn dispatcher(&self) -> Dispatch {
        self.dispatch.clone()
    }

    pub(crate) fn server_span(&self, parent: Option<&TraceContext>) -> Span {
        let parent = parent.and_then(remote_parent);
        tracing::dispatcher::with_default(&self.dispatch, || {
            let span = tracing::info_span!(
                target: SPAN_TARGET,
                "rpc.server",
                otel.name = "rpc.server",
                otel.kind = "server",
                rpc.system = "jsonrpc"
            );
            if let Some(parent) = parent {
                let _ = span.set_parent(parent);
            }
            span
        })
    }

    /// Drain only completed exporter batches. Invalid queued bytes are dropped;
    /// telemetry can never replace or fail a business response.
    pub(crate) fn response_batch(&self) -> TelemetryBatch {
        self.queue.drain_batch()
    }
}

impl TelemetryGuard {
    /// The simple exporter is synchronous and non-blocking; the SDK timeout also
    /// bounds its shutdown contract. Drop is an idempotent fallback for exits.
    pub fn shutdown(&mut self) {
        let Some(provider) = self.provider.take() else {
            return;
        };
        let _ = provider.force_flush();
        let _ = provider.shutdown_with_timeout(SHUTDOWN_TIMEOUT);
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn remote_parent(context: &TraceContext) -> Option<opentelemetry::Context> {
    if !context.is_valid() {
        return None;
    }
    let mut headers = HashMap::from([("traceparent".into(), context.traceparent.clone())]);
    if let Some(state) = &context.tracestate {
        headers.insert("tracestate".into(), state.clone());
    }
    let parent = TraceContextPropagator::new().extract(&headers);
    opentelemetry::trace::TraceContextExt::span(&parent)
        .span_context()
        .is_valid()
        .then_some(parent)
}

#[derive(Debug)]
struct QueueExporter {
    queue: Arc<Queue>,
    resource: ResourceAttributesWithSchema,
}

impl SpanExporter for QueueExporter {
    fn export(
        &self,
        batch: Vec<SpanData>,
    ) -> impl std::future::Future<Output = opentelemetry_sdk::error::OTelSdkResult> + Send {
        let request = ExportTraceServiceRequest {
            resource_spans: group_spans_by_resource_and_scope(batch, &self.resource),
        };
        self.queue.push(request.encode_to_vec());
        std::future::ready(Ok(()))
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.resource = resource.into();
    }
}

#[derive(Debug, Default)]
struct Queue {
    state: Mutex<QueueState>,
}

#[derive(Debug, Default)]
struct QueueState {
    encoded: VecDeque<Vec<u8>>,
    bytes: usize,
}

impl Queue {
    fn push(&self, encoded: Vec<u8>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if encoded.is_empty()
            || encoded.len() > MAX_QUEUE_BYTES
            || state.bytes.saturating_add(encoded.len()) > MAX_QUEUE_BYTES
        {
            return;
        }
        state.bytes += encoded.len();
        state.encoded.push_back(encoded);
    }

    fn drain_batch(&self) -> TelemetryBatch {
        let encoded = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.bytes = 0;
            std::mem::take(&mut state.encoded)
        };
        if encoded.is_empty() {
            return TelemetryBatch::default();
        }
        let mut merged = ExportTraceServiceRequest::default();
        for bytes in encoded {
            if let Ok(mut request) = ExportTraceServiceRequest::decode(bytes.as_slice()) {
                merged.resource_spans.append(&mut request.resource_spans);
            }
        }
        if merged.resource_spans.is_empty() {
            return TelemetryBatch::default();
        }
        TelemetryBatch {
            traces: Some(STANDARD.encode(merged.encode_to_vec())),
            logs: None,
        }
    }

    #[cfg(test)]
    fn queued_bytes(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_rejects_overflow_without_affecting_existing_bytes() {
        let queue = Queue::default();
        queue.push(vec![1; MAX_QUEUE_BYTES]);
        queue.push(vec![2]);
        assert_eq!(queue.queued_bytes(), MAX_QUEUE_BYTES);
        let _ = queue.drain_batch();
        assert_eq!(queue.queued_bytes(), 0);
        queue.push(vec![3; MAX_QUEUE_BYTES + 1]);
        assert_eq!(queue.queued_bytes(), 0);
    }

    #[test]
    fn invalid_remote_context_never_becomes_a_parent() {
        let context = TraceContext {
            traceparent: "invalid".into(),
            tracestate: None,
        };
        assert!(remote_parent(&context).is_none());
    }
}
