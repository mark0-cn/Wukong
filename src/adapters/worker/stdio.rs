//! Own the relay child, framed streams, and bounded stderr diagnostics.
use crate::adapters::process_io::StderrCapture;
use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use jsonrpsee_core::client::{
    Client, ClientBuilder, ReceivedMessage, TransportReceiverT, TransportSenderT,
};
use std::{process::Stdio, time::Duration};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
use wukong_worker_protocol::MAX_RPC_BYTES;

pub(super) struct StdioChannel {
    child: Child,
    stderr: StderrCapture,
    pub(super) client: Client,
}

impl StdioChannel {
    pub(super) fn spawn(mut cmd: Command, request_timeout_secs: u64) -> Result<Self> {
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
        let client = ClientBuilder::default()
            .request_timeout(Duration::from_secs(request_timeout_secs.clamp(1, 3600)))
            .max_concurrent_requests(1)
            .build_with_tokio(
                StdioSender(FramedWrite::new(
                    input,
                    LinesCodec::new_with_max_length(MAX_RPC_BYTES),
                )),
                StdioReceiver(FramedRead::new(
                    output,
                    LinesCodec::new_with_max_length(MAX_RPC_BYTES),
                )),
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

struct StdioSender(FramedWrite<ChildStdin, LinesCodec>);
impl TransportSenderT for StdioSender {
    type Error = std::io::Error;
    async fn send(&mut self, message: String) -> std::io::Result<()> {
        if message.len() > MAX_RPC_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "runtime request exceeds 48 MiB",
            ));
        }
        self.0.send(message).await.map_err(std::io::Error::other)
    }
    async fn close(&mut self) -> std::io::Result<()> {
        SinkExt::<String>::close(&mut self.0)
            .await
            .map_err(std::io::Error::other)
    }
}

struct StdioReceiver(FramedRead<ChildStdout, LinesCodec>);
impl TransportReceiverT for StdioReceiver {
    type Error = std::io::Error;
    async fn receive(&mut self) -> std::io::Result<ReceivedMessage> {
        match self.0.next().await {
            Some(Ok(message)) => Ok(ReceivedMessage::Text(message)),
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
