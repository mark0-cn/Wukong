//! System-owned worker IPC. Health establishes the transport boundary only;
//! target execution remains disabled until native resource operations exist.
use crate::broker_wire::{self, Health, HealthRequest, Request, SCHEMA, request::Operation};
use crate::{lease_proxy::Identity, pipe_transport, processes::JobObserver, security};
use anyhow::{Context, Result, ensure};
use futures_util::{SinkExt, StreamExt};
use prost::{Message, bytes::Bytes};
use std::{os::windows::io::AsRawHandle, time::Duration};
use tokio::net::windows::named_pipe::NamedPipeServer;
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;

pub(crate) struct Broker {
    identity: Identity,
    worker_sid: String,
    worker_job: JobObserver,
}
impl Broker {
    pub(crate) fn new(
        identity: Identity,
        worker_sid: String,
        worker_job: JobObserver,
    ) -> Result<Self> {
        security::validate_account_sid(&worker_sid)?;
        Ok(Self {
            identity,
            worker_sid,
            worker_job,
        })
    }
    fn authenticate(&self, server: &NamedPipeServer) -> Result<()> {
        // Both values come from the kernel, never request fields. SID checking
        // reverts impersonation before returning or performing any await.
        ensure!(
            pipe_transport::client_sid(server)? == self.worker_sid,
            "native broker caller is not the metadata worker"
        );
        let mut pid = 0;
        security::check(unsafe { GetNamedPipeClientProcessId(server.as_raw_handle(), &mut pid) })?;
        ensure!(
            pid > 0 && self.worker_job.contains(pid)?,
            "native broker caller is outside the current worker Job"
        );
        Ok(())
    }
    pub(crate) async fn serve(&self, server: NamedPipeServer) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut stream = broker_wire::framed(server);
            let frame = stream
                .next()
                .await
                .transpose()?
                .context("native broker closed before a request")?;
            self.authenticate(stream.get_ref())?;
            let request = Request::decode(frame)?;
            ensure!(request.schema == SCHEMA, "unsupported native broker schema");
            let reply = match request
                .operation
                .context("missing native broker operation")?
            {
                Operation::Health(_) => Health {
                    schema: SCHEMA,
                    identity: Some(self.identity.clone()),
                    production_execution: false,
                    scope: "native_broker_channel_only".into(),
                },
            };
            stream.send(Bytes::from(reply.encode_to_vec())).await?;
            Ok(())
        })
        .await
        .context("native broker frame deadline exceeded")?
    }
}

/// Fixed native client used by the metadata worker. It never accepts a command,
/// executable path, PID to attach, credential, or arbitrary broker operation.
pub async fn health(installation: &str, parent_pid: u32) -> Result<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut client =
            broker_wire::framed(pipe_transport::connect_broker(installation, parent_pid).await?);
        client
            .send(Bytes::from(
                Request {
                    schema: SCHEMA,
                    operation: Some(Operation::Health(HealthRequest {})),
                }
                .encode_to_vec(),
            ))
            .await?;
        let frame = client
            .next()
            .await
            .transpose()?
            .context("native broker closed without a response")?;
        let reply = Health::decode(frame)?;
        let identity = reply
            .identity
            .as_ref()
            .context("missing native broker identity")?;
        ensure!(
            reply.schema == SCHEMA
                && identity.installation_id == installation
                && !reply.production_execution
                && reply.scope == "native_broker_channel_only",
            "native broker health identity or scope changed"
        );
        let mut value = serde_json::to_value(reply)?;
        // The CLI projection is independent of the internal wire version.
        value["schema"] = 1.into();
        Ok(value)
    })
    .await
    .context("native broker health deadline exceeded")?
}

#[cfg(test)]
#[path = "broker_tests.rs"]
mod tests;
