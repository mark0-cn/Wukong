//! SCM lifetime and authenticated SSH transport. Worker dispatch and state live in worker.rs.
use crate::{
    host::accounts::Role,
    host::deployment::{self, Deployment},
    isolation::processes::{OwnedProcess, Spawn},
    isolation::security::{self, AclPolicy},
    rpc::pipe_transport,
    rpc::worker::Worker,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    ffi::{OsStr, c_void},
    fs,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{io::BufReader, net::windows::named_pipe::NamedPipeServer, sync::Semaphore};
use windows_sys::Win32::System::Services::*;

struct ContextState {
    installation: String,
    configuration: PathBuf,
}
static CONTEXT: OnceLock<ContextState> = OnceLock::new();
static STATUS: AtomicUsize = AtomicUsize::new(0);
static STOP: AtomicBool = AtomicBool::new(false);
static CHECKPOINT: AtomicUsize = AtomicUsize::new(0);
fn report(state: u32, error: u32) -> Result<()> {
    let pending = matches!(state, SERVICE_START_PENDING | SERVICE_STOP_PENDING);
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        } else {
            0
        },
        dwWin32ExitCode: error,
        dwCheckPoint: if pending {
            CHECKPOINT.fetch_add(1, Ordering::AcqRel) as u32 + 1
        } else {
            0
        },
        dwWaitHint: if pending { 120000 } else { 0 },
        ..Default::default()
    };
    security::check(unsafe {
        SetServiceStatus(
            STATUS.load(Ordering::Acquire) as SERVICE_STATUS_HANDLE,
            &status,
        )
    })
}
unsafe extern "system" fn control(
    code: u32,
    _event: u32,
    _data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    if code == SERVICE_CONTROL_STOP || code == SERVICE_CONTROL_SHUTDOWN {
        STOP.store(true, Ordering::Release);
        let _ = report(SERVICE_STOP_PENDING, 0);
    }
    0
}
unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
    let Some(context) = CONTEXT.get() else {
        return;
    };
    let Ok(name) = pipe_transport::service_name(&context.installation)
        .and_then(|s| security::wide(OsStr::new(&s)))
    else {
        return;
    };
    let handle = unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control), null()) };
    if handle.is_null() {
        return;
    }
    STATUS.store(handle as usize, Ordering::Release);
    let result = (|| -> Result<()> {
        report(SERVICE_START_PENDING, 0)?;
        security::require_system()?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?;
        runtime.block_on(run(context))
    })();
    if let Err(error) = &result {
        // Operator diagnostics only; never include RPC frames or endpoint auth.
        let message = format!("native worker service failed: {error:#}\n");
        if let Some(parent) = context.configuration.parent() {
            let path = parent.join("last-service-error.txt");
            if !path.exists()
                && let Ok(mut output) = AclPolicy::private_vm().create_file(&path)
            {
                let _ = std::io::Write::write_all(&mut output, message.as_bytes());
            }
        }
    }
    let _ = report(SERVICE_STOPPED, if result.is_ok() { 0 } else { 1066 });
}
pub fn dispatch(installation: &str, configuration: &Path) -> Result<()> {
    pipe_transport::service_name(installation)?;
    ensure!(
        configuration.is_absolute(),
        "SCM configuration must be absolute"
    );
    CONTEXT
        .set(ContextState {
            installation: installation.into(),
            configuration: configuration.to_owned(),
        })
        .map_err(|_| anyhow::anyhow!("SCM dispatcher already initialized"))?;
    let mut name = security::wide(OsStr::new(&pipe_transport::service_name(installation)?))?;
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: null_mut(),
            lpServiceProc: None,
        },
    ];
    security::check(unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) })
}

fn spawn_ssh(
    root: &Path,
    package: &Path,
    manifest: &wukong_worker_package::VerifiedManifest,
) -> Result<OwnedProcess> {
    let environment = crate::isolation::environment::system(root)?;
    let path = root.join("metadata/sshd-native.log");
    security::no_links(&path)?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let input = fs::File::open("NUL")?;
    Spawn {
        executable: &package.join(&manifest.manifest().entrypoints.sshd),
        arguments: &[
            "-D".into(),
            "-e".into(),
            "-f".into(),
            root.join("ssh/sshd_config").into_os_string(),
        ],
        directory: root,
        environment: &environment,
        log: &log,
        input: &input,
    }
    .system()
}
async fn offline_selftest(
    root: &Path,
    package: &Path,
    deployment: &Deployment,
    manifest: &wukong_worker_package::VerifiedManifest,
) -> Result<()> {
    let receipt_path = root
        .join("metadata")
        .join(format!("selftest-{}.json", manifest.sha256()));
    if receipt_path.exists() {
        AclPolicy::private_vm().verify(&receipt_path, false)?;
        let receipt: Value =
            serde_json::from_slice(&crate::artifacts::io::read_bytes(&receipt_path, 4096)?)?;
        ensure!(
            receipt
                == json!({"schema":1,"passed":true,"scope":"offline_dependencies_and_trusted_fixtures","signed_manifest_sha256":manifest.sha256(),"production_isolation_verified":false}),
            "invalid native offline self-test receipt"
        );
        return Ok(());
    }
    let mut environment = deployment::worker_environment(root, package)?;
    environment.insert(
        "PROGRAMDATA".into(),
        std::env::var_os("ProgramData").context("Windows ProgramData is missing")?,
    );
    let log_path = root.join("metadata").join(format!(
        "selftest-{}.log",
        hex::encode(crate::host::accounts::random_bytes::<16>()?)
    ));
    let log = AclPolicy::private_vm().create_file(&log_path)?;
    let input = fs::File::open("NUL")?;
    let process = Spawn {
        executable: &package.join(&manifest.manifest().entrypoints.python),
        arguments: &[
            "-B".into(),
            "-E".into(),
            package
                .join("runtime/worker/worker_selftest.py")
                .into_os_string(),
            "--package".into(),
            package.as_os_str().to_owned(),
            "--state".into(),
            root.join("data/selftests").into_os_string(),
        ],
        directory: &root.join("data"),
        environment: &environment,
        log: &log,
        input: &input,
    }
    .worker(
        &deployment.installation_id,
        deployment.account(Role::Worker)?,
    )?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let mut checkpoint = tokio::time::Instant::now();
    while !process.exited() {
        ensure!(
            !STOP.load(Ordering::Acquire),
            "service stopped during offline self-test"
        );
        ensure!(
            tokio::time::Instant::now() < deadline,
            "offline dependency self-test exceeded deadline"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        if checkpoint.elapsed() > Duration::from_secs(5) {
            report(SERVICE_START_PENDING, 0)?;
            checkpoint = tokio::time::Instant::now();
        }
    }
    ensure!(
        process.exit_code()? == Some(0),
        "packaged offline self-test failed; inspect its native log"
    );
    process.stop(Duration::from_secs(10))?;
    drop(process);
    drop(log);
    let report = crate::artifacts::io::read_bytes(&log_path, 8 * 1024 * 1024)?;
    let report = report
        .split(|b| *b == b'\n')
        .rev()
        .find(|line| line.iter().any(|b| !b.is_ascii_whitespace()))
        .context("offline self-test produced no report")?;
    let report: Value = serde_json::from_slice(report)?;
    ensure!(
        report["passed"] == true
            && report["signed_manifest_sha256"] == manifest.sha256()
            && report["scope"] == "offline_dependencies_and_trusted_fixtures"
            && report["production_isolation_verified"] == false,
        "offline self-test report differs from signed deployment"
    );
    manifest.verify_tree(package)?;
    crate::artifacts::io::write_new_json(
        &receipt_path,
        &json!({"schema":1,"passed":true,"scope":"offline_dependencies_and_trusted_fixtures","signed_manifest_sha256":manifest.sha256(),"production_isolation_verified":false}),
        &AclPolicy::private_vm(),
    )
}
async fn serve_client(
    server: NamedPipeServer,
    sid: String,
    worker: Arc<Worker>,
    _permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<()> {
    let rpc = crate::rpc::server::Server::new(worker)?;
    let mut stream = BufReader::new(server);
    loop {
        let Some(frame) = tokio::time::timeout(
            Duration::from_secs(310),
            pipe_transport::read_frame(&mut stream),
        )
        .await??
        else {
            return Ok(());
        };
        pipe_transport::authenticate_client(stream.get_ref(), &sid)?;
        let reply = rpc.call(&frame).await?;
        pipe_transport::write_frame(stream.get_mut(), &reply).await?;
    }
}
async fn run(context: &ContextState) -> Result<()> {
    let (deployment, root) = deployment::load(&context.configuration)?;
    ensure!(
        deployment.installation_id == context.installation,
        "SCM service installation differs from protected state"
    );
    let manifest = deployment.verify(&root)?;
    crate::host::service_access::verify_registered(
        &deployment.installation_id,
        &deployment.account(Role::Management)?.sid,
    )?;
    let package = deployment.package(&root);
    deployment::verify_running_binary(&package, &manifest)?;
    let binding: Value = serde_json::from_slice(&crate::artifacts::io::read_bytes(
        &root.join("binding.json"),
        4096,
    )?)?;
    ensure!(
        binding
            == json!({"schema":1,"protocol":3,"installation_id":deployment.installation_id,"package_sha256":manifest.sha256()}),
        "native worker binding differs from active signed payload"
    );
    let identity =
        crate::rpc::worker_identity::identity(&deployment.installation_id, manifest.sha256())?;
    let worker = Arc::new(Worker::open(identity.clone(), &root.join("metadata"))?);
    worker.open_resources(&root, &deployment, &manifest)?;
    let recovery_deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    while !worker.poll_resources()? {
        ensure!(
            !STOP.load(Ordering::Acquire) && tokio::time::Instant::now() < recovery_deadline,
            "native startup resource recovery did not finish"
        );
        report(SERVICE_START_PENDING, 0)?;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    offline_selftest(&root, &package, &deployment, &manifest).await?;
    let mut checkpoint = tokio::time::Instant::now();
    crate::host::resource_host::qualify(&root, &package, &deployment, &manifest, || {
        ensure!(
            !STOP.load(Ordering::Acquire),
            "service stopped during resource qualification"
        );
        if checkpoint.elapsed() > Duration::from_secs(5) {
            report(SERVICE_START_PENDING, 0)?;
            checkpoint = tokio::time::Instant::now();
        }
        Ok(())
    })
    .await?;
    worker.qualify();
    let sid = deployment.account(Role::Management)?.sid.clone();
    let mut listener = pipe_transport::server(&context.installation, &sid, true)?;
    let ssh = spawn_ssh(&root, &package, &manifest)?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    ensure!(
        !ssh.exited(),
        "packaged SSH daemon failed to start; inspect native SSH log"
    );
    report(SERVICE_RUNNING, 0)?;
    let semaphore = Arc::new(Semaphore::new(15));
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            result=listener.connect(), if semaphore.available_permits()>0 => {
                result?;
                let permit=semaphore.clone().acquire_owned().await?;
                let next=pipe_transport::server(&context.installation,&sid,false)?;
                let accepted=std::mem::replace(&mut listener,next);
                tasks.spawn(serve_client(accepted,sid.clone(),worker.clone(),permit));
            },
            _=tokio::time::sleep(Duration::from_millis(250))=>{
                worker.poll_resources()?;
                if STOP.load(Ordering::Acquire) {break;}
                ensure!(!ssh.exited(),"packaged SSH daemon exited");
            },
            _=tasks.join_next(), if !tasks.is_empty()=>{},
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    worker.cancel_resources().await?;
    let resource_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !worker.poll_resources()? {
        ensure!(
            tokio::time::Instant::now() < resource_deadline,
            "native resource cleanup remains pending during service stop"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    ssh.stop(Duration::from_secs(10))?;
    report(SERVICE_STOP_PENDING, 0)?;
    drop(ssh);
    Ok(())
}

/// Forced command ignores SSH_ORIGINAL_COMMAND and every caller-supplied path.
/// It can connect only to the enrolled service's pipe, verified against SCM.
pub async fn relay(installation: &str) -> Result<()> {
    let pipe = pipe_transport::connect(installation).await?;
    let mut pipe = BufReader::new(pipe);
    let mut input = BufReader::new(tokio::io::stdin());
    let mut output = tokio::io::stdout();
    while let Some(frame) = pipe_transport::read_frame(&mut input).await? {
        pipe_transport::write_frame(pipe.get_mut(), &frame).await?;
        let reply = pipe_transport::read_frame(&mut pipe)
            .await?
            .context("enrolled native service closed without a response")?;
        pipe_transport::write_frame(&mut output, &reply).await?;
    }
    Ok(())
}
