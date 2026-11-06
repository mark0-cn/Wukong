//! Credential-free transport to a persistent execution worker.
//!
//! The stdio bridge is only a transport. Its daemon and job runners survive this
//! client being dropped; reconnect uses the same worker state directory.
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

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
#[serde(default)]
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
            state_dir: PathBuf::from(".re-agent/worker"),
            python: "python".into(),
            bridge_path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("workers/runtime/bridge.py"),
            docker_container: None,
            docker_executable: default_docker_executable(),
            ssh_host: None,
            remote_bridge_path: "/opt/re-agent/runtime/bridge.py".into(),
            remote_state_dir: "/state".into(),
            request_timeout_secs: 120,
        }
    }
}

fn default_docker_executable() -> String {
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
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
    poisoned: bool,
}
#[derive(Clone)]
pub struct RuntimeClient {
    transport: Arc<Mutex<Transport>>,
    config: RuntimeConfig,
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
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd.spawn().context("start runtime bridge transport")?;
        let input = child.stdin.take().context("worker stdin unavailable")?;
        let output = BufReader::new(child.stdout.take().context("worker stdout unavailable")?);
        let client = Self {
            transport: Arc::new(Mutex::new(Transport {
                child,
                input,
                output,
                next_id: 1,
                poisoned: false,
            })),
            config,
        };
        client
            .call("ping", json!({}))
            .await
            .context("runtime handshake")?;
        Ok(client)
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }
    pub async fn doctor(&self) -> Result<Value> {
        self.call("doctor", json!({})).await
    }

    /// Calls are serialized over stdio. Worker jobs and debug sessions have their
    /// own ownership locks and may continue while this connection is absent.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let mut transport = self.transport.lock().await;
        if transport.poisoned {
            bail!("runtime transport lost; reconnect before another call");
        }
        let id = transport.next_id;
        transport.next_id += 1;
        let payload =
            serde_json::to_vec(&json!({"jsonrpc":"2.0", "id":id,"method":method,"params":params}))?;
        if payload.len() > 48 * 1024 * 1024 {
            bail!("runtime request exceeds 48 MiB");
        }
        let timeout = Duration::from_secs(self.config.request_timeout_secs.clamp(1, 300));
        let response = tokio::time::timeout(timeout, async {
            transport.input.write_all(&payload).await?;
            transport.input.write_all(b"\n").await?;
            transport.input.flush().await?;
            let mut data = Vec::new();
            // The controller enforces its own limit even for a broken worker.
            let n = (&mut transport.output)
                .take(48 * 1024 * 1024 + 1)
                .read_until(b'\n', &mut data)
                .await?;
            if n == 0 {
                return Err(anyhow!("runtime bridge closed the transport"));
            }
            if data.len() > 48 * 1024 * 1024 {
                bail!("runtime response too large");
            }
            let response: Value =
                serde_json::from_slice(&data).context("decode runtime response")?;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                bail!("runtime response ID mismatch");
            }
            Ok(response)
        })
        .await;
        match response {
            Ok(Ok(response)) => {
                if let Some(error) = response.get("error") {
                    bail!(
                        "runtime {method}: {}",
                        error.get("message").unwrap_or(error)
                    );
                }
                response
                    .get("result")
                    .cloned()
                    .context("runtime response has no result")
            }
            Ok(Err(error)) => {
                transport.poisoned = true;
                let _ = transport.child.kill().await;
                Err(error)
            }
            Err(_) => {
                transport.poisoned = true;
                let _ = transport.child.kill().await;
                bail!(
                    "runtime request timed out; reconnect and reconcile the existing job before retrying"
                )
            }
        }
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
