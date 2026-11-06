//! MCP stdio clients. Server credentials are explicitly allowlisted env names.
use anyhow::{Context, Result, bail, ensure};
use rig::completion::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
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
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    sequence: u64,
    timeout: u64,
}
impl Connection {
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.sequence += 1;
        let id = self.sequence;
        self.input
            .write_all(
                serde_json::to_string(
                    &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
                )?
                .as_bytes(),
            )
            .await?;
        self.input.write_all(b"\n").await?;
        self.input.flush().await?;
        tokio::time::timeout(Duration::from_secs(self.timeout.clamp(1,600)),async{
            loop{
                let mut line=String::new();ensure!(self.output.read_line(&mut line).await?>0,"MCP server closed stdout");ensure!(line.len()<=2*1024*1024,"MCP message exceeds 2 MiB");
                let message:Value=serde_json::from_str(&line).context("MCP stdout must contain JSON-RPC lines only")?;
                if message.get("id")==Some(&json!(id))&&message.get("method").is_none(){if let Some(error)=message.get("error"){bail!("MCP {method} failed: {error}");}return message.get("result").cloned().context("MCP result missing");}
                if let Some(request_id)=message.get("id").filter(|_|message.get("method").is_some()) {
                    let result=if message["method"]=="ping"{json!({"jsonrpc":"2.0","id":request_id,"result":{}})}else{json!({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"client method unavailable"}})};
                    self.input.write_all(format!("{result}\n").as_bytes()).await?;self.input.flush().await?;
                }
            }
        }).await.context("MCP request timed out")?
    }
}
#[derive(Clone, Default)]
pub struct McpRegistry {
    connections: BTreeMap<String, Arc<Mutex<Connection>>>,
    tools: BTreeMap<String, (String, String, ToolDefinition, bool)>,
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
                .stderr(Stdio::inherit())
                .kill_on_drop(true);
            #[cfg(windows)]
            command.creation_flags(0x08000000);
            let mut child = command
                .spawn()
                .with_context(|| format!("start MCP server {}", config.name))?;
            let mut connection = Connection {
                input: child.stdin.take().context("MCP stdin missing")?,
                output: BufReader::new(child.stdout.take().context("MCP stdout missing")?),
                child,
                sequence: 0,
                timeout: config.timeout_seconds,
            };
            connection.request("initialize",json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"re-agent","version":env!("CARGO_PKG_VERSION")}})).await?;
            connection
                .input
                .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
                .await?;
            connection.input.flush().await?;
            let mut cursor: Option<String> = None;
            loop {
                let response = connection
                    .request(
                        "tools/list",
                        cursor
                            .as_ref()
                            .map(|c| json!({"cursor":c}))
                            .unwrap_or(json!({})),
                    )
                    .await?;
                for tool in response["tools"]
                    .as_array()
                    .context("MCP tools/list has no tools array")?
                {
                    let original = tool["name"]
                        .as_str()
                        .context("MCP tool name missing")?
                        .to_owned();
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
                            tool["description"].as_str().unwrap_or(&original)
                        ),
                        tool["inputSchema"].clone(),
                    );
                    let read_only = tool["annotations"]["readOnlyHint"]
                        .as_bool()
                        .unwrap_or(false);
                    registry
                        .tools
                        .insert(name, (config.name.clone(), original, definition, read_only));
                }
                cursor = response["nextCursor"].as_str().map(str::to_owned);
                if cursor.is_none() {
                    break;
                }
            }
            registry
                .connections
                .insert(config.name.clone(), Arc::new(Mutex::new(connection)));
        }
        Ok(registry)
    }
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|(_, _, d, _)| d.clone()).collect()
    }
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }
    pub fn side_effecting(&self, name: &str) -> bool {
        self.tools
            .get(name)
            .is_none_or(|(_, _, _, readonly)| !*readonly)
    }
    pub async fn call(&self, name: &str, arguments: Value) -> Result<Value> {
        let (server, tool, _, _) = self.tools.get(name).context("unknown MCP tool")?;
        self.connections
            .get(server)
            .context("MCP connection missing")?
            .lock()
            .await
            .request("tools/call", json!({"name":tool,"arguments":arguments}))
            .await
    }
    pub async fn close(&self) {
        for connection in self.connections.values() {
            let _ = connection.lock().await.child.kill().await;
        }
    }
}
