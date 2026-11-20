//! Fixed System resource host. Private station selection stays in this child,
//! outside the concurrently serving SCM process. Analysis admission is separate.
use crate::{
    host::accounts::{self, Role},
    host::deployment::{self, Deployment},
    isolation::environment,
    isolation::persona::Persona,
    isolation::processes::Spawn,
    isolation::security::{self, AclPolicy, Handle},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::ErrorKind,
    mem::size_of,
    path::{Path, PathBuf},
    ptr::null_mut,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Security::{GetTokenInformation, TOKEN_QUERY, TokenAppContainerSid},
    System::{
        Registry::RegDisablePredefinedCacheEx,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

#[path = "resource_adversary.rs"]
mod adversary;
#[path = "resource_debugger.rs"]
mod debugger;

fn token_data(
    token: &Handle,
    class: windows_sys::Win32::Security::TOKEN_INFORMATION_CLASS,
) -> Result<Vec<usize>> {
    let mut size = 0;
    // SAFETY: `token.0` is a live handle owned by `Handle`; a null buffer with zero length is the
    // documented size-query form, and the following `ensure!` bounds the returned `size`.
    unsafe {
        GetTokenInformation(token.0, class, null_mut(), 0, &mut size);
    }
    ensure!(
        size > 0 && size <= 65536,
        "invalid resource token information length"
    );
    let mut data = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    // SAFETY: `size` was returned by the sizing call above, and `data` allocates that many bytes
    // rounded up to whole `usize` words, so it is aligned and large enough for this query.
    security::check(unsafe {
        GetTokenInformation(token.0, class, data.as_mut_ptr().cast(), size, &mut size)
    })?;
    Ok(data)
}
/// Inert sealed development probe. It cannot select a program, change an
/// account/profile or read sentinel contents.
pub fn probe(
    metadata: &Path,
    worker: &Path,
    expected_sid: &str,
    host_process: u32,
    host_thread: u32,
) -> Result<Value> {
    security::validate_app_sid(expected_sid)?;
    let mut token = null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no closing, and `token` is a
    // null-initialized out-param; `check` proves success before `Handle` takes ownership.
    security::check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
    let token = Handle(token);
    crate::isolation::token_policy::verify_appcontainer_policy(token.0)?;
    crate::isolation::processes::verify_sandbox_privileges(&token)?;
    let info = token_data(&token, TokenAppContainerSid)?;
    let sid =
        // SAFETY: `info` was filled by the `TokenAppContainerSid` query above, so its first word
        // is an aligned SID pointer; `sid_text` rejects that pointer if it is null.
        security::sid_text(unsafe { *info.as_ptr().cast::<windows_sys::Win32::Security::PSID>() })?;
    ensure!(
        sid == expected_sid,
        "resource AppContainer identity differs"
    );
    let authority = adversary::check(host_process, host_thread)?;
    for path in [metadata, worker] {
        match fs::File::open(path) {
            Err(error) if error.kind() == ErrorKind::PermissionDenied => {}
            _ => anyhow::bail!("resource could open a private sentinel or denial was inconclusive"),
        }
    }
    let scratch = std::env::current_dir()
        .context("resolve contained scratch directory")?
        .join("writable-probe.bin");
    fs::write(&scratch, b"owned-low-scratch").context("write contained scratch probe")?;
    ensure!(
        fs::read(&scratch).context("read contained scratch probe")? == b"owned-low-scratch",
        "resource scratch differs"
    );
    fs::remove_file(&scratch).context("remove contained scratch probe")?;
    security::probe_resource_root(&std::env::current_dir()?)
        .context("qualify held resource root")?;
    Ok(
        json!({"probe_completed":true,"appcontainer_sid":sid,"logon_sid":security::token_logon_sid(&token)?,"privileges":["SeChangeNotifyPrivilege"],"capabilities":0,
        "authority_after_self_dacl":authority,"metadata_denied":true,"worker_data_denied":true,"scratch_writable":true,"root_guard_denied":true}),
    )
}

pub(crate) struct Host {
    deployment: Deployment,
    pub installation: String,
    pub package: PathBuf,
    pub root: PathBuf,
    pub manifest: wukong_worker_package::VerifiedManifest,
    pub resources: PathBuf,
    pub environment: std::collections::BTreeMap<String, std::ffi::OsString>,
}
impl Host {
    pub(crate) fn account(&self, role: Role) -> Result<&crate::host::accounts::Account> {
        self.deployment.account(role)
    }
    pub(crate) fn persona(&self, role: Role, nonce: &str, resource: &str) -> Result<Persona> {
        create_persona_named(&self.deployment, &self.root, role, nonce, Some(resource))
            .map(|(persona, _)| persona)
    }
}
/// Shared, synchronous setup in a fresh System child, before any I/O threads.
pub(crate) fn open(installation: &str, configuration: &Path) -> Result<Host> {
    security::require_system()?;
    let (deployment, root) = deployment::load(configuration)?;
    ensure!(
        deployment.installation_id == installation,
        "resource host installation differs"
    );
    let manifest = deployment.verify(&root)?;
    let package = deployment.package(&root);
    deployment::verify_running_binary(&package, &manifest)?;
    // Resolve fixed Windows locations once under the fresh System context,
    // before changing predefined registry caching or loading role profiles.
    let system_environment = environment::system(&root)?;
    // SAFETY: No arguments or pointers; the process-wide predefined-handle cache change is sound
    // because `open` runs in a fresh System child on its only thread, before worker I/O.
    let status = unsafe { RegDisablePredefinedCacheEx() };
    ensure!(
        status == 0,
        "cannot disable resource-host registry handle cache: {status}"
    );
    let public = deployment.code_policy()?;
    let resources = root.join("resources");
    if !resources.exists() {
        public.create_directory(&resources)?;
    }
    public.verify(&resources, true)?;
    Ok(Host {
        installation: deployment.installation_id.clone(),
        package,
        deployment,
        root,
        manifest,
        resources,
        environment: system_environment,
    })
}
/// Internal startup qualification; it executes only this sealed binary's fixed
/// probe branch. It is not a model target execution API or production gate.
pub fn selftest(installation: &str, configuration: &Path, qualification: &str) -> Result<Value> {
    security::require_system()?;
    ensure!(
        qualification.len() == 32
            && qualification
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "invalid resource qualification identity"
    );
    let Host {
        deployment,
        root,
        manifest,
        resources,
        environment: system_environment,
        ..
    } = open(installation, configuration)?;
    let metadata = root.join("metadata/authority.json");
    AclPolicy::private_vm().verify(&metadata, false)?;
    let mut reports = Vec::new();
    for role in [Role::Target, Role::Debugger] {
        reports.push(check_persona(
            &deployment,
            &root,
            &resources,
            &metadata,
            role,
            &system_environment,
        )?);
    }
    ensure!(
        reports[0]["appcontainer_sid"] != reports[1]["appcontainer_sid"],
        "resource roles reused an AppContainer identity"
    );
    let mut debuggers = Vec::new();
    for architecture in ["x64", "x86"] {
        debuggers.push(debugger::check(
            &deployment,
            &root,
            &resources,
            &manifest,
            architecture,
            &system_environment,
        )?);
    }
    Ok(
        json!({"schema":5,"passed":true,"scope":"disposable_vm_guest_controls","execution_boundary":"disposable_vm","external_isolation_verified":false,
        "qualification_id":qualification,"manifest_sha256":manifest.sha256(),"personas":reports,"dbgeng":debuggers,"production_execution":false}),
    )
}
fn check_persona(
    deployment: &Deployment,
    root: &Path,
    resources: &Path,
    metadata: &Path,
    role: Role,
    system_environment: &std::collections::BTreeMap<String, std::ffi::OsString>,
) -> Result<Value> {
    let private = AclPolicy::private_vm();
    let (mut persona, nonce) = create_persona(deployment, root, role)?;
    let sid = persona.sid_text()?;
    let scratch =
        security::ResourceDirectory::create(&resources.join(&nonce), &sid, persona.logon_sid())?;
    let scratch_path = scratch.path().to_owned();
    let worker_sentinel = root.join("data").join(format!("resource-sentinel-{nonce}"));
    {
        use std::io::Write;
        let mut file = deployment.data_policy()?.create_file(&worker_sentinel)?;
        file.write_all(b"worker-private")?;
        file.sync_all()?;
    }
    let log_path = root
        .join("metadata")
        .join(format!("resource-probe-{nonce}.log"));
    let log = private.create_file(&log_path)?;
    let input = fs::File::open("NUL")?;
    let environment = scratch_environment(system_environment, &scratch_path);
    let executable = std::env::current_exe()?;
    let spawn = Spawn {
        executable: &executable,
        arguments: &[
            "isolation-probe".into(),
            metadata.as_os_str().to_owned(),
            worker_sentinel.as_os_str().to_owned(),
            sid.clone().into(),
            std::process::id().to_string().into(),
            // SAFETY: Takes no arguments or pointers and only reads the calling thread's id, which
            // is this live System host thread that the probe later compares against.
            unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
                .to_string()
                .into(),
        ],
        directory: &scratch_path,
        environment: &environment,
        log: &log,
        input: &input,
    };
    let process = match spawn.sandbox(&persona, scratch) {
        Ok(process) => process,
        Err(error) => {
            let diagnostic = match spawn.sandbox_creation_diagnostics(&persona, &error) {
                Ok(value) => value,
                Err(reason) => json!({"diagnostic_error":format!("{reason:#}")}),
            };
            anyhow::bail!(
                "owned {role:?} sandbox creation failed: {error:#}; controls: {diagnostic}"
            );
        }
    };
    wait(&process, Duration::from_secs(15))?;
    let code = process.exit_code()?;
    let launch_diagnostics = process.launch_diagnostics().cloned();
    process.stop(Duration::from_secs(10))?;
    drop(process);
    drop(log);
    let bytes = crate::artifacts::io::read_bytes(&log_path, 4096)?;
    ensure!(
        code == Some(0),
        "owned {role:?} AppContainer probe failed (exit {code:?}): {}; launch diagnostics: {}",
        String::from_utf8_lossy(&bytes),
        launch_diagnostics.unwrap_or(Value::Null)
    );
    let mut report: Value =
        serde_json::from_slice(&bytes).context("invalid resource probe report")?;
    adversary::validate(&report["authority_after_self_dacl"])?;
    ensure!(
        report["probe_completed"] == true
            && report["appcontainer_sid"] == sid
            && report["logon_sid"] == persona.logon_sid(),
        "resource probe identity differs"
    );
    ensure!(
        report["authority_after_self_dacl"]["protected_process_id"] == std::process::id()
            && report["authority_after_self_dacl"]["protected_thread_id"]
                // SAFETY: No arguments or pointers; returns the id of the calling host thread,
                // the same thread whose id was passed to the probe above.
                == unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() },
        "adversarial probe did not use this live System host"
    );
    report["passed"] = Value::Bool(true);
    persona.close()?;
    fs::remove_dir(&scratch_path)?;
    report["role"] = serde_json::to_value(role)?;
    report["profile_removed"] = Value::Bool(true);
    report["scratch_removed"] = Value::Bool(true);
    fs::remove_file(&worker_sentinel)?;
    Ok(report)
}

fn create_persona(deployment: &Deployment, root: &Path, role: Role) -> Result<(Persona, String)> {
    let nonce = hex::encode(accounts::random_bytes::<24>()?);
    create_persona_named(deployment, root, role, &nonce, None)
}
pub(crate) fn create_persona_named(
    deployment: &Deployment,
    root: &Path,
    role: Role,
    nonce: &str,
    resource: Option<&str>,
) -> Result<(Persona, String)> {
    ensure!(
        nonce.len() == 48
            && nonce
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid owned profile nonce"
    );
    let name = format!("Wukong-{nonce}");
    let intent = root
        .join("metadata")
        .join(format!("resource-profile-{nonce}.json"));
    crate::artifacts::io::publish_json(
        &intent,
        &json!({"schema":1,"installation_id":deployment.installation_id,
        "role":role,"account_sid":deployment.account(role)?.sid,"profile_name":name,"resource_id":resource}),
        &AclPolicy::private_vm(),
    )?;
    let persona = Persona::create(
        &deployment.installation_id,
        deployment.account(role)?,
        &name,
    )?;
    Ok((persona, nonce.into()))
}
pub(crate) fn scratch_environment(
    system_environment: &std::collections::BTreeMap<String, std::ffi::OsString>,
    scratch: &Path,
) -> std::collections::BTreeMap<String, std::ffi::OsString> {
    let mut environment = system_environment.clone();
    for key in ["TEMP", "TMP"] {
        environment.insert(key.into(), scratch.as_os_str().to_owned());
    }
    environment
}
fn wait(process: &crate::isolation::processes::OwnedProcess, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while !process.exited() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    ensure!(process.exited(), "owned resource probe exceeded deadline");
    Ok(())
}

pub(crate) async fn qualify(
    root: &Path,
    package: &Path,
    deployment: &Deployment,
    manifest: &wukong_worker_package::VerifiedManifest,
    mut tick: impl FnMut() -> Result<()>,
) -> Result<()> {
    let private = AclPolicy::private_vm();
    // Each service epoch must observe current OS policy. A prior manifest-bound
    // success is history, never evidence of this process's startup qualification.
    let qualification = hex::encode(crate::host::accounts::random_bytes::<16>()?);
    let receipt = root.join("metadata").join(format!(
        "isolation-selftest-{}-{qualification}.json",
        manifest.sha256()
    ));
    let log_path = root
        .join("metadata")
        .join(format!("isolation-host-{qualification}.log"));
    let log = private.create_file(&log_path)?;
    let input = fs::File::open("NUL")?;
    let environment = crate::isolation::environment::system(root)?;
    let process = Spawn {
        executable: &package.join(&manifest.manifest().entrypoints.worker),
        arguments: &[
            "resource-selftest".into(),
            deployment.installation_id.clone().into(),
            root.join("metadata/deployment.json").into_os_string(),
            qualification.clone().into(),
        ],
        directory: root,
        environment: &environment,
        log: &log,
        input: &input,
    }
    .system()?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    while !process.exited() {
        tick()?;
        ensure!(
            tokio::time::Instant::now() < deadline,
            "resource qualification exceeded deadline"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let code = process.exit_code()?;
    process.stop(Duration::from_secs(10))?;
    drop(process);
    drop(log);
    let bytes = crate::artifacts::io::read_bytes(&log_path, 8192)?;
    ensure!(
        code == Some(0),
        "native guest controls check failed: {}",
        String::from_utf8_lossy(&bytes)
    );
    let value: Value = serde_json::from_slice(&bytes)?;
    validate_resource_report(&value, manifest.sha256(), &qualification)?;
    crate::artifacts::io::write_new_json(&receipt, &value, &private)?;
    Ok(())
}
fn validate_resource_report(value: &Value, manifest: &str, qualification: &str) -> Result<()> {
    ensure!(
        value["schema"] == 5
            && value["passed"] == true
            && value["scope"] == "disposable_vm_guest_controls"
            && value["execution_boundary"] == "disposable_vm"
            && value["external_isolation_verified"] == false
            && value["manifest_sha256"] == manifest
            && value["qualification_id"] == qualification
            && value["production_execution"] == false,
        "resource qualification receipt differs"
    );
    let personas = value["personas"]
        .as_array()
        .context("resource qualification has no personas")?;
    ensure!(
        personas.len() == 2,
        "resource qualification role count differs"
    );
    for (persona, role) in personas.iter().zip(["target", "debugger"]) {
        adversary::validate(&persona["authority_after_self_dacl"])?;
        ensure!(
            persona["role"] == role
                && persona["passed"] == true
                && persona["profile_removed"] == true
                && persona["scratch_removed"] == true
                && persona["privileges"] == json!(["SeChangeNotifyPrivilege"])
                && persona["capabilities"] == 0
                && persona["metadata_denied"] == true
                && persona["worker_data_denied"] == true
                && persona["scratch_writable"] == true
                && persona["root_guard_denied"] == true,
            "resource qualification did not prove the required restrictions"
        );
        security::validate_app_sid(
            persona["appcontainer_sid"]
                .as_str()
                .context("resource qualification has no SID")?,
        )?;
        security::validate_logon_sid(
            persona["logon_sid"]
                .as_str()
                .context("resource qualification has no logon SID")?,
        )?;
    }
    ensure!(
        personas[0]["appcontainer_sid"] != personas[1]["appcontainer_sid"],
        "resource qualification reused an AppContainer SID"
    );
    debugger::validate_report(value)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_ordinary_development_process_cannot_enter_the_resource_host() -> Result<()> {
        ensure!(
            security::current_user_sid()? != "S-1-5-18",
            "inert unit tests must not run as System"
        );
        let root = tempfile::tempdir()?;
        let error = selftest(
            &"a".repeat(32),
            &root.path().join("not-a-deployment.json"),
            &"b".repeat(32),
        )
        .unwrap_err();
        assert!(error.to_string().contains("System"));
        assert_eq!(fs::read_dir(root.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn a_manifest_only_or_previous_service_qualification_never_admits_current_startup() {
        let manifest = "a".repeat(64);
        let current = "b".repeat(32);
        let previous = "c".repeat(32);
        let mut report = json!({"schema":3,"passed":true,"scope":"owned_appcontainer_trusted_probes",
            "manifest_sha256":manifest,"production_execution":false});
        assert!(
            validate_resource_report(&report, &manifest, &current)
                .unwrap_err()
                .to_string()
                .contains("receipt differs")
        );
        report["schema"] = json!(5);
        report["scope"] = json!("disposable_vm_guest_controls");
        report["execution_boundary"] = json!("disposable_vm");
        report["external_isolation_verified"] = json!(false);
        report["qualification_id"] = json!(previous);
        assert!(
            validate_resource_report(&report, &manifest, &current)
                .unwrap_err()
                .to_string()
                .contains("receipt differs")
        );
        report["qualification_id"] = json!(current);
        report["external_isolation_verified"] = json!(true);
        assert!(
            validate_resource_report(&report, &manifest, &current)
                .unwrap_err()
                .to_string()
                .contains("receipt differs")
        );
        report["external_isolation_verified"] = json!(false);
        assert!(
            validate_resource_report(&report, &manifest, &current)
                .unwrap_err()
                .to_string()
                .contains("no personas")
        );
    }
}
