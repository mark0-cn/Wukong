//! MCP client for a Job-owned external debugger. rmcp owns the protocol;
//! the native resource host owns admission, process handles and cleanup.
use anyhow::{Context, Result, ensure};
use futures_util::StreamExt;
use rmcp::{
    RoleClient, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig, Implementation,
    },
    service::{RunningService, RxJsonRpcMessage, TxJsonRpcMessage},
    transport::async_rw::JsonRpcMessageCodec,
};
use serde_json::{Value, json};
use std::{fs::File, io::Write, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{FramedRead, FramedWrite};

const TIMEOUT: Duration = Duration::from_secs(40);
const MAX_FRAME: usize = 2 * 1024 * 1024;
const MAX_TRANSCRIPT: usize = crate::session_artifacts::LIMIT;

pub(crate) struct Client {
    service: RunningService<RoleClient, ClientConfig>,
    transcript: File,
    retained: usize,
}

impl Client {
    /// Attach through the external tool while retaining the target's initial
    /// suspension. DbgEng can wait for loader/break-in events, so release that
    /// suspension after SDK attachment is observable, before awaiting the MCP
    /// result. The held process handle, not a model-provided PID, is authority.
    pub(crate) async fn attach_owned(
        &mut self,
        target: &mut crate::processes::OwnedProcess,
    ) -> Result<Value> {
        let (opened, resumed) = {
            let attaching = self.structured("attach_process", json!({"pid":target.pid()}));
            tokio::pin!(attaching);
            let release = async {
                while !target.release_debug_start()? {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Ok::<_, anyhow::Error>(())
            };
            tokio::select! {
                biased;
                result = &mut attaching => (result?, false),
                result = release => {
                    result?;
                    (attaching.await?, true)
                }
            }
        };
        if !resumed {
            ensure!(
                target.release_debug_start()?,
                "external debugger returned without an OS attachment"
            );
        }
        Ok(opened)
    }

    pub(crate) async fn connect(
        read: impl AsyncRead + Unpin + Send + 'static,
        write: impl AsyncWrite + Unpin + Send + 'static,
        transcript: File,
    ) -> Result<Self> {
        // FramedRead ends the session on malformed/oversized input. It never
        // grows an unbounded line or attempts to recover an uncertain mutation.
        let read = FramedRead::new(
            read,
            JsonRpcMessageCodec::<RxJsonRpcMessage<RoleClient>>::new_with_max_length(MAX_FRAME),
        )
        .scan((), |_, message| std::future::ready(message.ok()));
        let write = FramedWrite::new(
            write,
            JsonRpcMessageCodec::<TxJsonRpcMessage<RoleClient>>::new(),
        );
        let config = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("wukong-worker", env!("CARGO_PKG_VERSION")),
        );
        let service = tokio::time::timeout(TIMEOUT, config.serve((write, read)))
            .await
            .context("external debugger MCP initialization timed out")??;
        Ok(Self {
            service,
            transcript,
            retained: 0,
        })
    }

    pub(crate) async fn call(&mut self, name: &str, arguments: Value) -> Result<CallToolResult> {
        let result = self.raw(name, arguments).await?;
        ensure!(
            result.is_error != Some(true),
            "external debugger rejected {name}; inspect its MCP transcript"
        );
        Ok(result)
    }

    pub(crate) async fn raw(&mut self, name: &str, arguments: Value) -> Result<CallToolResult> {
        let mut request = CallToolRequestParams::new(name.to_owned());
        request.arguments = Some(
            arguments
                .as_object()
                .context("MCP arguments must be an object")?
                .clone(),
        );
        let result = tokio::time::timeout(TIMEOUT, self.service.peer().call_tool(request))
            .await
            .context("external debugger outcome is unknown after MCP timeout")??;
        let bytes =
            serde_json::to_vec(&json!({"tool":name,"arguments":arguments,"result":result}))?;
        ensure!(
            self.retained + bytes.len() < MAX_TRANSCRIPT,
            "external debugger transcript exceeded its resource budget"
        );
        self.transcript.write_all(&bytes)?;
        self.transcript.write_all(b"\n")?;
        self.transcript.sync_all()?;
        self.retained += bytes.len() + 1;
        Ok(result)
    }

    pub(crate) async fn structured(&mut self, name: &str, arguments: Value) -> Result<Value> {
        self.call(name, arguments)
            .await?
            .structured_content
            .with_context(|| format!("external debugger {name} did not return a structured result"))
    }

    pub(crate) async fn close(mut self) -> Result<()> {
        self.transcript.sync_all()?;
        tokio::time::timeout(Duration::from_secs(5), self.service.close())
            .await
            .context("external debugger MCP shutdown timed out")??;
        Ok(())
    }
}
