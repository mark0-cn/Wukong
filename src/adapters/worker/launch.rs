//! Backend-specific relay launch and inherited environment policy.
use super::TransportConfig;
use anyhow::{Result, bail};
use tokio::process::Command;

fn remote_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn command(config: &TransportConfig) -> Result<Command> {
    let mut cmd = match config {
        TransportConfig::Disabled => bail!(
            "Execution worker is disabled. Configure Docker/SSH, or trusted_local for trusted fixtures."
        ),
        TransportConfig::TrustedLocal(config) => {
            let mut c = Command::new(&config.python);
            c.arg(&config.bridge_path)
                .arg("--state")
                .arg(&config.state_dir)
                .arg("--stdio")
                .arg("--trusted-local");
            c
        }
        TransportConfig::Docker(config) => {
            let container = &config.docker_container;
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
        TransportConfig::ManagedSsh(config) => config
            .managed_ssh
            .command(&config.ssh_executable, &config.ssh_host)?,
        TransportConfig::Ssh(config) => {
            let host = &config.ssh_host;

            if host.starts_with('-') || host.contains(['\n', '\r']) {
                bail!("invalid ssh host");
            }
            let mut c = Command::new(&config.ssh_executable);
            c.args([
                "-T",
                "-a",
                "-x",
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "ClearAllForwardings=yes",
                "-o",
                "PermitLocalCommand=no",
                "--",
                host,
            ]);
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
    Ok(cmd)
}
