//! Own the relay child, framed streams, and bounded stderr diagnostics.
use crate::adapters::process_io::StderrCapture;
use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use jsonrpsee_core::{
    client::{
        Client, ClientBuilder, ReceivedMessage, TransportReceiverT, TransportSenderT,
        async_client::RpcService,
    },
    middleware::{Batch, Notification, Request, RpcServiceBuilder, RpcServiceT},
};
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
use wukong_worker_protocol::{MAX_RPC_BYTES, TraceContext};

const MAX_PENDING_CONTEXTS: usize = 4;

pub(super) struct StdioChannel {
    child: Child,
    stderr: StderrCapture,
    pub(super) client: Client<TelemetryService<RpcService>>,
}

impl StdioChannel {
    pub(super) fn spawn(
        mut cmd: Command,
        request_timeout_secs: u64,
        telemetry: crate::adapters::telemetry::TelemetryHandle,
    ) -> Result<Self> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd.spawn().context("spawn runtime bridge")?;
        let stderr = StderrCapture::new(child.stderr.take().context("worker stderr unavailable")?);
        let input = child.stdin.take().context("worker stdin unavailable")?;
        let output = child.stdout.take().context("worker stdout unavailable")?;
        let contexts = PendingContexts::new(telemetry.clone());
        let middleware = RpcServiceBuilder::new().layer_fn({
            let contexts = contexts.clone();
            move |inner| TelemetryService {
                inner,
                contexts: contexts.clone(),
            }
        });
        let client = ClientBuilder::default()
            .request_timeout(Duration::from_secs(request_timeout_secs.clamp(1, 3600)))
            .max_concurrent_requests(1)
            .set_rpc_middleware(middleware)
            .build_with_tokio(
                StdioSender {
                    writer: FramedWrite::new(input, LinesCodec::new_with_max_length(MAX_RPC_BYTES)),
                    contexts: contexts.clone(),
                },
                StdioReceiver {
                    reader: FramedRead::new(output, LinesCodec::new_with_max_length(MAX_RPC_BYTES)),
                    telemetry,
                },
            );
        Ok(Self {
            child,
            stderr,
            client,
        })
    }

    pub(super) async fn close(&mut self) -> std::io::Result<()> {
        self.child.kill().await
    }

    pub(super) async fn close_after_failure(&mut self) {
        let _ = self.close().await;
        self.stderr.finish().await;
    }

    pub(super) fn diagnostic(&self, label: &str) -> String {
        self.stderr.context(label)
    }
}

#[derive(Clone)]
pub(super) struct TelemetryService<S> {
    inner: S,
    contexts: PendingContexts,
}

impl<S: RpcServiceT> RpcServiceT for TelemetryService<S> {
    type MethodResponse = S::MethodResponse;
    type NotificationResponse = S::NotificationResponse;
    type BatchResponse = S::BatchResponse;

    fn call<'a>(
        &self,
        request: Request<'a>,
    ) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
        // jsonrpsee invokes middleware in the caller future, before its detached
        // transport task loses the tracing span. Bind context to the actual RPC ID.
        self.contexts.capture(&request);
        self.inner.call(request)
    }

    fn batch<'a>(
        &self,
        requests: Batch<'a>,
    ) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
        self.inner.batch(requests)
    }

    fn notification<'a>(
        &self,
        notification: Notification<'a>,
    ) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
        self.inner.notification(notification)
    }
}

#[derive(Clone)]
struct PendingContexts {
    inner: Arc<Mutex<HashMap<String, TraceContext>>>,
    telemetry: crate::adapters::telemetry::TelemetryHandle,
}

impl PendingContexts {
    fn new(telemetry: crate::adapters::telemetry::TelemetryHandle) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            telemetry,
        }
    }

    fn capture(&self, request: &Request<'_>) {
        let Some(context) = current_trace_context() else {
            return;
        };
        let Ok(id) = serde_json::to_string(&request.id) else {
            self.telemetry.dropped("traces", "invalid_rpc_id");
            return;
        };
        self.insert(id, context);
    }

    fn insert(&self, id: String, context: TraceContext) {
        let mut pending = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.len() >= MAX_PENDING_CONTEXTS && !pending.contains_key(&id) {
            self.telemetry.dropped("traces", "rpc_context_queue_full");
            return;
        }
        pending.insert(id, context);
    }

    fn attach(&self, message: String) -> String {
        let Some(context) = self.take(&message) else {
            return message;
        };
        match wukong_worker_protocol::attach_trace_context(&message, Some(&context)) {
            Ok(transformed) => {
                if transformed.status != wukong_worker_protocol::EnvelopeStatus::Attached {
                    self.telemetry.dropped("traces", "rpc_envelope_limit");
                }
                transformed.message
            }
            Err(_) => {
                self.telemetry.dropped("traces", "invalid_rpc_envelope");
                message
            }
        }
    }

    fn take(&self, message: &str) -> Option<TraceContext> {
        {
            let pending = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if pending.is_empty() {
                return None;
            }
        }
        let id = serde_json::from_str::<serde_json::Value>(message)
            .ok()?
            .get("id")
            .and_then(|id| serde_json::to_string(id).ok())?;
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id)
    }
}

fn current_trace_context() -> Option<TraceContext> {
    use opentelemetry::{global, trace::TraceContextExt as _};
    use tracing_opentelemetry::OpenTelemetrySpanExt as _;

    let current = tracing::Span::current().context();
    if !current.span().span_context().is_valid() {
        return None;
    }
    let mut headers = std::collections::HashMap::new();
    global::get_text_map_propagator(|propagator| propagator.inject_context(&current, &mut headers));
    let traceparent = headers.remove("traceparent")?;
    if traceparent.is_empty() {
        return None;
    }
    Some(TraceContext {
        traceparent,
        tracestate: headers.remove("tracestate"),
    })
}

struct StdioSender {
    writer: FramedWrite<ChildStdin, LinesCodec>,
    contexts: PendingContexts,
}
impl TransportSenderT for StdioSender {
    type Error = std::io::Error;
    async fn send(&mut self, message: String) -> std::io::Result<()> {
        if message.len() > MAX_RPC_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "runtime request exceeds 48 MiB",
            ));
        }
        let message = self.contexts.attach(message);
        self.writer
            .send(message)
            .await
            .map_err(std::io::Error::other)
    }
    async fn close(&mut self) -> std::io::Result<()> {
        SinkExt::<String>::close(&mut self.writer)
            .await
            .map_err(std::io::Error::other)
    }
}

struct StdioReceiver {
    reader: FramedRead<ChildStdout, LinesCodec>,
    telemetry: crate::adapters::telemetry::TelemetryHandle,
}
impl TransportReceiverT for StdioReceiver {
    type Error = std::io::Error;
    async fn receive(&mut self) -> std::io::Result<ReceivedMessage> {
        match self.reader.next().await {
            Some(Ok(message)) => {
                if !message.contains(wukong_worker_protocol::TELEMETRY_FIELD) {
                    return Ok(ReceivedMessage::Text(message));
                }
                match wukong_worker_protocol::extract_response_telemetry(&message) {
                    Ok(extracted) => {
                        if let Some(batch) = extracted.batch {
                            self.telemetry.enqueue_worker_batch(batch);
                        }
                        if extracted.dropped {
                            self.telemetry.dropped("traces", "invalid_worker_batch");
                        }
                        Ok(ReceivedMessage::Text(extracted.message))
                    }
                    Err(_) => Ok(ReceivedMessage::Text(message)),
                }
            }
            // A codec error terminates this RPC connection immediately. Never
            // keep discarding an unbounded overlong line from a broken worker.
            Some(Err(error)) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "runtime bridge closed the transport",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> TraceContext {
        TraceContext {
            traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into(),
            tracestate: None,
        }
    }

    #[test]
    fn pending_context_is_consumed_by_exact_request_and_bounded() {
        let contexts =
            PendingContexts::new(crate::adapters::telemetry::TelemetryHandle::disabled());
        for id in 0..MAX_PENDING_CONTEXTS {
            contexts.insert(id.to_string(), context());
        }
        let request =
            serde_json::json!({"jsonrpc":"2.0","id":0,"method":"doctor","params":{}}).to_string();
        let attached = contexts.attach(request.clone());
        assert!(attached.contains(wukong_worker_protocol::TELEMETRY_FIELD));
        assert_eq!(contexts.attach(request.clone()), request);
        contexts.insert("4".into(), context());
        contexts.insert("5".into(), context());
        let overflow =
            serde_json::json!({"jsonrpc":"2.0","id":5,"method":"doctor","params":{}}).to_string();
        assert_eq!(contexts.attach(overflow.clone()), overflow);
    }
}
