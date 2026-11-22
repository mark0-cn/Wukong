//! Backend-specific relay launch and inherited environment policy.
use super::TransportConfig;
use anyhow::{Result, bail};
use tokio::process::Command;

fn remote_quote(value: &str) -> Result<String> {
    Ok(shlex::try_quote(value)?.into_owned())
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
                remote_quote(&config.remote_bridge_path)?,
                remote_quote(&config.remote_state_dir)?
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

#[cfg(test)]
mod tests {
    use super::{command, remote_quote};
    use crate::adapters::worker::{ManagedSshConfig, SshConfig, TransportConfig, ssh::ManagedSsh};
    use anyhow::Result;
    use base64::Engine;
    use std::{fs, path::Path};
    use wukong_worker_package::SshPublicKey;

    const VALUES: &[&str] = &[
        "",
        "plain",
        "/opt/wukong/path with spaces/bridge.py",
        "single'quote",
        "double\"quote",
        "$HOME `printf injected` $(printf injected) ; \\ * ? [abc]",
        "tab\tline\nnext",
        "路径/状態",
    ];

    #[test]
    fn remote_quoting_round_trips_each_complete_shell_word() {
        for value in VALUES {
            let quoted = remote_quote(value).expect("representative path must quote");
            assert_eq!(shlex::split(&quoted), Some(vec![value.to_string()]));
        }
        assert!(remote_quote("bridge\0.py").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn remote_quoting_preserves_literal_shell_values() {
        for value in VALUES {
            let script = format!("printf '%s' {}", remote_quote(value).unwrap());
            let output = std::process::Command::new("/bin/sh")
                .args(["-c", &script])
                .output()
                .expect("trusted shell fixture must run");
            assert!(output.status.success());
            assert_eq!(output.stdout, value.as_bytes());
        }
    }

    #[test]
    fn non_managed_ssh_builds_one_literal_remote_command() -> Result<()> {
        let mut config = SshConfig::new("worker.example".into());
        config.remote_bridge_path = "/opt/wukong/a path/'quoted'/bridge.py".into();
        config.remote_state_dir = "/state/$literal/`literal`".into();
        let process = command(&TransportConfig::Ssh(config.clone()))?;
        let args = process
            .as_std()
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            shlex::split(args.last().expect("remote command")),
            Some(vec![
                "python3".to_string(),
                config.remote_bridge_path.clone(),
                "--state".to_string(),
                config.remote_state_dir.clone(),
                "--stdio".to_string(),
            ])
        );
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
            "PYTHONUNBUFFERED",
        ];
        assert!(process.as_std().get_envs().all(|(key, _)| {
            allowed
                .iter()
                .any(|candidate| key.to_string_lossy().eq_ignore_ascii_case(candidate))
        }));
        assert_eq!(
            process
                .as_std()
                .get_envs()
                .find(|(key, _)| *key == "PYTHONUNBUFFERED")
                .and_then(|(_, value)| value),
            Some(std::ffi::OsStr::new("1"))
        );
        Ok(())
    }

    fn managed_fixture(root: &Path) -> Result<ManagedSsh> {
        fs::create_dir_all(root)?;
        let root = root.canonicalize()?;
        let mut blob = vec![0, 0, 0, 11];
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&[0, 0, 0, 32]);
        blob.extend_from_slice(&[7; 32]);
        let config = ManagedSsh {
            installation_id: "a".repeat(32),
            user: format!("wkgm{}", "a".repeat(16)),
            port: 2222,
            identity_file: root.join("private-key"),
            known_hosts_file: root.join("host-pin"),
            host_key: SshPublicKey {
                algorithm: "ssh-ed25519".into(),
                key: base64::engine::general_purpose::STANDARD.encode(blob),
            },
        };
        fs::write(&config.identity_file, b"inert fixture, never authenticated")?;
        fs::write(&config.known_hosts_file, config.known_hosts_record()?)?;
        Ok(config)
    }

    #[test]
    fn managed_ssh_argv_remains_delegated_without_shell_quoting() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let managed = managed_fixture(&temporary.path().join("enrolled paths"))?;
        let expected = managed.command("ssh-fixture", "192.0.2.1")?;
        let expected_args = expected
            .as_std()
            .get_args()
            .map(|value| value.to_os_string())
            .collect::<Vec<_>>();
        let actual = command(&TransportConfig::ManagedSsh(ManagedSshConfig {
            ssh_host: "192.0.2.1".into(),
            ssh_executable: "ssh-fixture".into(),
            managed_ssh: managed,
        }))?;
        assert_eq!(
            actual
                .as_std()
                .get_args()
                .map(|value| value.to_os_string())
                .collect::<Vec<_>>(),
            expected_args
        );
        assert_eq!(
            actual
                .as_std()
                .get_args()
                .last()
                .and_then(|value| value.to_str()),
            Some("wukong-native-relay-v2")
        );
        Ok(())
    }
}
