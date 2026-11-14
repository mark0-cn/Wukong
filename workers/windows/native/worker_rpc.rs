//! JSON-RPC library adapter. The service authenticates and bounds each frame.
use crate::worker::Worker;
use anyhow::{Context, Result, ensure};
use jsonrpsee_core::server::RpcModule;
use jsonrpsee_types::{ErrorObjectOwned, Response};
use serde_json::{Value, json};
use std::sync::Arc;
use wukong_worker_protocol::{MAX_RPC_BYTES, Method, WorkerErrorData, WorkerErrorKind};

pub(crate) struct Server(RpcModule<Worker>);
impl Server {
    pub(crate) fn new(worker: Arc<Worker>) -> Result<Self> {
        let mut module = RpcModule::from_arc(worker);
        for method in Method::ALL.iter().copied().filter(|method| {
            matches!(
                method,
                Method::Ping | Method::Doctor | Method::LeaseAcquire | Method::LeaseRelease
            ) || crate::worker_catalog::route(method.name()).is_some()
        }) {
            module.register_async_method(method.name(), move |params, worker, _| async move {
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
                wukong_worker_protocol::validate_result(method, &result).map_err(worker_error)?;
                Ok::<_, ErrorObjectOwned>(result)
            })?;
        }
        Ok(Self(module))
    }
    pub(crate) async fn call(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        ensure!(bytes.len() <= MAX_RPC_BYTES, "request exceeds size limit");
        let request = std::str::from_utf8(bytes).context("RPC frame is not UTF-8")?;
        let response = match self.0.raw_json_request(request, 1).await {
            Ok((response, _)) => response.get().to_owned(),
            Err(_) => serde_json::to_string(&Response::<Value>::new(
                jsonrpsee_types::ResponsePayload::error(ErrorObjectOwned::from(
                    jsonrpsee_types::error::ErrorCode::ParseError,
                )),
                jsonrpsee_types::Id::Null,
            ))?,
        };
        ensure!(
            response.len() < MAX_RPC_BYTES,
            "response exceeds size limit; reconcile original request"
        );
        Ok((response + "\n").into_bytes())
    }
}

fn worker_error(error: anyhow::Error) -> ErrorObjectOwned {
    let kind = error
        .downcast_ref::<crate::worker_leases::Rejected>()
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
