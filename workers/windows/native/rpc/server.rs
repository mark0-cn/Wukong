//! JSON-RPC library adapter. The service authenticates and bounds each frame.
use crate::{rpc::worker::Worker, telemetry::Telemetry};
use anyhow::{Context, Result, ensure};
use jsonrpsee_core::server::RpcModule;
use jsonrpsee_types::{ErrorObjectOwned, Response};
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::{Instrument as _, instrument::WithSubscriber as _};
use wukong_worker_protocol::{
    MAX_RPC_BYTES, TelemetryBatch, WorkerErrorData, WorkerErrorKind, attach_response_telemetry,
    extract_trace_context,
};

pub(crate) struct Server {
    module: RpcModule<Worker>,
    telemetry: Telemetry,
}
impl Server {
    pub(crate) fn with_telemetry(worker: Arc<Worker>, telemetry: Telemetry) -> Result<Self> {
        let mut module = RpcModule::from_arc(worker);
        for method in crate::rpc::catalog::GROUPS
            .iter()
            .flat_map(|group| group.methods.iter().map(|entry| entry.method))
        {
            module.register_async_method(method.name(), move |params, worker, _| {
                let name = method.name();
                let span = tracing::info_span!(
                    target: "wukong_windows_worker::rpc",
                    "rpc.method",
                    otel.name = name,
                    otel.kind = "internal",
                    rpc.method = name
                );
                async move {
                    let params: Value = if params.as_str().is_none() {
                        json!({})
                    } else {
                        params.parse()?
                    };
                    wukong_worker_protocol::decode(method, params.clone()).map_err(|error| {
                        ErrorObjectOwned::owned(
                            -32602,
                            error.to_string(),
                            Some(WorkerErrorData {
                                kind: WorkerErrorKind::InvalidRequest,
                            }),
                        )
                    })?;
                    let result = worker
                        .dispatch(method, params)
                        .await
                        .map_err(worker_error)?;
                    wukong_worker_protocol::validate_result(method, &result)
                        .map_err(worker_error)?;
                    Ok::<_, ErrorObjectOwned>(result)
                }
                .instrument(span)
            })?;
        }
        Ok(Self { module, telemetry })
    }
    pub(crate) async fn call(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        ensure!(bytes.len() <= MAX_RPC_BYTES, "request exceeds size limit");
        let request = std::str::from_utf8(bytes).context("RPC frame is not UTF-8")?;
        let extracted = extract_trace_context(request).ok();
        let request = extracted
            .as_ref()
            .map_or(request, |item| item.message.as_str());
        let parent = extracted.as_ref().and_then(|item| item.context.as_ref());
        let span = self.telemetry.server_span(parent);
        let response = match self
            .module
            .raw_json_request(request, 1)
            .instrument(span)
            .with_subscriber(self.telemetry.dispatcher())
            .await
        {
            Ok((response, _)) => response.get().to_owned(),
            Err(_) => serde_json::to_string(&Response::<Value>::new(
                jsonrpsee_types::ResponsePayload::error(ErrorObjectOwned::from(
                    jsonrpsee_types::error::ErrorCode::ParseError,
                )),
                jsonrpsee_types::Id::Null,
            ))?,
        };
        let response = with_response_batch(response, self.telemetry.response_batch());
        ensure!(
            response.len() < MAX_RPC_BYTES,
            "response exceeds size limit; reconcile original request"
        );
        Ok((response + "\n").into_bytes())
    }
}

fn with_response_batch(response: String, batch: TelemetryBatch) -> String {
    match attach_response_telemetry(&response, &batch) {
        Ok(transformed) if transformed.message.len() < MAX_RPC_BYTES => transformed.message,
        _ => response,
    }
}

fn worker_error(error: anyhow::Error) -> ErrorObjectOwned {
    let kind = error
        .downcast_ref::<crate::rpc::worker_leases::Rejected>()
        .map(|e| e.kind)
        .unwrap_or(if error.is::<wukong_worker_protocol::CapacityBusy>() {
            WorkerErrorKind::ResourceBusy
        } else if error.is::<serde_json::Error>() {
            WorkerErrorKind::InvalidRequest
        } else {
            WorkerErrorKind::Internal
        });
    ErrorObjectOwned::owned(
        -32000,
        format!("{error:#}").chars().take(2048).collect::<String>(),
        Some(WorkerErrorData { kind }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_size_limits_never_replace_the_business_response() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#.to_owned();
        let empty = with_response_batch(response.clone(), TelemetryBatch::default());
        assert_eq!(empty, response);
        let small = TelemetryBatch {
            traces: Some("AA==".into()),
            logs: None,
        };
        let attached = with_response_batch(response.clone(), small.clone());
        assert_ne!(attached, response);
        let extracted = wukong_worker_protocol::extract_response_telemetry(&attached).unwrap();
        assert_eq!(extracted.batch, Some(small));
        assert!(extracted.message.contains(r#""ok":true"#));

        let oversize = TelemetryBatch {
            traces: Some("A".repeat(wukong_worker_protocol::MAX_TELEMETRY_BYTES)),
            logs: None,
        };
        assert!(!oversize.is_bounded());
        assert_eq!(with_response_batch(response.clone(), oversize), response);
    }
}
