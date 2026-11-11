//! Controller-owned native Windows SSH enrollment. A dedicated public host pin
//! is checked before connection; client configuration cannot add forwarding,
//! agents, proxy commands, alternate identities or shared sessions.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use tokio::process::Command;
use wukong_worker_package::SshPublicKey;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedSsh {
    pub installation_id: String,
    pub user: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub identity_file: PathBuf,
    pub known_hosts_file: PathBuf,
    pub host_key: SshPublicKey,
}
fn default_port() -> u16 {
    22
}
impl ManagedSsh {
    pub fn alias(&self) -> String {
        format!("wukong-worker-{}", self.installation_id)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.installation_id.len() == 32
                && self
                    .installation_id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid managed SSH installation identity"
        );
        ensure!(
            self.user == format!("wkgm{}", &self.installation_id[..16]),
            "managed SSH must use this installation's ordinary management account"
        );
        ensure!(self.port > 0, "managed SSH requires a positive port");
        self.host_key.authorized_key()?;
        for path in [&self.identity_file, &self.known_hosts_file] {
            validate_path(path)?;
        }
        Ok(())
    }
    pub fn known_hosts_record(&self) -> Result<String> {
        self.validate()?;
        Ok(format!(
            "{} {}\n",
            self.alias(),
            self.host_key.authorized_key()?
        ))
    }
    fn validate_files(&self) -> Result<()> {
        for path in [&self.identity_file, &self.known_hosts_file] {
            let metadata = fs::symlink_metadata(path)
                .context("enrolled SSH identity or host-pin file is missing")?;
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "managed SSH files must be ordinary local files"
            );
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(
                    metadata.file_attributes() & 0x400 == 0,
                    "managed SSH files cannot be reparse points"
                );
            }
        }
        let mut bytes = Vec::new();
        fs::File::open(&self.known_hosts_file)?
            .take(4097)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes == self.known_hosts_record()?.as_bytes(),
            "managed SSH host-pin file differs from its independently enrolled public key"
        );
        Ok(())
    }
    pub fn command(&self, executable: &str, host: &str) -> Result<Command> {
        self.validate()?;
        self.validate_files()?;
        ensure!(
            host.parse::<std::net::IpAddr>().is_ok()
                || (!host.is_empty()
                    && host.len() <= 253
                    && host
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
                    && host.as_bytes()[0].is_ascii_alphanumeric()),
            "managed SSH requires a literal IP or DNS hostname without user/options"
        );
        let mut command = Command::new(executable);
        command.args(["-F", "none", "-T", "-a", "-x"]);
        for option in [
            "BatchMode=yes",
            "PreferredAuthentications=publickey",
            "PubkeyAuthentication=yes",
            "PasswordAuthentication=no",
            "KbdInteractiveAuthentication=no",
            "GSSAPIAuthentication=no",
            "HostbasedAuthentication=no",
            "StrictHostKeyChecking=yes",
            "CheckHostIP=no",
            "HostKeyAlgorithms=ssh-ed25519",
            "IdentitiesOnly=yes",
            "IdentityAgent=none",
            "ForwardAgent=no",
            "ForwardX11=no",
            "ForwardX11Trusted=no",
            "ClearAllForwardings=yes",
            "PermitLocalCommand=no",
            "ProxyCommand=none",
            "ProxyJump=none",
            "ControlMaster=no",
            "ControlPath=none",
            "ControlPersist=no",
            "UpdateHostKeys=no",
            "AddKeysToAgent=no",
            "GlobalKnownHostsFile=none",
            "ConnectionAttempts=1",
            "ConnectTimeout=10",
            "EscapeChar=none",
            "SendEnv=-*",
        ] {
            command.arg("-o").arg(option);
        }
        command
            .arg("-o")
            .arg(format!("HostKeyAlias={}", self.alias()));
        command.arg("-o").arg(format!(
            "UserKnownHostsFile={}",
            quoted_path(&self.known_hosts_file)?
        ));
        command
            .arg("-i")
            .arg(client_path(&self.identity_file)?)
            .arg("-l")
            .arg(&self.user)
            .arg("-p")
            .arg(self.port.to_string());
        command.arg("--").arg(host).arg("wukong-native-relay-v2");
        Ok(command)
    }
}
fn validate_path(path: &Path) -> Result<()> {
    let text = path.to_str().context("managed SSH path must be Unicode")?;
    ensure!(
        path.is_absolute()
            && !text.contains(['\0', '\n', '\r', '$', '%'])
            && !text
                .split(['/', '\\'])
                .any(|part| part == "." || part == "..")
            && !path.components().any(|c| matches!(
                c,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )),
        "managed SSH paths must be absolute without aliases or SSH expansion tokens"
    );
    #[cfg(windows)]
    ensure!(
        matches!(path.components().next(),Some(std::path::Component::Prefix(prefix)) if matches!(prefix.kind(),std::path::Prefix::Disk(_)|std::path::Prefix::VerbatimDisk(_))),
        "managed SSH key paths must use a local drive rather than a network/device namespace"
    );
    Ok(())
}
fn quoted_path(path: &Path) -> Result<String> {
    let text = client_path(path)?;
    Ok(format!(
        "\"{}\"",
        text.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}
fn client_path(path: &Path) -> Result<String> {
    validate_path(path)?;
    let text = path
        .to_str()
        .context("invalid managed SSH path")?
        .to_owned();
    #[cfg(windows)]
    let text = text
        .strip_prefix(r"\\?\")
        .unwrap_or(&text)
        .replace('\\', "/");
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Invoked only by the explicitly selected disposable Windows CI fixture.
    /// This exercises the real controller transport, never target execution.
    #[tokio::test]
    #[ignore = "requires an installed worker on a dedicated GitHub-hosted Windows runner"]
    async fn installed_windows_worker_controller_acceptance() -> Result<()> {
        use crate::runtime::{
            RuntimeClient, RuntimeConfig,
            protocol::{EnvironmentLease, WorkerError, WorkerErrorKind},
        };
        use serde_json::{Value, json};
        ensure!(
            cfg!(windows)
                && std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
                && std::env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted"),
            "installed-worker acceptance requires an isolated hosted Windows runner"
        );
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Profile {
            config: RuntimeConfig,
            output: PathBuf,
            manifest_sha256: String,
            minimum_epoch: u64,
        }
        let path = PathBuf::from(
            std::env::var_os("WUKONG_INSTALLED_WORKER_TEST")
                .context("missing dedicated acceptance profile")?,
        );
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 16 * 1024, "acceptance profile exceeds limit");
        let profile: Profile = serde_json::from_slice(&bytes)?;
        ensure!(
            profile.config.ssh_host.as_deref() == Some("127.0.0.1")
                && profile.config.managed_ssh.is_some(),
            "hosted acceptance must use its local installed managed worker"
        );
        let client = RuntimeClient::connect(profile.config).await?;
        ensure!(
            client.identity().package_sha256 == profile.manifest_sha256,
            "installed package differs from prepared payload"
        );
        let ping = client.call("ping", json!({})).await?;
        let doctor = client.doctor().await?;
        ensure!(
            doctor["capabilities"]["native_target_execution"] == false,
            "unaccepted production execution must remain unavailable"
        );
        let acquire = json!({"run_id":"hosted-installation-check","owner_id":"controller-fixture"});
        let first: EnvironmentLease = serde_json::from_value(
            client
                .call("environment.lease.acquire", acquire.clone())
                .await?,
        )?;
        first.validate(
            "hosted-installation-check",
            "controller-fixture",
            client.identity(),
        )?;
        ensure!(
            first.epoch > profile.minimum_epoch,
            "service restart did not preserve increasing native epochs"
        );
        let conflict = client
            .call(
                "environment.lease.acquire",
                json!({"run_id":"other","owner_id":"other"}),
            )
            .await
            .err()
            .context("another controller unexpectedly acquired the installed worker")?;
        ensure!(
            conflict
                .downcast_ref::<WorkerError>()
                .is_some_and(|e| e.kind == WorkerErrorKind::Conflict),
            "another controller unexpectedly acquired the installed worker"
        );
        let bound = client.with_lease(first.clone());
        let put = json!({"path":"hosted-smoke.bin","data_base64":"AAECAw==","request_id":"hosted-stable-upload"});
        let artifact = bound.call("file.put", put.clone()).await?;
        ensure!(
            bound.call("file.put", put.clone()).await? == artifact,
            "immutable upload receipt changed"
        );
        let read = bound
            .call("file.get", json!({"path":"files/hosted-smoke.bin"}))
            .await?;
        ensure!(
            read["data_base64"] == "AAECAw==" && read["sha256"] == artifact["sha256"],
            "installed transfer identity mismatch"
        );
        client
            .call("environment.lease.release", json!({"lease":first}))
            .await?;
        let second: EnvironmentLease =
            serde_json::from_value(client.call("environment.lease.acquire", acquire).await?)?;
        ensure!(
            second.epoch > first.epoch,
            "released native fence was reused"
        );
        let stale = bound
            .call("file.put", put)
            .await
            .err()
            .context("stale fence reached a cached worker receipt")?;
        ensure!(
            stale
                .downcast_ref::<WorkerError>()
                .is_some_and(|e| e.kind == WorkerErrorKind::LeaseLost),
            "stale fence reached a cached worker receipt"
        );
        client
            .call("environment.lease.release", json!({"lease":second}))
            .await?;
        let report: Value = json!({"passed":true,"scope":"hosted_native_service_and_controller_ssh",
            "identity":client.identity(),"daemon_id":ping["daemon_id"],"daemon_pid":ping["daemon_pid"],
            "first_epoch":first.epoch,"last_epoch":second.epoch,"artifact_sha256":artifact["sha256"],
            "production_isolation_verified":false});
        use std::io::Write;
        fs::File::create_new(profile.output)?.write_all(&serde_json::to_vec_pretty(&report)?)?;
        client.close().await?;
        Ok(())
    }
    fn fixture(root: &Path) -> Result<ManagedSsh> {
        use base64::Engine;
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
    fn managed_ssh_requires_the_enrolled_host_key_identity_and_unexpanded_paths() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let mut config = fixture(&temporary.path().join("paths with spaces"))?;
        config.command("ssh", "192.0.2.1")?;
        for host in [
            "-oProxyCommand=anything",
            "user@host",
            "host\nother",
            "ssh://host",
        ] {
            assert!(config.command("ssh", host).is_err());
        }
        fs::write(
            &config.known_hosts_file,
            format!(
                "{}extra alias ssh-ed25519 unexpected\n",
                config.known_hosts_record()?
            ),
        )?;
        assert!(config.command("ssh", "192.0.2.1").is_err());
        config.user = "Administrator".into();
        assert!(config.validate().is_err());
        config.user = format!("wkgm{}", "a".repeat(16));
        config.identity_file = temporary.path().join("%h");
        assert!(config.validate().is_err());
        Ok(())
    }
    #[test]
    fn native_ssh_command_has_explicit_pins_and_no_shell_or_forwarding_configuration() -> Result<()>
    {
        let temporary = tempfile::tempdir()?;
        let config = fixture(&temporary.path().join("paths with spaces"))?;
        let command = config.command("ssh", "2001:db8::1")?;
        let args = command
            .as_std()
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.windows(2).any(|s| s == ["-F", "none"]));
        for value in [
            "StrictHostKeyChecking=yes",
            "IdentitiesOnly=yes",
            "IdentityAgent=none",
            "ClearAllForwardings=yes",
            "ControlPath=none",
        ] {
            assert!(args.iter().any(|s| s == value));
        }
        assert!(
            args.iter()
                .any(|s| s.starts_with("UserKnownHostsFile=\"") && s.contains("paths with spaces"))
        );
        assert_eq!(
            &args[args.len() - 3..],
            ["--", "2001:db8::1", "wukong-native-relay-v2"]
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires WUKONG_TEST_SSH pointing to an explicit OpenSSH client; configuration query only"]
    fn official_openssh_parses_native_pins_and_paths_with_spaces_without_connecting() -> Result<()>
    {
        let executable = std::env::var_os("WUKONG_TEST_SSH")
            .context("set WUKONG_TEST_SSH to a verified OpenSSH client")?;
        let temporary = tempfile::tempdir()?;
        let config = fixture(&temporary.path().join("paths with spaces"))?;
        let base = config.command("ssh", "192.0.2.1")?;
        let mut query = std::process::Command::new(executable);
        query.arg("-G").args(base.as_std().get_args()).env_clear();
        for key in [
            "SystemRoot",
            "WINDIR",
            "PROGRAMDATA",
            "PATH",
            "TEMP",
            "TMP",
            "HOME",
            "USERPROFILE",
        ] {
            if let Some(value) = std::env::var_os(key) {
                query.env(key, value);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            query.creation_flags(0x08000000);
        }
        let output = query.output()?;
        ensure!(
            output.status.success(),
            "OpenSSH rejected the native client configuration: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout)?;
        let values = text
            .lines()
            .filter_map(|line| line.split_once(' '))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(values.get("hostkeyalias"), Some(&config.alias().as_str()));
        assert_eq!(values.get("preferredauthentications"), Some(&"publickey"));
        assert_eq!(values.get("hostkeyalgorithms"), Some(&"ssh-ed25519"));
        assert_eq!(values.get("stricthostkeychecking"), Some(&"true"));
        assert_eq!(values.get("clearallforwardings"), Some(&"yes"));
        assert!(
            values
                .get("userknownhostsfile")
                .context("missing OpenSSH host-pin path")?
                .contains("paths with spaces")
        );
        Ok(())
    }
    #[test]
    fn registered_native_ssh_resolves_configuration_paths_lazily_and_keeps_installation_binding()
    -> Result<()> {
        use crate::{
            config::AppConfig,
            environments::{EnvironmentConfig, Platform},
            runtime::{RuntimeBackend, RuntimeConfig},
        };
        let temporary = tempfile::tempdir()?;
        let mut profile = fixture(&temporary.path().join("pins"))?;
        fs::remove_file(&profile.identity_file)?;
        fs::remove_file(&profile.known_hosts_file)?;
        let mut environment = EnvironmentConfig {
            name: "Windows VM".into(),
            platform: Platform::Windows,
            installation_id: Some(profile.installation_id.clone()),
            runtime: RuntimeConfig {
                backend: RuntimeBackend::Ssh,
                ssh_host: Some("192.0.2.1".into()),
                managed_ssh: Some(profile.clone()),
                ..Default::default()
            },
        };
        environment.validate()?; // Registration does not open the absent private files.
        environment.installation_id = Some("b".repeat(32));
        assert!(environment.validate().is_err());
        environment.installation_id = Some(profile.installation_id.clone());
        profile.identity_file = "keys/private".into();
        profile.known_hosts_file = "keys/known_hosts".into();
        environment.runtime.managed_ssh = Some(profile);
        let mut config = AppConfig {
            default_environment: Some("windows".into()),
            ..Default::default()
        };
        config.environments.insert("windows".into(), environment);
        let path = temporary.path().join("config.toml");
        fs::write(&path, toml::to_string(&config)?)?;
        let resolved = AppConfig::load(Some(&path))?;
        let profile = resolved.environments["windows"]
            .runtime
            .managed_ssh
            .as_ref()
            .unwrap();
        assert_eq!(
            profile.identity_file,
            temporary.path().canonicalize()?.join("keys/private")
        );
        assert_eq!(
            profile.known_hosts_file,
            temporary.path().canonicalize()?.join("keys/known_hosts")
        );
        assert!(profile.command("ssh", "192.0.2.1").is_err());
        Ok(())
    }
}
