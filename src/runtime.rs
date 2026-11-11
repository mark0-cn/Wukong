//! Credential-free transport to a persistent execution worker.
//!
//! The stdio bridge is only a transport. Its daemon and job runners survive this
//! client being dropped, and a lost transport is replaced transparently on the
//! next call; reconnect uses the same worker state directory.
use anyhow::{Context, Result, anyhow, bail};
use futures::{SinkExt, StreamExt};
use jsonrpsee_core::client::{
    Client, ClientBuilder, ClientT, Error as RpcError, ReceivedMessage, TransportReceiverT,
    TransportSenderT,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
pub mod protocol;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackend {
    #[default]
    Disabled,
    TrustedLocal,
    Docker,
    Ssh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    pub backend: RuntimeBackend,
    pub state_dir: PathBuf,
    pub python: String,
    pub bridge_path: PathBuf,
    pub docker_container: Option<String>,
    pub docker_executable: String,
    pub ssh_host: Option<String>,
    pub remote_bridge_path: String,
    pub remote_state_dir: String,
    pub request_timeout_secs: u64,
}
impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            backend: RuntimeBackend::Disabled,
            state_dir: PathBuf::from(".wukong/worker"),
            python: "python".into(),
            bridge_path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("workers/runtime/bridge.py"),
            docker_container: None,
            docker_executable: default_docker_executable(),
            ssh_host: None,
            remote_bridge_path: "/opt/wukong/runtime/bridge.py".into(),
            remote_state_dir: "/state".into(),
            request_timeout_secs: 300,
        }
    }
}

pub(crate) fn default_docker_executable() -> String {
    #[cfg(windows)]
    {
        let candidate = PathBuf::from(
            std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into()),
        )
        .join("Docker/Docker/resources/bin/docker.exe");
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    "docker".into()
}

struct Transport {
    child: Child,
    client: Client,
    poisoned: bool,
    stderr: crate::process_io::StderrCapture,
    identity: Option<protocol::WorkerIdentity>,
}
#[derive(Clone)]
pub struct RuntimeClient {
    transport: Arc<Mutex<Transport>>,
    config: RuntimeConfig,
    identity: protocol::WorkerIdentity,
    lease: Option<protocol::EnvironmentLease>,
}

fn remote_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

impl RuntimeClient {
    pub async fn connect(config: RuntimeConfig) -> Result<Self> {
        if config.backend == RuntimeBackend::Disabled {
            bail!(
                "Execution worker is disabled. Configure Docker/SSH, or explicitly select trusted_local for trusted fixtures."
            );
        }
        let transport = Self::spawn_transport(&config)
            .await
            .context("start runtime bridge transport")?;
        let identity = transport
            .identity
            .clone()
            .context("missing worker identity")?;
        Ok(Self {
            transport: Arc::new(Mutex::new(transport)),
            config,
            identity,
            lease: None,
        })
    }

    /// Build, spawn and handshake one stdio transport. The bridge daemon,
    /// job supervisors and debugger sessions live in the shared state
    /// directory, so a fresh transport observes the same jobs and sessions
    /// as the one it replaces.
    async fn spawn_transport(config: &RuntimeConfig) -> Result<Transport> {
        let mut cmd = match config.backend {
            RuntimeBackend::Disabled => unreachable!(),
            RuntimeBackend::TrustedLocal => {
                let mut c = Command::new(&config.python);
                c.arg(&config.bridge_path)
                    .arg("--state")
                    .arg(&config.state_dir)
                    .arg("--stdio")
                    .arg("--trusted-local");
                c
            }
            RuntimeBackend::Docker => {
                let container = config
                    .docker_container
                    .as_deref()
                    .context("docker_container is required")?;
                if container.starts_with('-') {
                    bail!("invalid docker container");
                }
                let mut c = Command::new(&config.docker_executable);
                c.args([
                    "exec",
                    "-i",
                    container,
                    "python3",
                    &config.remote_bridge_path,
                    "--state",
                    &config.remote_state_dir,
                    "--stdio",
                ]);
                c
            }
            RuntimeBackend::Ssh => {
                let host = config.ssh_host.as_deref().context("ssh_host is required")?;
                if host.starts_with('-') || host.contains(['\n', '\r']) {
                    bail!("invalid ssh host");
                }
                let mut c = Command::new("ssh");
                c.args(["-T", "-o", "BatchMode=yes", "--", host]);
                // OpenSSH passes a remote command through a shell; quote each path.
                c.arg(format!(
                    "python3 {} --state {} --stdio",
                    remote_quote(&config.remote_bridge_path),
                    remote_quote(&config.remote_state_dir)
                ));
                c
            }
        };
        // Never forward model/provider secrets into an analysis worker.
        let allowed = [
            "PATH",
            "SystemRoot",
            "WINDIR",
            "TEMP",
            "TMP",
            "PATHEXT",
            "COMSPEC",
            "USERPROFILE",
            "HOME",
            "APPDATA",
            "LOCALAPPDATA",
            "PROGRAMDATA",
            "SSH_AUTH_SOCK",
            "DOCKER_HOST",
            "DOCKER_CONTEXT",
            "DOCKER_CONFIG",
        ];
        let env: Vec<_> = allowed
            .iter()
            .filter_map(|k| std::env::var_os(k).map(|v| (*k, v)))
            .collect();
        cmd.env_clear().envs(env).env("PYTHONUNBUFFERED", "1");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd.spawn().context("spawn runtime bridge")?;
        let stderr = crate::process_io::StderrCapture::new(
            child.stderr.take().context("worker stderr unavailable")?,
        );
        let input = child.stdin.take().context("worker stdin unavailable")?;
        let output = child.stdout.take().context("worker stdout unavailable")?;
        let client = ClientBuilder::default()
            .request_timeout(Duration::from_secs(
                config.request_timeout_secs.clamp(1, 3600),
            ))
            .max_concurrent_requests(1)
            .build_with_tokio(
                StdioSender(FramedWrite::new(
                    input,
                    LinesCodec::new_with_max_length(MAX_RPC),
                )),
                StdioReceiver(FramedRead::new(
                    output,
                    LinesCodec::new_with_max_length(MAX_RPC),
                )),
            );
        let mut transport = Transport {
            child,
            client,
            poisoned: false,
            stderr,
            identity: None,
        };
        match request(&mut transport, "ping", json!({})).await {
            Ok(reply) => {
                let identity: protocol::WorkerIdentity =
                    serde_json::from_value(reply["identity"].clone())
                        .context("worker handshake did not establish its identity")?;
                identity.validate()?;
                transport.identity = Some(identity);
                Ok(transport)
            }
            Err(RequestFailure::Worker(error)) => {
                let _ = transport.child.kill().await;
                transport.stderr.finish().await;
                Err(error).context(transport.stderr.context("runtime handshake"))
            }
            Err(RequestFailure::Transport { error, .. }) => {
                let _ = transport.child.kill().await;
                transport.stderr.finish().await;
                Err(error).context(transport.stderr.context("runtime handshake"))
            }
        }
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }
    pub fn identity(&self) -> &protocol::WorkerIdentity {
        &self.identity
    }
    pub(crate) fn with_lease(&self, lease: protocol::EnvironmentLease) -> Self {
        Self {
            lease: Some(lease),
            ..self.clone()
        }
    }
    pub async fn doctor(&self) -> Result<Value> {
        self.call("doctor", json!({})).await
    }

    /// Calls are serialized over stdio. A lost transport is replaced
    /// transparently on the next call: the bridge daemon and job supervisors
    /// live in the shared state directory and survive the stdio client. The
    /// failed request itself is never re-sent, so a timed-out side effect
    /// still follows the durable reconcile path instead of being replayed.
    pub async fn call(&self, method: &str, mut params: Value) -> Result<Value> {
        if method != "ping" && method != "doctor" {
            params["_binding"] = serde_json::to_value(&self.identity)?;
            if let Some(lease) = &self.lease {
                params["_lease"] = serde_json::to_value(lease)?;
            }
        }
        let mut transport = self.transport.lock().await;
        if transport.poisoned {
            *transport = Self::spawn_transport(&self.config)
                .await
                .context("runtime transport lost and reconnect failed")?;
        }
        if transport.identity.as_ref() != Some(&self.identity) {
            let _ = transport.child.kill().await;
            transport.poisoned = true;
            bail!(
                "worker installation, boot or package identity changed; refusing execution in the previous binding"
            );
        }
        match request(&mut transport, method, params).await {
            Ok(result) => Ok(result),
            Err(RequestFailure::Worker(error)) => Err(error),
            Err(RequestFailure::Transport { timed_out, error }) => {
                transport.poisoned = true;
                let _ = transport.child.kill().await;
                transport.stderr.finish().await;
                if timed_out {
                    return Err(protocol::TransportFailure { timed_out:true, message:"runtime request timed out; the worker may have executed it — reconcile the durable call before retrying (the next call reconnects automatically)".into() }.into());
                }
                Err(protocol::TransportFailure {
                    timed_out: false,
                    message: format!(
                        "{}: {error:#}",
                        transport.stderr.context(
                            "runtime transport failed; the next call reconnects automatically"
                        )
                    ),
                }
                .into())
            }
        }
    }

    pub async fn job_status(&self, job_id: &str) -> Result<protocol::JobStatus> {
        let status: protocol::JobStatus =
            serde_json::from_value(self.call("job.status", json!({"job_id":job_id})).await?)
                .context("invalid worker job status")?;
        anyhow::ensure!(status.job_id == job_id, "worker returned the wrong job");
        Ok(status)
    }

    /// Closes only the transport. To stop work, call job.cancel/debug.close first.
    pub async fn close(&self) -> Result<()> {
        let mut transport = self.transport.lock().await;
        transport.poisoned = true;
        transport
            .child
            .kill()
            .await
            .context("close runtime transport")
    }
}

enum RequestFailure {
    /// The worker answered with an RPC error; the transport stays usable.
    Worker(anyhow::Error),
    /// The stdio channel is unusable and must be replaced.
    Transport {
        timed_out: bool,
        error: anyhow::Error,
    },
}

const MAX_RPC: usize = 48 * 1024 * 1024;

struct StdioSender(FramedWrite<ChildStdin, LinesCodec>);
impl TransportSenderT for StdioSender {
    type Error = std::io::Error;
    async fn send(&mut self, message: String) -> std::io::Result<()> {
        if message.len() > MAX_RPC {
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

async fn request(
    transport: &mut Transport,
    method: &str,
    params: Value,
) -> Result<Value, RequestFailure> {
    let params = params
        .as_object()
        .cloned()
        .ok_or_else(|| RequestFailure::Worker(anyhow!("runtime params must be an object")))?;
    match transport.client.request(method, params).await {
        Ok(result) => Ok(result),
        Err(RpcError::Call(error)) => Err(RequestFailure::Worker(
            protocol::WorkerError::from_rpc(
                format!("runtime {method}: {}", error.message()),
                error.data().map(|data| data.get()),
            )
            .into(),
        )),
        Err(error) => Err(RequestFailure::Transport {
            timed_out: matches!(error, RpcError::RequestTimeout),
            error: error.into(),
        }),
    }
}
