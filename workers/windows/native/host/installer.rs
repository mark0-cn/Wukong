//! Offline VM bootstrap. All network-capable asset preparation stays on the
//! controller; this module has no download, MSI custom-action or shell-script path.
use crate::{
    host::accounts::{self, Role},
    host::authority::{Authority, SignedEnrollment},
    host::deployment::{self, Deployment},
    host::network,
    host::service_access,
    host::staging,
    isolation::environment,
    isolation::processes,
    isolation::security::{self, AclPolicy},
    rpc::pipe_transport,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    io::Write,
    mem::size_of,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    Storage::FileSystem::{READ_CONTROL, WRITE_DAC, WRITE_OWNER},
    System::{
        Services::*,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};
use wukong_worker_package::{
    Compatibility, PublicKey, SshPublicKey, VerifiedEnrollment, VerifiedManifest, verify_package,
};

/// Written receipt format version; version 2 adds guest address candidates.
pub const RECEIPT_VERSION: u64 = 2;
const RECEIPT_BYTES_LIMIT: u64 = 64 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    pub schema: u32,
    pub controller_public_key: String,
    pub controller_version: String,
}
pub struct Plan {
    pub base: PathBuf,
    pub setup: Setup,
    pub manifest: VerifiedManifest,
    pub enrollment: VerifiedEnrollment,
}
impl Plan {
    pub fn read(configuration: &Path, pinned: &PublicKey) -> Result<Self> {
        ensure!(
            configuration.is_absolute()
                && configuration.file_name() == Some(OsStr::new("setup.json")),
            "offline setup must use an absolute setup.json path"
        );
        let setup: Setup =
            serde_json::from_slice(&crate::artifacts::io::read_bytes(configuration, 16384)?)?;
        ensure!(
            setup.schema == 1 && setup.controller_public_key == pinned.to_hex(),
            "offline setup differs from the independently pinned controller key"
        );
        let base = configuration
            .parent()
            .context("offline setup has no directory")?
            .to_owned();
        let manifest = verify_package(
            &base.join("payload"),
            pinned,
            &Compatibility {
                runtime_protocol: 3,
                controller_version: &setup.controller_version,
            },
        )?;

        let enrollment = deployment::read_enrollment(&base.join("enrollment"), pinned)?;
        enrollment.bind_payload(&manifest)?;
        let bytes = crate::artifacts::io::read_bytes(&std::env::current_exe()?, 64 * 1024 * 1024)?;
        ensure!(
            hex::encode(Sha256::digest(bytes))
                == manifest.manifest().files[&manifest.manifest().entrypoints.worker].sha256,
            "offline setup executable differs from its signed worker payload"
        );
        Ok(Self {
            base,
            setup,
            manifest,
            enrollment,
        })
    }
}
pub fn elevated() -> Result<bool> {
    let mut token = null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no closing, and `token` is a
    // null-initialized out-param; `check` proves success before `Handle` takes ownership.
    security::check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
    let token = security::Handle(token);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0;
    // SAFETY: `token.0` is a live `Handle`, `elevation` is a live `TOKEN_ELEVATION` on this
    // frame, and the call's declared length is exactly `size_of::<TOKEN_ELEVATION>()`.
    security::check(unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    })?;
    Ok(elevation.TokenIsElevated != 0)
}
/// One complete installation receipt found below the protected installation base.
pub struct InstalledReceipt {
    pub installation_id: String,
    pub receipt: Value,
}
/// Protected installation base directory; every installation owns one child directory.
pub fn install_base() -> Result<PathBuf> {
    Ok(environment::program_data()?.join("WukongWorkers"))
}
/// Public receipt path of one installation. The installation ID is revalidated so
/// no caller can turn this into an arbitrary path.
pub fn installation_receipt_path(base: &Path, installation_id: &str) -> Result<PathBuf> {
    pipe_transport::service_name(installation_id)?;
    Ok(base.join(installation_id).join("connection.json"))
}
/// List every installation below `base` that has a complete receipt. A directory
/// without one is a failed or foreign entry, not an existing installation.
pub fn installed_receipts(base: &Path) -> Result<Vec<InstalledReceipt>> {
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut receipts = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if pipe_transport::service_name(&name).is_err() || !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("connection.json");
        // A receipt that cannot be read or validated is not treated as installed, so
        // the media path may still repair an interrupted installation below `base`.
        let Ok(bytes) = crate::artifacts::io::read_bytes(&path, RECEIPT_BYTES_LIMIT) else {
            continue;
        };
        let Ok(receipt) = installed_receipt(&bytes, &name) else {
            continue;
        };
        receipts.push(InstalledReceipt {
            installation_id: name,
            receipt,
        });
    }
    receipts.sort_by(|left, right| left.installation_id.cmp(&right.installation_id));
    Ok(receipts)
}
/// Parse and validate one stored `connection.json` against the installation
/// directory that owns it. Older version-1 receipts remain valid for re-export.
pub fn installed_receipt(bytes: &[u8], installation_id: &str) -> Result<Value> {
    pipe_transport::service_name(installation_id)?;
    let receipt: Value =
        serde_json::from_slice(bytes).context("installed receipt is not valid JSON")?;
    let fields = receipt
        .as_object()
        .context("installed receipt is not a JSON object")?;
    ensure!(
        fields.get("schema").and_then(Value::as_u64) == Some(1),
        "unsupported installed receipt schema"
    );
    ensure!(
        fields.get("installed").and_then(Value::as_bool) == Some(true),
        "installation is not complete"
    );
    ensure!(
        fields.get("installation_id").and_then(Value::as_str) == Some(installation_id),
        "installed receipt belongs to another installation"
    );
    for name in [
        "environment_id",
        "manifest_sha256",
        "ssh_user",
        "host_key",
        "host_fingerprint",
    ] {
        ensure!(
            fields.contains_key(name),
            "installed receipt lacks required field {name}"
        );
    }
    Ok(receipt)
}
/// Refresh only the guest-dependent part of a stored receipt. Installation,
/// controller, SSH and host-key identity are copied unchanged; only the receipt
/// version and the re-enumerated address candidates change.
pub fn refreshed_receipt(receipt: &Value, candidates: Vec<String>) -> Result<Value> {
    let mut fields = receipt
        .as_object()
        .context("installed receipt is not a JSON object")?
        .clone();
    ensure!(
        fields.get("schema").and_then(Value::as_u64) == Some(1)
            && fields.get("installed").and_then(Value::as_bool) == Some(true),
        "cannot refresh an incomplete installed receipt"
    );
    let installation_id = fields
        .get("installation_id")
        .and_then(Value::as_str)
        .context("installed receipt has no installation identity")?;
    pipe_transport::service_name(installation_id)?;
    fields.insert("receipt_version".into(), json!(RECEIPT_VERSION));
    fields.insert("candidate_addresses".into(), json!(candidates));
    Ok(Value::Object(fields))
}
fn copy_new(source: &Path, target: &Path, policy: &AclPolicy, maximum: u64) -> Result<()> {
    let bytes = crate::artifacts::io::read_bytes(source, maximum)?;
    let mut output = policy.create_file(target)?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    drop(output);
    policy.verify(target, false)
}
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
struct Service(SC_HANDLE);
impl Drop for Service {
    fn drop(&mut self) {
        // SAFETY: `Service` owns the `SC_HANDLE` from `OpenSCManagerW`/`CreateServiceW`, and
        // `Drop` runs once, so every non-null handle is closed exactly once.
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}
fn register_service(
    root: &Path,
    deployment: &Deployment,
    manifest: &VerifiedManifest,
) -> Result<()> {
    // SAFETY: Both arguments are null, which selects the local machine's default SCM database;
    // the returned handle is checked before `Service` adopts it, because that Drop closes it.
    let manager = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CREATE_SERVICE) };
    ensure!(
        !manager.is_null(),
        "cannot open SCM for offline worker installation: {}",
        std::io::Error::last_os_error()
    );
    let manager = Service(manager);
    let name = security::wide(OsStr::new(&pipe_transport::service_name(
        &deployment.installation_id,
    )?))?;
    let command = processes::command_line(&[
        security::local_file_path(
            &deployment
                .package(root)
                .join(&manifest.manifest().entrypoints.worker),
        )?
        .into_os_string(),
        "service".into(),
        deployment.installation_id.clone().into(),
        security::local_file_path(&root.join("metadata/deployment.json"))?.into_os_string(),
    ])?;
    // SAFETY: `manager.0` is a non-null SCM handle from the `ensure!` above, and `name`/`command`
    // are NUL-terminated wide buffers in locals that outlive this call; options are null.
    let handle = unsafe {
        CreateServiceW(
            manager.0,
            name.as_ptr(),
            name.as_ptr(),
            SERVICE_START | SERVICE_QUERY_STATUS | READ_CONTROL | WRITE_DAC | WRITE_OWNER,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            command.as_ptr(),
            null(),
            null_mut(),
            null(),
            null(),
            null(),
        )
    };
    ensure!(
        !handle.is_null(),
        "cannot create owned worker service; no existing service will be adopted: {}",
        std::io::Error::last_os_error()
    );
    let service = Service(handle);
    service_access::apply(service.0, &deployment.account(Role::Management)?.sid)
        .context("set owned service status-only relay access")?;
    // SAFETY: `service.0` is the non-null service handle proven by the `ensure!` above, and a zero
    // argument count with a null vector is the documented no-arguments form.
    security::check(unsafe { StartServiceW(service.0, 0, null()) })?;
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        let mut status = SERVICE_STATUS_PROCESS::default();
        let mut returned = 0;
        // SAFETY: `service.0` is a live handle created with SERVICE_QUERY_STATUS, and `status` is a
        // live `SERVICE_STATUS_PROCESS` whose declared buffer size is exactly its own type.
        security::check(unsafe {
            QueryServiceStatusEx(
                service.0,
                SC_STATUS_PROCESS_INFO,
                (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
                size_of::<SERVICE_STATUS_PROCESS>() as u32,
                &mut returned,
            )
        })?;
        if status.dwCurrentState == SERVICE_RUNNING {
            return Ok(());
        }
        ensure!(
            status.dwCurrentState != SERVICE_STOPPED,
            "offline worker service failed readiness (Windows error {}); inspect protected service diagnostics",
            status.dwWin32ExitCode
        );
        ensure!(
            Instant::now() < deadline,
            "offline worker service did not reach readiness"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Elevated VM-only operation. The setup GUI reviews this concrete plan first.
/// Task tests never invoke this on the developer's installed environment.
pub fn install(plan: Plan) -> Result<Value> {
    ensure!(
        elevated()?,
        "offline worker installation requires VM administrator elevation"
    );
    let enrollment = plan.enrollment.enrollment();
    let base = environment::program_data()?.join("WukongWorkers");
    let private = AclPolicy::private_vm();
    let public = AclPolicy::code_vm();
    if base.exists() {
        public.verify(&base, true)?;
    } else {
        public.create_directory(&base)?;
    }
    let root = base.join(&enrollment.installation_id);
    let metadata = root.join("metadata");
    let recovering = root.exists();
    if recovering {
        crate::host::recovery::early_account_phase(&root, &plan)?;
    } else {
        private.create_directory(&root)?;
        private.create_directory(&metadata)?;
        crate::artifacts::io::write_new_json(
            &metadata.join("installation-intent.json"),
            &json!({"schema":1,"installation_id":enrollment.installation_id,"manifest_sha256":plan.manifest.sha256(),"generation":enrollment.generation,"roles":["management","worker","debugger","target"]}),
            &private,
        )?;
        let key = PublicKey::from_hex(&plan.setup.controller_public_key)?;
        let signed = SignedEnrollment::read(&plan.base.join("enrollment"), &key)?;
        ensure!(
            signed.enrollment.canonical_bytes()? == enrollment.canonical_bytes()?,
            "installation enrollment changed after review"
        );
        Authority::new(signed, &key)?.persist(&metadata, &private)?;
    }
    let mut accounts = Vec::new();
    for role in [Role::Management, Role::Worker, Role::Debugger, Role::Target] {
        let account = if recovering && role == Role::Management {
            accounts::recover_management(&enrollment.installation_id)?
        } else {
            accounts::create(&enrollment.installation_id, role)?
        };
        crate::artifacts::io::write_new_json(
            &metadata.join(format!("account-{role:?}.json")),
            &account,
            &private,
        )?;
        accounts.push(account);
    }
    let deployment = Deployment {
        schema: 1,
        installation_id: enrollment.installation_id.clone(),
        controller_public_key: plan.setup.controller_public_key.clone(),
        controller_version: plan.setup.controller_version.clone(),
        active_manifest_sha256: plan.manifest.sha256().into(),
        accounts,
    };
    let code = deployment.code_policy()?;
    code.apply(&root, true)?;
    code.create_directory(&root.join("versions"))?;
    let package = deployment.package(&root);
    let key = PublicKey::from_hex(&deployment.controller_public_key)?;
    staging::stage_payload(
        &plan.base.join("payload"),
        &package,
        &key,
        &Compatibility {
            runtime_protocol: 3,
            controller_version: &deployment.controller_version,
        },
        &code,
    )?;
    for name in ["enrollment.json", "enrollment.sig"] {
        copy_new(
            &plan.base.join("enrollment").join(name),
            &metadata.join(name),
            &private,
            16384,
        )?;
    }
    let worker = deployment.account(Role::Worker)?;
    let data = deployment.data_policy()?;
    data.create_directory(&root.join("data"))?;
    data.create_directory(&root.join("data/tmp"))?;
    crate::artifacts::io::write_new_json(
        &root.join("data/installation.json"),
        &json!({"installation_id":deployment.installation_id}),
        &data,
    )?;
    crate::artifacts::io::write_new_json(
        &root.join("binding.json"),
        &json!({"schema":1,"protocol":3,"installation_id":deployment.installation_id,"package_sha256":plan.manifest.sha256()}),
        &private.clone().with_account(&worker.sid, false)?,
    )?;
    let entry = &plan.manifest.manifest().entrypoints;
    code.create_directory(&root.join("bootstrap"))?;
    let relay = root.join("bootstrap/wukong-worker.exe");
    copy_new(
        &package.join(&entry.worker),
        &relay,
        &code,
        64 * 1024 * 1024,
    )?;
    // ssh-keygen creates its files before we can apply an explicit file ACL.
    // Keep the entire generation directory private from its first creation.
    private.create_directory(&root.join("ssh"))?;
    private.create_directory(&metadata.join("system-tmp"))?;
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
        &root,
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
    let fingerprint = host_public.fingerprint()?;
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
        &root,
        Duration::from_secs(30),
    )?;
    crate::artifacts::io::write_new_json(&metadata.join("deployment.json"), &deployment, &private)?;
    deployment
        .verify(&root)
        .context("verify installed worker state before activation")?;
    crate::isolation::firewall::add(
        &deployment.installation_id,
        &package.join(&entry.sshd),
        enrollment.ssh_port,
        enrollment.management_peer,
    )
    .context("configure owned inbound SSH firewall")?;
    crate::artifacts::io::write_new_json(
        &metadata.join("firewall-created.json"),
        &json!({"name":pipe_transport::service_name(&deployment.installation_id)?}),
        &private,
    )?;
    register_service(&root, &deployment, &plan.manifest).with_context(|| {
        format!(
            "register and start owned worker service; startup diagnostic: {}",
            super::startup::failure_summary(
                &metadata.join("deployment.json"),
                &deployment.installation_id,
                plan.manifest.sha256()
            )
            .unwrap_or_else(|_| "current bounded diagnostic unavailable".into())
        )
    })?;
    // Candidate addresses are captured after the guest network state is final for
    // this install attempt; enumeration is best effort and never fails the install.
    let candidates = network::receipt_candidates(&network::SystemAdapters);
    let result = json!({"schema":1,"receipt_version":RECEIPT_VERSION,"installed":true,"installation_id":deployment.installation_id,"environment_id":enrollment.environment_id,"generation":enrollment.generation,"manifest_sha256":plan.manifest.sha256(),"ssh_user":management.name,"ssh_port":enrollment.ssh_port,"host_key":host_public,"host_fingerprint":fingerprint,"candidate_addresses":candidates,"production_isolation_verified":false});
    crate::artifacts::io::write_new_json(&root.join("connection.json"), &result, &code)?;
    Ok(result)
}

#[cfg(test)]
#[path = "installer_tests.rs"]
mod tests;
