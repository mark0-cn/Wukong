//! System-owned worker IPC. Health establishes the transport boundary only;
//! target execution remains disabled until native resource operations exist.
use crate::{lease_proxy::Identity, pipe_transport, processes::JobObserver, security};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{os::windows::io::AsRawHandle, time::Duration};
use tokio::{io::BufReader, net::windows::named_pipe::NamedPipeServer};
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;

const FRAME_LIMIT: usize = 4096;
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema: u32,
    operation: Operation,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Health,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Health {
    schema: u32,
    identity: Identity,
    production_execution: bool,
    scope: String,
}

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
            let mut stream = BufReader::new(server);
            let frame = pipe_transport::read_bounded_frame(&mut stream, FRAME_LIMIT)
                .await?
                .context("native broker closed before a request")?;
            self.authenticate(stream.get_ref())?;
            let request: Request = serde_json::from_slice(&frame)?;
            ensure!(request.schema == 1, "unsupported native broker schema");
            let reply = match request.operation {
                Operation::Health => Health {
                    schema: 1,
                    identity: self.identity.clone(),
                    production_execution: false,
                    scope: "native_broker_channel_only".into(),
                },
            };
            let mut bytes = serde_json::to_vec(&reply)?;
            bytes.push(b'\n');
            ensure!(
                bytes.len() <= FRAME_LIMIT,
                "native broker response exceeds limit"
            );
            pipe_transport::write_frame(stream.get_mut(), &bytes).await
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
            BufReader::new(pipe_transport::connect_broker(installation, parent_pid).await?);
        pipe_transport::write_frame(
            client.get_mut(),
            b"{\"schema\":1,\"operation\":\"health\"}\n",
        )
        .await?;
        let frame = pipe_transport::read_bounded_frame(&mut client, FRAME_LIMIT)
            .await?
            .context("native broker closed without a response")?;
        let reply: Health = serde_json::from_slice(&frame)?;
        ensure!(
            reply.schema == 1
                && reply.identity.installation_id == installation
                && !reply.production_execution
                && reply.scope == "native_broker_channel_only",
            "native broker health identity or scope changed"
        );
        Ok(serde_json::to_value(reply)?)
    })
    .await
    .context("native broker health deadline exceeded")?
}

#[cfg(test)]
#[path = "broker_tests.rs"]
mod tests;
