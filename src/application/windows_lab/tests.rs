use super::*;
use anyhow::bail;
use async_trait::async_trait;
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
