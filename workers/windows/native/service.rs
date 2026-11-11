//! SCM-owned native worker epoch and authenticated fixed SSH relay. The Python
//! runtime runs as a separate ordinary account; LocalSystem never parses targets.
use crate::{
    accounts::Role,
    deployment::{self, Deployment},
    pipe_transport,
    processes::{OwnedProcess, Spawn},
    security::{self, AclPolicy},
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    ffi::{OsStr, OsString, c_void},
    fs,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::BufReader,
    net::{TcpStream, windows::named_pipe::NamedPipeServer},
    sync::Semaphore,
};
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
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        } else {
            0
        },
        dwWin32ExitCode: error,
        dwCheckPoint: if state == SERVICE_START_PENDING {
            CHECKPOINT.fetch_add(1, Ordering::AcqRel) as u32 + 1
        } else {
            0
        },
        dwWaitHint: if state == SERVICE_START_PENDING {
            120000
        } else {
            0
        },
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

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Endpoint {
    protocol: u32,
    host: String,
    port: u16,
    auth: String,
    pid: u32,
}
impl Endpoint {
    fn read(root: &Path, process: &OwnedProcess) -> Result<Self> {
        let value: Self = serde_json::from_slice(&deployment::read_bytes(
            &root.join("data/endpoint.json"),
            4096,
        )?)?;
        ensure!(
            value.protocol == 2
                && value.host == "127.0.0.1"
                && value.port > 0
                && value.pid == process.pid()
                && value.auth.len() == 64
                && value.auth.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid owned native daemon endpoint"
        );
        Ok(value)
    }
    async fn call(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        let mut request: Value = serde_json::from_slice(bytes)?;
        let object = request
            .as_object_mut()
            .context("native worker request must be an object")?;
        ensure!(
            !object.contains_key("auth"),
            "native worker authentication is service-owned"
        );
        object.insert("auth".into(), Value::String(self.auth.clone()));
        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(b'\n');
        tokio::time::timeout(Duration::from_secs(310), async {
            let stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, self.port)).await?;
            let mut stream = BufReader::new(stream);
            pipe_transport::write_frame(stream.get_mut(), &bytes).await?;
            pipe_transport::read_frame(&mut stream)
                .await?
                .context("owned worker closed without a response")
        })
        .await
        .context("owned worker RPC deadline exceeded")?
    }
}
fn spawn_worker(
    root: &Path,
    package: &Path,
    deployment: &Deployment,
    manifest: &wukong_worker_package::VerifiedManifest,
) -> Result<OwnedProcess> {
    let entry = &manifest.manifest().entrypoints;
    let environment = deployment::worker_environment(root, package)?;
    let path = root.join("data/daemon-native.log");
    security::no_links(&path)?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let input = fs::File::open("NUL")?;
    Spawn {
        executable: &package.join(&entry.python),
        arguments: &[
            OsString::from("-B"),
            OsString::from("-E"),
            package.join(&entry.bridge).into_os_string(),
            "--state".into(),
            root.join("data").into_os_string(),
            "--daemon".into(),
        ],
        directory: &root.join("data"),
        environment: &environment,
        log: &log,
        input: &input,
    }
    .worker(
        &deployment.installation_id,
        deployment.account(Role::Worker)?,
    )
}
fn spawn_ssh(
    root: &Path,
    package: &Path,
    manifest: &wukong_worker_package::VerifiedManifest,
) -> Result<OwnedProcess> {
    let mut environment = deployment::worker_environment(root, package)?;
    environment.remove("WUKONG_NATIVE_BINDING_FILE");
    environment.insert(
        "PROGRAMDATA".into(),
        std::env::var_os("ProgramData").context("Windows ProgramData is missing")?,
    );
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
        directory: &root.join("ssh"),
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
        let receipt: Value = serde_json::from_slice(&deployment::read_bytes(&receipt_path, 4096)?)?;
        ensure!(
            receipt
                == json!({"schema":1,"passed":true,"scope":"offline_dependencies_and_trusted_fixtures","signed_manifest_sha256":manifest.sha256(),"production_isolation_verified":false}),
            "invalid native offline self-test receipt"
        );
        return Ok(());
    }
    let mut environment = deployment::worker_environment(root, package)?;
    environment.remove("WUKONG_NATIVE_BINDING_FILE");
    environment.insert(
        "PROGRAMDATA".into(),
        std::env::var_os("ProgramData").context("Windows ProgramData is missing")?,
    );
    let log_path = root.join("metadata").join(format!(
        "selftest-{}.log",
        hex::encode(crate::accounts::random_bytes::<16>()?)
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
    drop(process);
    drop(log);
    let report = deployment::read_bytes(&log_path, 8 * 1024 * 1024)?;
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
    deployment::write_new_json(
        &receipt_path,
        &json!({"schema":1,"passed":true,"scope":"offline_dependencies_and_trusted_fixtures","signed_manifest_sha256":manifest.sha256(),"production_isolation_verified":false}),
        &AclPolicy::private_vm(),
    )
}
async fn serve_client(
    server: NamedPipeServer,
    sid: String,
    endpoint: Endpoint,
    _permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<()> {
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
        let reply = endpoint.call(&frame).await?;
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
    let package = deployment.package(&root);
    deployment::verify_running_binary(&package, &manifest)?;
    let binding: Value =
        serde_json::from_slice(&deployment::read_bytes(&root.join("binding.json"), 4096)?)?;
    ensure!(
        binding
            == json!({"schema":1,"protocol":2,"installation_id":deployment.installation_id,"package_sha256":manifest.sha256()}),
        "native worker binding differs from active signed payload"
    );
    offline_selftest(&root, &package, &deployment, &manifest).await?;
    let worker = spawn_worker(&root, &package, &deployment, &manifest)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let endpoint = loop {
        ensure!(
            !worker.exited(),
            "ordinary-account Python worker exited before readiness"
        );
        ensure!(
            !STOP.load(Ordering::Acquire),
            "service stopped during startup"
        );
        if let Ok(endpoint) = Endpoint::read(&root, &worker)
            && let Ok(Ok(reply)) = tokio::time::timeout(
                Duration::from_secs(2),
                endpoint.call(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"ping\"}\n"),
            )
            .await
        {
            let reply: Value = serde_json::from_slice(&reply)?;
            if reply["result"]["daemon_pid"] == worker.pid()
                && reply["result"]["identity"]["package_sha256"] == manifest.sha256()
            {
                break endpoint;
            }
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "ordinary-account Python worker readiness timed out"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
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
                tasks.spawn(serve_client(accepted,sid.clone(),endpoint.clone(),permit));
            },
            _=tokio::time::sleep(Duration::from_millis(250))=>{
                if STOP.load(Ordering::Acquire) {break;}
                ensure!(!worker.exited(),"Python worker epoch exited; all owned descendants will be killed before SCM restart");
                ensure!(!ssh.exited(),"packaged SSH daemon exited");
            },
            _=tasks.join_next(), if !tasks.is_empty()=>{},
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    drop(ssh);
    drop(worker); // Each closes/terminates its entire Job, not just a PID.
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
