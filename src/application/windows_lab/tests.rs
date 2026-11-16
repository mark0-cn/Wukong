use super::*;
use crate::application::windows_lab::admin::{AdminOutcome, AdminRequest, LabAdmin};
use crate::application::windows_lab::runner::{LabWorker, LabWorkerOutcome, LabWorkerRequest};
use anyhow::bail;
use async_trait::async_trait;
use sha2::Digest;
use std::path::Path;
use std::sync::Mutex;

#[derive(Default)]
struct FakeState {
    running: bool,
    snapshots: Vec<String>,
    calls: Vec<String>,
    fail_snapshot: bool,
    delay: Option<Duration>,
}

struct FakeHost {
    state: Mutex<FakeState>,
}

impl FakeHost {
    fn new() -> Self {
        Self {
            state: Mutex::new(FakeState::default()),
        }
    }
    fn with_delay(delay: Duration) -> Self {
        let host = Self::new();
        host.state.lock().unwrap().delay = Some(delay);
        host
    }
    fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
    fn fail_snapshot(&self) {
        self.state.lock().unwrap().fail_snapshot = true;
    }
    fn allow_snapshot(&self) {
        self.state.lock().unwrap().fail_snapshot = false;
    }
    fn set_running(&self, running: bool) {
        self.state.lock().unwrap().running = running;
    }
    async fn wait(&self) {
        let delay = self.state.lock().unwrap().delay;
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
    }
}

#[async_trait]
impl VmHost for FakeHost {
    async fn running(&self) -> Result<bool> {
        self.state.lock().unwrap().calls.push("running".into());
        self.wait().await;
        Ok(self.state.lock().unwrap().running)
    }
    async fn snapshots(&self) -> Result<Vec<String>> {
        self.state.lock().unwrap().calls.push("snapshots".into());
        self.wait().await;
        Ok(self.state.lock().unwrap().snapshots.clone())
    }
    async fn snapshot(&self, name: &str) -> Result<()> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(format!("snapshot {name}"));
        self.wait().await;
        let mut state = self.state.lock().unwrap();
        if state.fail_snapshot {
            bail!("snapshot failed");
        }
        state.snapshots.push(name.to_owned());
        Ok(())
    }
    async fn revert(&self, name: &str) -> Result<()> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(format!("revert {name}"));
        self.wait().await;
        Ok(())
    }
    async fn start(&self) -> Result<()> {
        self.state.lock().unwrap().calls.push("start".into());
        self.wait().await;
        self.state.lock().unwrap().running = true;
        Ok(())
    }
}

fn config(dir: &Path) -> WindowsLabVm {
    WindowsLabVm {
        vmx: dir.join("vm.vmx"),
        vmrun: dir.join("vmrun.exe"),
        environment: None,
        admin: None,
        baseline_snapshot: "baseline".into(),
        snapshot_prefix: "lab".into(),
        operation_timeout_seconds: 30,
        start_after_restore: false,
    }
}

fn spec() -> ExperimentSpec {
    ExperimentSpec {
        target_sha256: "a".repeat(64),
        launch: LaunchKind::Exe,
        input_channel: InputChannel::Stdin,
        account: AccountContext::User,
        os_build: "10.0.17134".into(),
        toolchain_sha256: "b".repeat(64),
        recovery: "revert to baseline".into(),
        dependencies: vec![],
        install: None,
    }
}

fn dependency(dir: &Path, name: &str, bytes: &[u8]) -> package::Dependency {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    package::Dependency {
        name: name.into(),
        sha256: hex::encode(sha2::Sha256::digest(bytes)),
        source: package::DependencySource::Path { path },
    }
}

fn install_plan() -> install::InstallPlan {
    install::InstallPlan {
        targets: vec![r"C:\lab\harness.exe".into()],
        service: None,
        account: None,
        cleanup: vec![install::CleanupAction {
            kind: install::CleanupKind::File,
            target: r"C:\lab\harness.exe".into(),
        }],
    }
}

fn lab(dir: &Path, host: Arc<FakeHost>) -> WindowsLab {
    WindowsLab::open(dir, "labvm", &config(dir), host).unwrap()
}

fn state(dir: &Path) -> LabState {
    LabStore::new(dir).load("labvm", "baseline").unwrap()
}

#[tokio::test]
async fn prepare_creates_baseline_and_experiment_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    let lab = lab(dir.path(), host.clone());
    let lease = lab
        .prepare("analyst", "parser triage", spec())
        .await
        .unwrap();
    assert_eq!(lease.generation, 1);
    let calls = host.calls();
    assert_eq!(calls[0], "snapshots");
    assert_eq!(calls[1], "snapshot baseline");
    assert_eq!(calls[2], format!("snapshot {}", lease.snapshot));
    let state = state(dir.path());
    assert_eq!(state.phase, LabPhase::Ready);
    assert_eq!(state.generation, 1);
    assert_eq!(state.active.unwrap().lab_id, lease.lab_id);
}

#[tokio::test]
async fn prepare_requires_restore_before_another_experiment() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    let lab = lab(dir.path(), host.clone());
    lab.prepare("analyst", "first", spec()).await.unwrap();
    let error = lab.prepare("analyst", "second", spec()).await.unwrap_err();
    assert!(format!("{error:#}").contains("restore or reconcile"));
    assert_eq!(state(dir.path()).phase, LabPhase::Ready);
}

#[tokio::test]
async fn restore_rejects_a_stale_generation_without_touching_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    let lab = lab(dir.path(), host.clone());
    let lease = lab.prepare("analyst", "first", spec()).await.unwrap();
    let before = host.calls().len();
    let error = lab
        .restore(&lease.lab_id, lease.generation - 1)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("stale lab lease"));
    assert_eq!(host.calls().len(), before);
    assert_eq!(state(dir.path()).phase, LabPhase::Ready);
}

#[tokio::test]
async fn restore_fences_the_old_lease_and_returns_to_idle() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    let lab = lab(dir.path(), host.clone());
    let lease = lab.prepare("analyst", "first", spec()).await.unwrap();
    lab.restore(&lease.lab_id, lease.generation).await.unwrap();
    let state = state(dir.path());
    assert_eq!(state.phase, LabPhase::Idle);
    assert_eq!(state.generation, 2);
    assert!(state.active.is_none());
    assert!(host.calls().contains(&"revert baseline".into()));
    let error = lab
        .restore(&lease.lab_id, lease.generation)
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("no active experiment")
            || format!("{error:#}").contains("stale")
    );
}

#[tokio::test]
async fn restore_starts_the_vm_only_when_needed() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    let mut config = config(dir.path());
    config.start_after_restore = true;
    let lab = WindowsLab::open(dir.path(), "labvm", &config, host.clone()).unwrap();
    let lease = lab
        .prepare("analyst", "start needed", spec())
        .await
        .unwrap();
    lab.restore(&lease.lab_id, lease.generation).await.unwrap();
    assert!(host.calls().contains(&"start".into()));

    let lease = lab
        .prepare("analyst", "already running", spec())
        .await
        .unwrap();
    host.set_running(true);
    let before = host.calls().len();
    lab.restore(&lease.lab_id, lease.generation).await.unwrap();
    assert!(!host.calls()[before..].contains(&"start".into()));
}

#[tokio::test]
async fn timeout_marks_unknown_and_blocks_until_reconciled() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::with_delay(Duration::from_secs(2)));
    let mut config = config(dir.path());
    config.operation_timeout_seconds = 1;
    let lab = WindowsLab::open(dir.path(), "labvm", &config, host).unwrap();
    let error = lab.prepare("analyst", "slow", spec()).await.unwrap_err();
    assert!(format!("{error:#}").contains("unknown after timeout"));
    let state = state(dir.path());
    assert_eq!(state.phase, LabPhase::RecoveryRequired);
    let operation = state.pending.clone().unwrap().operation_id;
    assert!(lab.prepare("analyst", "again", spec()).await.is_err());
    let reconciled = lab
        .reconcile(&operation, ReconcileOutcome::Completed)
        .await
        .unwrap();
    assert_eq!(reconciled["state"]["phase"], "ready");
}

#[tokio::test]
async fn authoritative_snapshot_failure_marks_failed_and_allows_retry() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    host.fail_snapshot();
    let lab = lab(dir.path(), host.clone());
    assert!(lab.prepare("analyst", "broken", spec()).await.is_err());
    assert_eq!(state(dir.path()).phase, LabPhase::Failed);
    host.allow_snapshot();
    let lease = lab.prepare("analyst", "retry", spec()).await.unwrap();
    assert_eq!(lease.generation, 2);
}

#[tokio::test]
async fn controller_restart_requires_reconciliation_for_a_pending_effect() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    let lab = lab(dir.path(), host);
    let lease = lab.prepare("analyst", "first", spec()).await.unwrap();
    // Simulate a controller crash after the prepare intent was persisted.
    let store = LabStore::new(dir.path());
    let mut interrupted = state(dir.path());
    interrupted.phase = LabPhase::Preparing;
    interrupted.active = None;
    interrupted.pending = Some(PendingOperation {
        operation_id: "pending-op".into(),
        kind: OperationKind::Prepare,
        generation: lease.generation,
        lab_id: lease.lab_id.clone(),
        owner: "analyst".into(),
        purpose: "first".into(),
        spec_sha256: lease.spec_sha256.clone(),
        spec: spec(),
        package_sha256: None,
        install_sha256: None,
        snapshot: lease.snapshot.clone(),
        started_at: now(),
    });
    store.save(&interrupted).unwrap();
    let status = lab.status().await.unwrap();
    assert_eq!(status["state"]["phase"], "recovery_required");
    assert!(lab.prepare("analyst", "again", spec()).await.is_err());
    assert!(lab.restore(&lease.lab_id, lease.generation).await.is_err());
}

#[tokio::test]
async fn reconcile_rejects_a_different_operation() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::with_delay(Duration::from_secs(2)));
    let mut config = config(dir.path());
    config.operation_timeout_seconds = 1;
    let lab = WindowsLab::open(dir.path(), "labvm", &config, host).unwrap();
    assert!(lab.prepare("analyst", "slow", spec()).await.is_err());
    assert!(
        lab.reconcile("not-the-operation", ReconcileOutcome::Completed)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn prepare_binds_offline_package_and_install_identity() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    let lab = lab(dir.path(), host);
    let mut spec = spec();
    spec.dependencies = vec![dependency(
        dir.path(),
        "harness.c",
        b"int main(void){return 0;}",
    )];
    spec.install = Some(install_plan());
    let lease = lab.prepare("analyst", "package", spec).await.unwrap();
    let package_id = lease.package_sha256.clone().unwrap();
    assert!(
        dir.path()
            .join("windows-lab/packages")
            .join(&package_id)
            .join("harness.c")
            .is_file()
    );
    let state = state(dir.path());
    let active = state.active.unwrap();
    assert_eq!(active.package_sha256, Some(package_id));
    assert!(active.install_sha256.is_some());
    assert!(lease.install_sha256.is_some());
}

#[tokio::test]
async fn invalid_dependency_fails_before_any_host_effect() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    let lab = lab(dir.path(), host.clone());
    let mut spec = spec();
    let mut dependency = dependency(dir.path(), "harness.c", b"source");
    dependency.sha256 = "c".repeat(64);
    spec.dependencies = vec![dependency];
    let error = lab.prepare("analyst", "package", spec).await.unwrap_err();
    assert!(format!("{error:#}").contains("hash mismatch"));
    assert!(host.calls().is_empty());
    assert_eq!(state(dir.path()).phase, LabPhase::Idle);
}

#[tokio::test]
async fn run_executes_a_packaged_exe_and_writes_a_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    let worker = Arc::new(FakeWorker::completed());
    let lab = lab(dir.path(), host).with_worker(worker.clone());
    let mut spec = spec();
    spec.dependencies = vec![dependency(dir.path(), "harness.exe", b"MZ-fake")];
    let lease = lab.prepare("analyst", "run", spec).await.unwrap();
    let result = lab
        .run(
            &lease.lab_id,
            lease.generation,
            "harness.exe",
            Some(b"stdin".to_vec()),
            30,
        )
        .await
        .unwrap();
    assert_eq!(result["run"]["status"], "completed");
    assert_eq!(result["run"]["exit_code"], 0);
    assert_eq!(result["run"]["stdout"], "ok\n");
    let receipt = result["receipt"].as_str().unwrap();
    assert!(Path::new(receipt).is_file());
    let requests = worker.requests.lock().unwrap();
    assert_eq!(requests[0].artifact_name, "harness.exe");
    assert_eq!(requests[0].executable, b"MZ-fake");
    assert_eq!(requests[0].stdin.as_deref(), Some(b"stdin".as_slice()));
}

#[tokio::test]
async fn unknown_run_record_is_reconciled_by_restore() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    let worker = Arc::new(FakeWorker::failing("transport lost"));
    let lab = lab(dir.path(), host).with_worker(worker);
    let mut spec = spec();
    spec.dependencies = vec![dependency(dir.path(), "harness.exe", b"MZ-fake")];
    let lease = lab.prepare("analyst", "run", spec).await.unwrap();
    let error = lab
        .run(&lease.lab_id, lease.generation, "harness.exe", None, 30)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("outcome is unknown"));
    assert_eq!(
        state(dir.path()).last_run.unwrap().status,
        RunStatus::Unknown
    );
    assert!(
        lab.run(&lease.lab_id, lease.generation, "harness.exe", None, 30)
            .await
            .is_err()
    );
    lab.restore(&lease.lab_id, lease.generation).await.unwrap();
    assert_eq!(state(dir.path()).last_run.unwrap().status, RunStatus::Lost);
}

#[tokio::test]
async fn run_rejects_non_exe_launch_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    let lab = lab(dir.path(), host).with_worker(Arc::new(FakeWorker::completed()));
    let mut spec = spec();
    spec.launch = LaunchKind::Service;
    spec.dependencies = vec![dependency(dir.path(), "service.exe", b"MZ-fake")];
    let lease = lab.prepare("analyst", "service", spec).await.unwrap();
    let error = lab
        .run(&lease.lab_id, lease.generation, "service.exe", None, 30)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("only launch=exe"));
}

struct FakeWorker {
    outcome: Mutex<Option<std::result::Result<LabWorkerOutcome, String>>>,
    requests: Mutex<Vec<LabWorkerRequest>>,
}

impl FakeWorker {
    fn completed() -> Self {
        Self {
            outcome: Mutex::new(Some(Ok(LabWorkerOutcome {
                environment_id: "windows".into(),
                installation_id: "installation".into(),
                job_id: "job-1".into(),
                state: "completed".into(),
                settled: true,
                exit_code: Some(0),
                executable_path: Some("files/lab/harness.exe".into()),
                stdout: "ok\n".into(),
                stderr: String::new(),
                stdout_bytes: 3,
                stderr_bytes: 0,
            }))),
            requests: Mutex::new(Vec::new()),
        }
    }
    fn failing(message: &str) -> Self {
        Self {
            outcome: Mutex::new(Some(Err(message.into()))),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LabWorker for FakeWorker {
    async fn execute(&self, request: LabWorkerRequest) -> Result<LabWorkerOutcome> {
        self.requests.lock().unwrap().push(request);
        match self.outcome.lock().unwrap().take().unwrap() {
            Ok(outcome) => Ok(outcome),
            Err(message) => bail!("{message}"),
        }
    }
}

#[tokio::test]
async fn install_runs_verified_media_through_the_administrative_channel() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    let admin = Arc::new(FakeAdmin::default());
    let lab = lab(dir.path(), host).with_admin(admin.clone());
    let mut spec = spec();
    spec.launch = LaunchKind::Service;
    spec.dependencies = vec![
        dependency(dir.path(), "install.ps1", b"Write-Output install"),
        dependency(dir.path(), "app.exe", b"MZ-fake"),
    ];
    spec.install = Some(install::InstallPlan {
        targets: vec![r"C:\lab\app.exe".into()],
        service: Some(install::ServiceInstall {
            name: "wukong-lab".into(),
            binary: r"C:\lab\app.exe".into(),
            kernel_driver: false,
            start: true,
        }),
        account: None,
        cleanup: vec![],
    });
    let lease = lab.prepare("analyst", "install", spec).await.unwrap();
    let result = lab
        .install(&lease.lab_id, lease.generation, 60)
        .await
        .unwrap();
    assert_eq!(result["install"]["status"], "completed");
    assert_eq!(result["install"]["exit_code"], 0);
    assert!(Path::new(result["receipt"].as_str().unwrap()).is_file());
    let requests = admin.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].media_sha256,
        lease.package_sha256.clone().unwrap()
    );
    assert!(
        requests[0]
            .files
            .iter()
            .any(|file| file.name == "install.ps1")
    );
    assert!(requests[0].files.iter().all(|file| file.path.is_file()));
}

#[tokio::test]
async fn install_requires_an_administrative_channel() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new());
    let lab = lab(dir.path(), host);
    let mut spec = spec();
    spec.launch = LaunchKind::Service;
    spec.dependencies = vec![dependency(dir.path(), "app.exe", b"MZ-fake")];
    spec.install = Some(install::InstallPlan {
        targets: vec![r"C:\lab\app.exe".into()],
        service: None,
        account: None,
        cleanup: vec![],
    });
    let lease = lab.prepare("analyst", "install", spec).await.unwrap();
    let error = lab
        .install(&lease.lab_id, lease.generation, 60)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("administrative channel is unavailable"));
}

#[tokio::test]
async fn install_requires_a_packaged_install_script() {
    let dir = tempfile::tempdir().unwrap();
    let admin = Arc::new(FakeAdmin::default());
    let lab = lab_with_admin(dir.path(), admin);
    let mut spec = spec();
    spec.launch = LaunchKind::Service;
    spec.dependencies = vec![dependency(dir.path(), "app.exe", b"MZ-fake")];
    spec.install = Some(install::InstallPlan {
        targets: vec![r"C:\lab\app.exe".into()],
        service: None,
        account: None,
        cleanup: vec![],
    });
    let lease = lab.prepare("analyst", "install", spec).await.unwrap();
    let error = lab
        .install(&lease.lab_id, lease.generation, 60)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("install.ps1"));
}

fn lab_with_admin(dir: &Path, admin: Arc<FakeAdmin>) -> WindowsLab {
    WindowsLab::open(dir, "labvm", &config(dir), Arc::new(FakeHost::new()))
        .unwrap()
        .with_admin(admin)
}

struct FakeAdmin {
    requests: Mutex<Vec<AdminRequest>>,
}

impl Default for FakeAdmin {
    fn default() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LabAdmin for FakeAdmin {
    async fn install(&self, request: AdminRequest) -> Result<AdminOutcome> {
        self.requests.lock().unwrap().push(request);
        Ok(AdminOutcome {
            exit_code: Some(0),
            stdout: "installed\n".into(),
            stderr: String::new(),
            guest_media: r"C:\Windows\Temp\wukong-lab.zip".into(),
            guest_script: r"C:\Windows\Temp\wukong-lab\install.ps1".into(),
            settled: true,
        })
    }
}

#[tokio::test]
async fn experiment_spec_rejects_invalid_identity_fields() {
    let mut invalid = spec();
    invalid.target_sha256 = "A".repeat(63);
    assert!(invalid.validate().is_err());
    invalid.target_sha256 = "a".repeat(64);
    invalid.os_build = String::new();
    assert!(invalid.validate().is_err());
    invalid.os_build = "10.0.17134".into();
    invalid.recovery = "x".repeat(513);
    assert!(invalid.validate().is_err());
}
