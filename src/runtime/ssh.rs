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
            validate_no_links(path)?;
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
        validate_host(host)?;
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
pub(crate) fn validate_host(host: &str) -> Result<()> {
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
    Ok(())
}
pub(crate) fn validate_no_links(path: &Path) -> Result<()> {
    validate_path(path)?;
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "managed SSH paths cannot contain links"
                );
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    ensure!(
                        metadata.file_attributes() & 0x400 == 0,
                        "managed SSH paths cannot contain reparse points"
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
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
#[path = "ssh_debugger_acceptance.rs"]
mod debugger_acceptance;
#[cfg(test)]
#[path = "ssh_debugger_fault_acceptance.rs"]
mod debugger_fault_acceptance;
#[cfg(test)]
#[path = "ssh_inventory_acceptance.rs"]
mod inventory_acceptance;
#[cfg(test)]
#[path = "ssh_launch_acceptance.rs"]
mod launch_acceptance;
#[cfg(test)]
#[path = "ssh_process_acceptance.rs"]
mod process_acceptance;
#[cfg(test)]
#[path = "ssh_process_fault_acceptance.rs"]
mod process_fault_acceptance;
#[cfg(test)]
mod tests {
    use super::*;
    /// The installer fixture retains its hosted-machine isolation checks.
    #[tokio::test]
    #[ignore = "requires an installed worker on a dedicated GitHub-hosted Windows runner"]
    async fn installed_windows_worker_controller_acceptance() -> Result<()> {
        installed_controller_acceptance("WUKONG_INSTALLED_WORKER_TEST", true).await
    }

    /// Only the controller runs locally; the enrolled remote VM is already installed.
    #[tokio::test]
    #[ignore = "requires an explicitly enrolled dedicated remote Windows test VM"]
    async fn remote_installed_windows_worker_controller_acceptance() -> Result<()> {
        installed_controller_acceptance("WUKONG_REMOTE_WORKER_TEST", false).await
    }

    /// An operator reboots the dedicated VM after the ready file appears, then
    /// creates the resume file only after independently checking fresh readiness.
    /// This test never reboots a machine or installs software itself.
    #[tokio::test]
    #[ignore = "requires a coordinated reboot of an explicitly enrolled dedicated Windows VM"]
    async fn remote_windows_worker_reboot_binding_acceptance() -> Result<()> {
        use crate::runtime::{
            RuntimeClient, RuntimeConfig,
            protocol::{EnvironmentLease, WorkerError},
        };
        use serde_json::json;
        use std::{io::Write, time::Duration};
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Profile {
            config: RuntimeConfig,
            manifest_sha256: String,
            ready: PathBuf,
            resume: PathBuf,
            output: PathBuf,
        }
        let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_REBOOT_TEST")?;
        enrolled_fixture_address(&profile.config, false)?;
        ensure!(
            [&profile.ready, &profile.resume, &profile.output]
                .iter()
                .all(|p| p.is_absolute() && !p.exists()),
            "reboot fixture requires new absolute coordination/output files"
        );
        let client = RuntimeClient::connect(profile.config.clone()).await?;
        let before = client.identity().clone();
        ensure!(
            before.package_sha256 == profile.manifest_sha256,
            "reboot fixture package differs"
        );
        let first: EnvironmentLease = serde_json::from_value(
            client
                .call(
                    "environment.lease.acquire",
                    json!({"run_id":"vm-reboot-boundary", "owner_id":"before-reboot"}),
                )
                .await?,
        )?;
        first.validate("vm-reboot-boundary", "before-reboot", &before)?;
        let stale = client.with_lease(first.clone());
        let sentinel = format!("reboot-stale-{}.bin", uuid::Uuid::new_v4().simple());
        fs::File::create_new(&profile.ready)?.write_all(&serde_json::to_vec_pretty(&json!({
            "scope":"remote_vm_reboot_boundary_ready", "identity":before,"lease_epoch":first.epoch,
            "sentinel":sentinel,"controller_pid":std::process::id()}))?)?;
        tokio::time::timeout(Duration::from_secs(360), async {
            while !profile.resume.exists() {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .context("operator did not complete dedicated VM reboot within the fixture deadline")?;
        // Retain the actual pre-reboot transport. Its first lost read must not be
        // replayed; the following reconnect must compare the old binding before
        // any write reaches the new worker.
        ensure!(
            client.call("ping", json!({})).await.is_err(),
            "pre-reboot transport unexpectedly survived"
        );
        let rejected = stale
            .call(
                "file.put",
                json!({"path":sentinel,"data_base64":"AAECAw==",
            "request_id":"pre-reboot-stale-write"}),
            )
            .await
            .err()
            .context("stale boot binding admitted a write")?;
        ensure!(
            rejected
                .to_string()
                .contains("identity changed; refusing execution"),
            "old controller binding did not fail at reconnect: {rejected:#}"
        );
        // The reconnect guard has already reaped its transport. Drop both old
        // handles rather than trying to kill that same exited child twice.
        drop(stale);
        drop(client);
        let fresh = RuntimeClient::connect(profile.config).await?;
        let after = fresh.identity().clone();
        ensure!(
            before.installation_id == after.installation_id
                && before.package_sha256 == after.package_sha256
                && before.protocol == after.protocol
                && before.boot_id != after.boot_id,
            "dedicated VM reboot did not change only the OS boot identity"
        );
        let second: EnvironmentLease = serde_json::from_value(
            fresh
                .call(
                    "environment.lease.acquire",
                    json!({"run_id":"vm-reboot-boundary", "owner_id":"after-reboot"}),
                )
                .await?,
        )?;
        second.validate("vm-reboot-boundary", "after-reboot", &after)?;
        ensure!(
            second.epoch > first.epoch,
            "reboot reset the native lease journal"
        );
        let bound = fresh.with_lease(second.clone());
        bound
            .call("file.stat", json!({"path":"files/hosted-smoke.bin"}))
            .await?;
        let absent = bound
            .call("file.stat", json!({"path":format!("files/{sentinel}")}))
            .await
            .err()
            .context("stale reboot write created its sentinel")?;
        ensure!(
            absent.downcast_ref::<WorkerError>().is_some()
                && absent.to_string().contains(&sentinel),
            "missing sentinel was not established by the live worker: {absent:#}"
        );
        fresh
            .call("environment.lease.release", json!({"lease":second}))
            .await?;
        fresh.close().await?;
        fs::File::create_new(&profile.output)?.write_all(&serde_json::to_vec_pretty(&json!({
            "passed":true,"scope":"real_remote_windows_vm_reboot_binding",
            "before":before,"after":after,"first_epoch":first.epoch,"last_epoch":second.epoch,
            "stale_transport_lost":true,"stale_reconnect_binding_rejected":true,"stale_write_absent":true,
            "production_isolation_verified":false}))?)?;
        Ok(())
    }

    pub(super) fn read_fixture_profile<T: serde::de::DeserializeOwned>(
        variable: &str,
    ) -> Result<T> {
        let path = PathBuf::from(
            std::env::var_os(variable).context("missing dedicated acceptance profile")?,
        );
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 16 * 1024, "acceptance profile exceeds limit");
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub(super) fn enrolled_fixture_address(
        config: &crate::runtime::RuntimeConfig,
        hosted: bool,
    ) -> Result<()> {
        let address: std::net::Ipv4Addr = config
            .ssh_host
            .as_deref()
            .context("missing worker management address")?
            .parse()?;
        ensure!(
            !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_multicast()
                && config.managed_ssh.is_some(),
            "installed-worker acceptance requires an enrolled non-loopback IPv4 worker"
        );
        if hosted {
            // The installer fixture must address its own dedicated test machine.
            drop(std::net::UdpSocket::bind((address, 0))?);
        } else {
            ensure!(
                address.is_private(),
                "remote VM acceptance requires an enrolled private IPv4 address"
            );
        }
        Ok(())
    }

    async fn installed_controller_acceptance(profile_variable: &str, hosted: bool) -> Result<()> {
        use crate::runtime::{
            RuntimeClient, RuntimeConfig,
            protocol::{EnvironmentLease, WorkerError, WorkerErrorKind},
        };
        use serde_json::{Value, json};
        if hosted {
            ensure!(
                cfg!(windows)
                    && std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
                    && std::env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted"),
                "installed-worker acceptance requires an isolated hosted Windows runner"
            );
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Profile {
            config: RuntimeConfig,
            output: PathBuf,
            manifest_sha256: String,
            minimum_epoch: u64,
        }
        let profile: Profile = read_fixture_profile(profile_variable)?;
        enrolled_fixture_address(&profile.config, hosted)?;
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
        ensure!(
            doctor["native_broker"]["available"] == true
                && doctor["native_broker"]["identity"] == doctor["identity"]
                && doctor["native_broker"]["production_execution"] == false
                && doctor["native_broker"]["scope"] == "native_broker_channel_only",
            "installed metadata worker cannot authenticate its native broker: {}",
            doctor["native_broker"]
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
            artifact["native_input"]["schema"] == 1
                && artifact["native_input"]["identity"] == doctor["identity"]
                && artifact["native_input"]["run_id"] == first.run_id
                && artifact["native_input"]["owner_id"] == first.owner_id
                && artifact["native_input"]["epoch"] == first.epoch
                && artifact["native_input"]["sha256"] == artifact["sha256"]
                && artifact["native_input"]["size"] == artifact["size"]
                && artifact["native_input"]["executable"] == false,
            "installed upload has no matching native controller-byte provenance"
        );
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
        let begin = bound
            .call(
                "file.upload.begin",
                json!({"path":"hosted-chunked.bin",
            "sha256":artifact["sha256"],"size":4,"executable":true,
            "request_id":"hosted-chunked-begin"}),
            )
            .await?;
        let transfer = begin["transfer_id"]
            .as_str()
            .context("installed transfer has no ID")?;
        let status = bound
            .call("file.upload.status", json!({"transfer_id":transfer}))
            .await?;
        ensure!(
            status["offset"] == 0 && status["complete"] == false,
            "another native fence reused worker bytes without controller provenance"
        );
        for (offset, data) in [(0, "AAE="), (2, "AgM=")] {
            let next = bound
                .call(
                    "file.upload.write",
                    json!({"transfer_id":transfer,"offset":offset,
                "data_base64":data,"request_id":format!("hosted-chunked-{}-{offset}",first.epoch)}),
                )
                .await?;
            ensure!(
                next["offset"] == offset + 2,
                "native and worker chunk acknowledgements differ"
            );
        }
        let commit = json!({"transfer_id":transfer,"request_id":"hosted-chunked-commit"});
        let chunked = bound.call("file.upload.commit", commit.clone()).await?;
        ensure!(
            chunked["native_input"]["identity"] == doctor["identity"]
                && chunked["native_input"]["epoch"] == first.epoch
                && chunked["native_input"]["sha256"] == artifact["sha256"]
                && chunked["native_input"]["size"] == 4
                && chunked["native_input"]["executable"] == true,
            "installed resumable upload has no exact native input provenance"
        );
        ensure!(
            bound.call("file.upload.commit", commit).await? == chunked,
            "native resumable upload proof changed on retry"
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
        let scope = if hosted {
            "hosted_native_service_and_controller_ssh"
        } else {
            "enrolled_remote_vm_and_controller_ssh"
        };
        let report: Value = json!({"passed":true,"scope":scope,
            "identity":client.identity(),"daemon_id":ping["daemon_id"],"daemon_pid":ping["daemon_pid"],
            "first_epoch":first.epoch,"last_epoch":second.epoch,"artifact_sha256":artifact["sha256"],
            "native_broker":doctor["native_broker"],
            "native_input":artifact["native_input"],
            "native_resumable_input":chunked["native_input"],
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
