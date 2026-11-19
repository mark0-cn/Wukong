//! Guest-side SSH identity and configuration for one reviewed installation.
use super::{accounts::Role, deployment::Deployment};
use crate::isolation::{
    environment,
    security::{self, AclPolicy},
};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsStr,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};
use wukong_worker_package::{Enrollment, EntryPoints, SshPublicKey};

fn ssh_path(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .context("SSH paths must be Unicode")?
        .replace('\\', "/");
    ensure!(
        !value.contains(['"', '\n', '\r', '\0']),
        "invalid SSH configuration path"
    );
    Ok(format!("\"{value}\""))
}
fn fixed_command(
    executable: &Path,
    arguments: &[std::ffi::OsString],
    root: &Path,
    timeout: Duration,
) -> Result<()> {
    security::no_links(executable)?;
    let tool = match executable.file_name().and_then(OsStr::to_str) {
        Some("ssh-keygen.exe") => "ssh-keygen",
        Some("sshd.exe") => "sshd",
        _ => anyhow::bail!("unexpected fixed offline installation tool"),
    };
    // These two reviewed commands never emit key material. Retain their
    // diagnostics privately instead of losing pre-service failures to NUL.
    let log = AclPolicy::private_vm()
        .create_file(&root.join(format!("metadata/install-tool-{tool}.log")))?;
    let mut command = std::process::Command::new(executable);
    use std::os::windows::process::CommandExt;
    command
        .args(arguments)
        .env_clear()
        .envs(environment::system(root)?)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .with_context(|| format!("cannot launch fixed offline installation tool {tool}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "fixed offline installation tool {tool} failed: {status}; inspect its protected install-tool log"
            );
            return Ok(());
        }
        if Instant::now() >= deadline {
            child.kill()?;
            let _ = child.wait();
            anyhow::bail!("fixed offline installation tool {tool} exceeded its deadline");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
pub(super) fn configure(
    root: &Path,
    package: &Path,
    deployment: &Deployment,
    enrollment: &Enrollment,
    entry: &EntryPoints,
    code: &AclPolicy,
) -> Result<SshPublicKey> {
    let private = AclPolicy::private_vm();
    code.create_directory(&root.join("bootstrap"))?;
    let relay = root.join("bootstrap/wukong-worker.exe");
    crate::artifacts::io::copy_new(&package.join(&entry.worker), &relay, code, 64 * 1024 * 1024)?;
    // ssh-keygen creates its files before we can apply an explicit file ACL.
    // Keep the entire generation directory private from its first creation.
    private.create_directory(&root.join("ssh"))?;
    private.create_directory(&root.join("metadata/system-tmp"))?;
    let host_key = root.join("ssh/ssh_host_ed25519_key");
    fixed_command(
        &package.join(&entry.ssh_keygen),
        &[
            "-q".into(),
            "-t".into(),
            "ed25519".into(),
            "-N".into(),
            "".into(),
            "-f".into(),
            host_key.as_os_str().to_owned(),
        ],
        root,
        Duration::from_secs(30),
    )?;
    private.apply(&host_key, false)?;
    private.apply(&host_key.with_extension("pub"), false)?;
    let key_text = String::from_utf8(crate::artifacts::io::read_bytes(
        &host_key.with_extension("pub"),
        1024,
    )?)?;
    let fields = key_text.split_whitespace().collect::<Vec<_>>();
    ensure!(fields.len() >= 2, "invalid generated SSH public host key");
    let host_public = SshPublicKey {
        algorithm: fields[0].into(),
        key: fields[1].into(),
    };
    host_public.fingerprint()?;
    let management = deployment.account(Role::Management)?;
    let mut authorized = private
        .clone()
        .with_account(&management.sid, false)?
        .create_file(&root.join("bootstrap/authorized_keys"))?;
    writeln!(
        authorized,
        "restrict,from=\"{}\" {}",
        enrollment.management_peer,
        enrollment.ssh_public_key.authorized_key()?
    )?;
    authorized.sync_all()?;
    drop(authorized);
    let config = format!(
        "Port {}\nListenAddress 0.0.0.0\nHostKey {}\nAuthorizedKeysFile {}\nAuthenticationMethods publickey\nPubkeyAuthentication yes\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitEmptyPasswords no\nPermitTTY no\nDisableForwarding yes\nAllowAgentForwarding no\nAllowTcpForwarding no\nX11Forwarding no\nPermitTunnel no\nPermitUserRC no\nPermitUserEnvironment no\nStrictModes yes\nUseDNS no\nLoginGraceTime 30\nMaxAuthTries 3\nMaxSessions 16\nAllowUsers {}@{}\nForceCommand {} relay {}\nLogLevel VERBOSE\n",
        enrollment.ssh_port,
        ssh_path(&host_key)?,
        ssh_path(&root.join("bootstrap/authorized_keys"))?,
        management.name,
        enrollment.management_peer,
        ssh_path(&relay)?,
        deployment.installation_id
    );
    let mut output = private.create_file(&root.join("ssh/sshd_config"))?;
    output.write_all(config.as_bytes())?;
    output.sync_all()?;
    drop(output);
    fixed_command(
        &package.join(&entry.sshd),
        &[
            "-e".into(),
            "-ddd".into(),
            "-t".into(),
            "-f".into(),
            root.join("ssh/sshd_config").into_os_string(),
        ],
        root,
        Duration::from_secs(30),
    )?;
    Ok(host_public)
}
