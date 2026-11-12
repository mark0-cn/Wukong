//! Fixed System resource host. Private station selection stays in this child,
//! outside the concurrently serving SCM process. Analysis admission is separate.
use crate::{
    accounts::{self, Role},
    deployment::{self, Deployment},
    environment,
    persona::Persona,
    processes::Spawn,
    security::{self, AclPolicy, Handle},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::ErrorKind,
    mem::size_of,
    net::{TcpListener, TcpStream},
    path::Path,
    ptr::null_mut,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Security::{
        GetTokenInformation, TOKEN_QUERY, TokenAppContainerSid, TokenCapabilities,
        TokenIsAppContainer,
    },
    System::{
        Registry::RegDisablePredefinedCacheEx,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

#[path = "resource_debugger.rs"]
mod debugger;

fn token_data(
    token: &Handle,
    class: windows_sys::Win32::Security::TOKEN_INFORMATION_CLASS,
) -> Result<Vec<usize>> {
    let mut size = 0;
    unsafe {
        GetTokenInformation(token.0, class, null_mut(), 0, &mut size);
    }
    ensure!(
        size > 0 && size <= 65536,
        "invalid resource token information length"
    );
    let mut data = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    security::check(unsafe {
        GetTokenInformation(token.0, class, data.as_mut_ptr().cast(), size, &mut size)
    })?;
    Ok(data)
}
/// Inert sealed development probe. It cannot select a program, change an
/// account/profile, open a network outside loopback, or read sentinel contents.
pub fn probe(metadata: &Path, worker: &Path, port: u16, expected_sid: &str) -> Result<Value> {
    security::validate_app_sid(expected_sid)?;
    ensure!(port > 0, "missing owned probe listener");
    let mut token = null_mut();
    security::check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
    let token = Handle(token);
    let app = token_data(&token, TokenIsAppContainer)?;
    ensure!(
        unsafe { *app.as_ptr().cast::<u32>() } == 1,
        "resource probe is not an AppContainer"
    );
    crate::processes::verify_sandbox_privileges(&token)?;
    let capabilities = token_data(&token, TokenCapabilities)?;
    ensure!(
        unsafe { *capabilities.as_ptr().cast::<u32>() } == 0,
        "resource token retained capabilities"
    );
    let info = token_data(&token, TokenAppContainerSid)?;
    let sid =
        security::sid_text(unsafe { *info.as_ptr().cast::<windows_sys::Win32::Security::PSID>() })?;
    ensure!(
        sid == expected_sid,
        "resource AppContainer identity differs"
    );
    for path in [metadata, worker] {
        match fs::File::open(path) {
            Err(error) if error.kind() == ErrorKind::PermissionDenied => {}
            _ => anyhow::bail!("resource could open a private sentinel or denial was inconclusive"),
        }
    }
    match TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2)) {
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {}
        Err(error) => anyhow::bail!(
            "AppContainer loopback denial was inconclusive (kind={:?}, win32={:?}): {error}",
            error.kind(),
            error.raw_os_error()
        ),
        Ok(stream) => {
            drop(stream);
            anyhow::bail!("AppContainer unexpectedly connected to the owned loopback listener");
        }
    }
    let scratch = std::env::current_dir()?.join("writable-probe.bin");
    fs::write(&scratch, b"owned-low-scratch")?;
    ensure!(
        fs::read(&scratch)? == b"owned-low-scratch",
        "resource scratch differs"
    );
    fs::remove_file(&scratch)?;
    security::probe_resource_root(&std::env::current_dir()?)?;
    Ok(
        json!({"passed":true,"appcontainer_sid":sid,"logon_sid":security::token_logon_sid(&token)?,"privileges":["SeChangeNotifyPrivilege"],"capabilities":0,
        "metadata_denied":true,"worker_data_denied":true,"loopback_denied":true,"scratch_writable":true,"root_guard_denied":true}),
    )
}

/// Internal startup qualification; it executes only this sealed binary's fixed
/// probe branch. It is not a model target execution API or production gate.
pub fn selftest(installation: &str, configuration: &Path) -> Result<Value> {
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
    let metadata = root.join("metadata/authority.json");
    AclPolicy::private_vm().verify(&metadata, false)?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    // Demonstrate the exact listener is reachable before using access denied
    // as evidence. The probe never sends traffic outside this local listener.
    drop(TcpStream::connect_timeout(
        &([127, 0, 0, 1], port).into(),
        Duration::from_secs(2),
    )?);
    drop(listener.accept()?);
    let mut reports = Vec::new();
    for role in [Role::Target, Role::Debugger] {
        reports.push(check_persona(
            &deployment,
            &root,
            &resources,
            &metadata,
            port,
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
        json!({"schema":2,"passed":true,"scope":"owned_appcontainer_trusted_probes",
        "manifest_sha256":manifest.sha256(),"personas":reports,"dbgeng":debuggers,"production_execution":false}),
    )
}
fn check_persona(
    deployment: &Deployment,
    root: &Path,
    resources: &Path,
    metadata: &Path,
    port: u16,
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
            port.to_string().into(),
            sid.clone().into(),
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
    let bytes = deployment::read_bytes(&log_path, 4096)?;
    ensure!(
        code == Some(0),
        "owned {role:?} AppContainer probe failed (exit {code:?}): {}; launch diagnostics: {}",
        String::from_utf8_lossy(&bytes),
        launch_diagnostics.unwrap_or(Value::Null)
    );
    let mut report: Value =
        serde_json::from_slice(&bytes).context("invalid resource probe report")?;
    ensure!(
        report["passed"] == true
            && report["appcontainer_sid"] == sid
            && report["logon_sid"] == persona.logon_sid(),
        "resource probe identity differs"
    );
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
    let name = format!("Wukong-{nonce}");
    let intent = root
        .join("metadata")
        .join(format!("resource-profile-{nonce}.json"));
    deployment::write_new_json(
        &intent,
        &json!({"schema":1,"installation_id":deployment.installation_id,
        "role":role,"account_sid":deployment.account(role)?.sid,"profile_name":name}),
        &AclPolicy::private_vm(),
    )?;
    let persona = Persona::create(
        &deployment.installation_id,
        deployment.account(role)?,
        &name,
    )?;
    Ok((persona, nonce))
}
fn scratch_environment(
    system_environment: &std::collections::BTreeMap<String, std::ffi::OsString>,
    scratch: &Path,
) -> std::collections::BTreeMap<String, std::ffi::OsString> {
    let mut environment = system_environment.clone();
    for key in ["TEMP", "TMP"] {
        environment.insert(key.into(), scratch.as_os_str().to_owned());
    }
    environment
}
fn wait(process: &crate::processes::OwnedProcess, timeout: Duration) -> Result<()> {
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
    let receipt = root
        .join("metadata")
        .join(format!("isolation-selftest-{}.json", manifest.sha256()));
    if receipt.exists() {
        private.verify(&receipt, false)?;
        let value: Value = serde_json::from_slice(&deployment::read_bytes(&receipt, 8192)?)?;
        validate_resource_report(&value, manifest.sha256())?;
        return Ok(());
    }
    let log_path = root.join("metadata").join(format!(
        "isolation-host-{}.log",
        hex::encode(crate::accounts::random_bytes::<16>()?)
    ));
    let log = private.create_file(&log_path)?;
    let input = fs::File::open("NUL")?;
    let environment = crate::environment::system(root)?;
    let process = Spawn {
        executable: &package.join(&manifest.manifest().entrypoints.worker),
        arguments: &[
            "resource-selftest".into(),
            deployment.installation_id.clone().into(),
            root.join("metadata/deployment.json").into_os_string(),
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
    let bytes = deployment::read_bytes(&log_path, 8192)?;
    ensure!(
        code == Some(0),
        "native resource qualification failed: {}",
        String::from_utf8_lossy(&bytes)
    );
    let value: Value = serde_json::from_slice(&bytes)?;
    validate_resource_report(&value, manifest.sha256())?;
    deployment::write_new_json(&receipt, &value, &private)
}
fn validate_resource_report(value: &Value, manifest: &str) -> Result<()> {
    ensure!(
        value["schema"] == 2
            && value["passed"] == true
            && value["scope"] == "owned_appcontainer_trusted_probes"
            && value["manifest_sha256"] == manifest
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
        ensure!(
            persona["role"] == role
                && persona["passed"] == true
                && persona["profile_removed"] == true
                && persona["scratch_removed"] == true
                && persona["privileges"] == json!(["SeChangeNotifyPrivilege"])
                && persona["capabilities"] == 0
                && persona["metadata_denied"] == true
                && persona["worker_data_denied"] == true
                && persona["loopback_denied"] == true
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
        let error =
            selftest(&"a".repeat(32), &root.path().join("not-a-deployment.json")).unwrap_err();
        assert!(error.to_string().contains("System"));
        assert_eq!(fs::read_dir(root.path())?.count(), 0);
        Ok(())
    }
}
