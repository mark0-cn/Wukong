//! MCP stdio clients. Server credentials are explicitly allowlisted env names.
use anyhow::{Context, Result, ensure};
use futures::StreamExt;
use rig::{
    completion::ToolDefinition,
    tool::rmcp::{
        McpConnection,
        rmcp::{self, ServiceExt},
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
}
fn timeout() -> u64 {
    60
}
struct Connection {
    sdk: McpConnection,
    child: Mutex<Child>,
    timeout: Duration,
    _stderr: crate::process_io::StderrCapture,
}
#[derive(Clone, Default)]
pub struct McpRegistry {
    connections: BTreeMap<String, Arc<Connection>>,
    tools: BTreeMap<String, (String, String, ToolDefinition)>,
}
impl McpRegistry {
    pub async fn connect(servers: &[McpServerConfig]) -> Result<Self> {
        let mut registry = Self::default();
        for config in servers {
            ensure!(
                !config.name.is_empty()
                    && config
                        .name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "MCP server name must be alphanumeric/underscore"
            );
            ensure!(
                !registry.connections.contains_key(&config.name),
                "duplicate MCP server name"
            );
            let mut command = Command::new(&config.command);
            command.args(&config.args).env_clear();
            for key in [
                "PATH",
                "SystemRoot",
                "WINDIR",
                "TEMP",
                "TMP",
                "PATHEXT",
                "HOME",
                "USERPROFILE",
            ]
            .into_iter()
            .chain(config.env.iter().map(String::as_str))
            {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(windows)]
            command.creation_flags(0x08000000);
            let mut child = command
                .spawn()
                .with_context(|| format!("start MCP server {}", config.name))?;
            let stderr = crate::process_io::StderrCapture::new(
                child.stderr.take().context("MCP stderr missing")?,
            );
            // Use rmcp's codec on bounded frames. An oversized/malformed line
            // closes this transport immediately instead of discarding an
            // endless stream until the request timeout.
            use rmcp::{
                service::{RoleClient, RxJsonRpcMessage, TxJsonRpcMessage},
                transport::async_rw::JsonRpcMessageCodec,
            };
            let transport_error = Arc::new(std::sync::Mutex::new(None::<String>));
            let read_error = transport_error.clone();
            let read = tokio_util::codec::FramedRead::new(
                child.stdout.take().context("MCP stdout missing")?,
                JsonRpcMessageCodec::<RxJsonRpcMessage<RoleClient>>::new_with_max_length(
                    2 * 1024 * 1024,
                ),
            )
            .scan((), move |_, item| {
                let message = match item {
                    Ok(message) => Some(message),
                    Err(error) => {
                        *read_error
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Some(format!("MCP stdio frame invalid or exceeds 2 MiB: {error}"));
                        None
                    }
                };
                std::future::ready(message)
            });
            let write = tokio_util::codec::FramedWrite::new(
                child.stdin.take().context("MCP stdin missing")?,
                JsonRpcMessageCodec::<TxJsonRpcMessage<RoleClient>>::new(),
            );
            let transport = (write, read);
            let timeout = Duration::from_secs(config.timeout_seconds.clamp(1, 600));
            let client = rmcp::model::ClientConfig::new(
                rmcp::model::ClientCapabilities::default(),
                rmcp::model::Implementation::new("re-agent", env!("CARGO_PKG_VERSION")),
            );
            let service = tokio::time::timeout(timeout, client.serve(transport))
                .await
                .context("MCP initialization timed out")?
                .with_context(|| {
                    transport_error
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone()
                        .unwrap_or_else(|| stderr.context("MCP initialization failed"))
                })?;
            let sdk = McpConnection::new(service);
            let tools = tokio::time::timeout(timeout, sdk.tools())
                .await
                .context("MCP tools/list timed out")??;
            for tool in tools {
                let tool = tool.definition();
                let original = tool.name.to_string();
                let normalized: String = original
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '_' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                let name = format!("mcp_{}_{}", config.name, normalized);
                ensure!(
                    !registry.tools.contains_key(&name),
                    "MCP tool names collide after normalization: {name}"
                );
                let definition = ToolDefinition::new(
                    rig::message::ToolName::new(name.clone())?,
                    format!(
                        "MCP {}: {}",
                        config.name,
                        tool.description.as_deref().unwrap_or(&original)
                    ),
                    Value::Object((*tool.input_schema).clone()),
                );
                let read_only = tool
                    .annotations
                    .as_ref()
                    .and_then(|annotations| annotations.read_only_hint)
                    .unwrap_or(false);
                let definition = definition.with_annotations(
                    rig::completion::ToolAnnotations::default().with_read_only(read_only),
                );
                registry
                    .tools
                    .insert(name, (config.name.clone(), original, definition));
            }
            registry.connections.insert(
                config.name.clone(),
                Arc::new(Connection {
                    sdk,
                    child: Mutex::new(child),
                    timeout,
                    _stderr: stderr,
                }),
            );
        }
        Ok(registry)
    }
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|(_, _, d)| d.clone()).collect()
    }
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }
    pub fn side_effecting(&self, name: &str) -> bool {
        self.tools.get(name).is_none_or(|(_, _, definition)| {
            !rig_core::tool::Touches::from_hints(&definition.annotations).reads_only()
        })
    }
    pub async fn call(&self, name: &str, arguments: Value) -> Result<Value> {
        let (server, tool, _) = self.tools.get(name).context("unknown MCP tool")?;
        let connection = self
            .connections
            .get(server)
            .context("MCP connection missing")?;
        let mut request = rmcp::model::CallToolRequestParams::new(tool.clone());
        request.arguments = Some(
            arguments
                .as_object()
                .context("MCP arguments must be an object")?
                .clone(),
        );
        // A timed-out remote mutation is uncertain. The application ledger
        // records that outcome and never blindly repeats it.
        let result =
            tokio::time::timeout(connection.timeout, connection.sdk.peer().call_tool(request))
                .await
                .context("MCP request timed out")??;
        Ok(serde_json::to_value(result)?)
    }

    pub async fn close(&self) {
        for connection in self.connections.values() {
            connection.sdk.close().await;
            let _ = connection.child.lock().await.kill().await;
        }
    }
}
