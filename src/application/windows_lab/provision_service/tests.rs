//! `lab provision` state-machine tests with fake host, offline disk and ISO
//! adapters. No test executes vmrun, WSL, libguestfs or a real VM.
use super::*;
use crate::application::windows_lab::MediaIdentity;
use crate::application::windows_lab::provision::GuestFile;
use crate::application::windows_lab::provision::fixture;
use crate::application::windows_lab::provision::{
    DiskInspection, DiskInspectionRequest, FinalizeRequest, GuestNetwork, GuestObservation,
    GuestObservationRequest, GuestPaths, HivePatch, HivePatchEvidence, IsoBuilder, MediaFinalizer,
    OfflineDisk, ProvisionEnvironment, PublicConnection,
};
use crate::application::windows_lab::types::{
    LabPhase, LabProvision, LabState, OperationKind, PendingOperation, ProvisionSource,
    ProvisioningSpec, RunStatus,
};
use crate::application::windows_lab::{LabStore, VmHost};
use crate::config::WindowsLabVm;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

type Events = Arc<Mutex<Vec<String>>>;

#[derive(Default)]
struct HostState {
    running: bool,
    snapshots: Vec<String>,
    calls: Vec<String>,
    boot_polls: u32,
    polls_remaining: u32,
    boot_started: bool,
    fail_running: bool,
    fail_start: bool,
    fail_snapshot: bool,
    fail_revert: bool,
    fail_delete: bool,
    /// Emulated VMware snapshot commit: rewrite the VMX when the snapshot goes.
    rewrite_vmx_on_delete: Option<(PathBuf, Vec<u8>)>,
    state_dir: Option<PathBuf>,
    delete_observation: Option<DeleteObservation>,
}

/// What the controller state and provision directory looked like when the
/// configured baseline snapshot was deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DeleteObservation {
    vmx_backups: usize,
    phase: Option<String>,
    pending_provision: bool,
}

struct FakeHost {
    state: Mutex<HostState>,
    events: Events,
}

impl FakeHost {
    fn new() -> Self {
        Self {
            state: Mutex::new(HostState::default()),
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
    fn set_running(&self, running: bool) {
        self.state.lock().unwrap().running = running;
    }
    fn add_snapshot(&self, name: &str) {
        self.state.lock().unwrap().snapshots.push(name.into());
    }
    fn set_boot_polls(&self, polls: u32) {
        self.state.lock().unwrap().boot_polls = polls;
    }
    fn events(&self) -> Events {
        self.events.clone()
    }
    fn record(&self, event: impl Into<String>) {
        self.events.lock().unwrap().push(event.into());
    }
    fn rewrite_vmx_on_delete(&self, vmx: &Path, bytes: Vec<u8>) {
        self.state.lock().unwrap().rewrite_vmx_on_delete = Some((vmx.to_path_buf(), bytes));
    }
    fn watch_state(&self, state_dir: &Path) {
        self.state.lock().unwrap().state_dir = Some(state_dir.to_path_buf());
    }
    fn delete_observation(&self) -> Option<DeleteObservation> {
        self.state.lock().unwrap().delete_observation.clone()
    }

    /// Record what a controller crash right after the snapshot deletion would
    /// leave behind: the backed-up VMX file and the persisted provisioning
    /// intent.
    fn observe_delete(&self, state: &mut HostState) {
        let Some(directory) = state.state_dir.clone() else {
            return;
        };
        let provisions = directory.join("windows-lab").join("provisions");
        let vmx_backups = fs::read_dir(&provisions)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().join("vmx.bak").is_file())
                    .count()
            })
            .unwrap_or_default();
        let (phase, pending_provision) = fs::read(directory.join("windows-lab").join("labvm.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .map(|value| {
                (
                    value["phase"].as_str().map(str::to_owned),
                    value["pending"]["kind"].as_str() == Some("provision"),
                )
            })
            .unwrap_or((None, false));
        state.delete_observation = Some(DeleteObservation {
            vmx_backups,
            phase,
            pending_provision,
        });
    }
}

#[async_trait]
impl VmHost for FakeHost {
    async fn running(&self) -> Result<bool> {
        self.record("host:running");
        let mut state = self.state.lock().unwrap();
        state.calls.push("running".into());
        if state.fail_running {
            bail!("vmrun list failed");
        }
        if state.boot_started {
            if state.polls_remaining > 0 {
                state.polls_remaining -= 1;
                return Ok(true);
            }
            state.boot_started = false;
            state.running = false;
            return Ok(false);
        }
        Ok(state.running)
    }

    async fn snapshots(&self) -> Result<Vec<String>> {
        self.record("host:snapshots");
        self.state.lock().unwrap().calls.push("snapshots".into());
        Ok(self.state.lock().unwrap().snapshots.clone())
    }

    async fn snapshot(&self, name: &str) -> Result<()> {
        self.record(format!("host:snapshot {name}"));
        let mut state = self.state.lock().unwrap();
        state.calls.push(format!("snapshot {name}"));
        if state.fail_snapshot {
            bail!("snapshot failed");
        }
        state.snapshots.push(name.into());
        Ok(())
    }

    async fn revert(&self, name: &str) -> Result<()> {
        self.record(format!("host:revert {name}"));
        let mut state = self.state.lock().unwrap();
        state.calls.push(format!("revert {name}"));
        if state.fail_revert {
            bail!("revert failed");
        }
        state.running = false;
        Ok(())
    }

    async fn start(&self) -> Result<()> {
        self.record("host:start");
        let mut state = self.state.lock().unwrap();
        state.calls.push("start".into());
        if state.fail_start {
            bail!("start failed");
        }
        state.running = true;
        state.boot_started = true;
        state.polls_remaining = state.boot_polls;
        Ok(())
    }

    async fn delete_snapshot(&self, name: &str) -> Result<()> {
        self.record(format!("host:delete {name}"));
        let mut state = self.state.lock().unwrap();
        state.calls.push(format!("delete {name}"));
        if state.fail_delete {
            bail!("delete snapshot failed");
        }
        self.observe_delete(&mut state);
        // A real VMware snapshot commit rewrites the VMX away from the delta.
        if let Some((vmx, bytes)) = state.rewrite_vmx_on_delete.clone() {
            fs::write(vmx, bytes)?;
        }
        state.snapshots.retain(|snapshot| snapshot != name);
        Ok(())
    }
}

#[derive(Default)]
struct DiskState {
    calls: Vec<String>,
    written: Vec<GuestFile>,
    /// Guest paths converted exactly as the WSL adapter would convert them.
    guest_paths: Vec<String>,
    inspection: Option<DiskInspection>,
    tamper: Option<Tampered>,
}

#[derive(Clone, Copy)]
enum Tampered {
    ReceiptInstallation,
    ServiceLeftBehind,
}

struct FakeDisk {
    state: Mutex<DiskState>,
    connection: PublicConnection,
    events: Option<Events>,
}

fn guest_path(windows: &str) -> Result<String> {
    GuestPaths::guestfish(windows)
        .context("offline disk requests must carry absolute Windows guest paths")
}

impl FakeDisk {
    fn new(connection: PublicConnection) -> Self {
        Self {
            state: Mutex::new(DiskState {
                inspection: Some(DiskInspection {
                    windows_volume: true,
                    system_hive_present: true,
                    program_data_present: true,
                    control_set: Some(1),
                    existing_service: None,
                    provision_root_entries: vec![],
                }),
                ..Default::default()
            }),
            connection,
            events: None,
        }
    }
    fn with_events(mut self, events: Events) -> Self {
        self.events = Some(events);
        self
    }
    fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
    fn guest_paths(&self) -> Vec<String> {
        self.state.lock().unwrap().guest_paths.clone()
    }
    fn record(&self, event: &str) {
        if let Some(events) = &self.events {
            events.lock().unwrap().push(format!("disk:{event}"));
        }
    }
    fn tamper(&self, tamper: Tampered) {
        self.state.lock().unwrap().tamper = Some(tamper);
    }
    fn written(&self) -> Vec<GuestFile> {
        self.state
            .lock()
            .unwrap()
            .written
            .iter()
            .map(|file| GuestFile {
                windows_path: file.windows_path.clone(),
                bytes: file.bytes.clone(),
                sha256: file.sha256.clone(),
            })
            .collect()
    }
}

fn plan_json(written: &[GuestFile]) -> serde_json::Value {
    let plan = written
        .iter()
        .find(|file| file.windows_path.ends_with("provision-plan.json"))
        .expect("provision plan was injected");
    serde_json::from_slice(&plan.bytes).unwrap()
}

fn image_path_from_script(script: &str) -> String {
    script
        .lines()
        .find_map(|line| line.strip_prefix("expandstring:"))
        .expect("hive script has a service image path")
        .to_owned()
}

#[async_trait]
impl OfflineDisk for FakeDisk {
    async fn inspect(
        &self,
        _disk: &Path,
        request: &DiskInspectionRequest,
    ) -> Result<DiskInspection> {
        self.record("inspect");
        let mut state = self.state.lock().unwrap();
        state.calls.push("inspect".into());
        for windows in [
            &request.system_hive,
            &request.program_data,
            &request.provision_root,
        ] {
            let guest = guest_path(windows)?;
            state.guest_paths.push(guest);
        }
        Ok(state.inspection.clone().expect("inspection configured"))
    }

    async fn write_guest_files(&self, _disk: &Path, files: &[GuestFile]) -> Result<()> {
        self.record("write");
        let mut state = self.state.lock().unwrap();
        state.calls.push("write".into());
        for file in files {
            let guest = guest_path(&file.windows_path)?;
            state.guest_paths.push(guest);
        }
        state.written.extend(files.iter().map(|file| GuestFile {
            windows_path: file.windows_path.clone(),
            bytes: file.bytes.clone(),
            sha256: file.sha256.clone(),
        }));
        Ok(())
    }

    async fn patch_system_hive(
        &self,
        _disk: &Path,
        patch: &HivePatch,
    ) -> Result<HivePatchEvidence> {
        self.record("patch");
        let mut state = self.state.lock().unwrap();
        state.calls.push("patch".into());
        state.guest_paths.push(guest_path(&patch.system_hive)?);
        let image = image_path_from_script(&patch.script);
        let output = format!(
            "\"Type\"=dword:00000010\n\"Start\"=dword:00000002\n\"ErrorControl\"=dword:00000001\n\"ImagePath\"=str(2):\"{}\"\n\"ObjectName\"=\"LocalSystem\"\n",
            image.replace('\\', "\\\\")
        );
        Ok(HivePatchEvidence {
            hive_sha256_before: "a".repeat(64),
            hive_sha256_after: "b".repeat(64),
            verify_output: output,
        })
    }

    async fn observe_guest(
        &self,
        _disk: &Path,
        request: &GuestObservationRequest,
    ) -> Result<GuestObservation> {
        self.record("observe");
        {
            let mut state = self.state.lock().unwrap();
            state.calls.push("observe".into());
            for windows in [
                &request.system_hive,
                &request.wrapper,
                &request.plan,
                &request.receipt,
            ] {
                let guest = guest_path(windows)?;
                state.guest_paths.push(guest);
            }
        }
        let written = self.written();
        let plan = plan_json(&written);
        let mut connection = self.connection.clone();
        connection.installation_id = plan["installation_id"].as_str().unwrap().into();
        connection.environment_id = plan["environment_id"].as_str().unwrap().into();
        connection.generation = plan["generation"].as_u64().unwrap();
        connection.manifest_sha256 = plan["manifest_sha256"].as_str().unwrap().into();
        connection.ssh_port = u16::try_from(plan["ssh_port"].as_u64().unwrap()).unwrap();
        if matches!(
            self.state.lock().unwrap().tamper,
            Some(Tampered::ReceiptInstallation)
        ) {
            connection.installation_id = "0".repeat(32);
        }
        let receipt = fixture::guest_receipt(plan["provision_id"].as_str().unwrap(), &connection);
        Ok(GuestObservation {
            receipt: Some(receipt),
            service_present: matches!(
                self.state.lock().unwrap().tamper,
                Some(Tampered::ServiceLeftBehind)
            ),
            wrapper_present: false,
            plan_present: false,
        })
    }
}

#[derive(Default)]
struct IsoState {
    builds: Vec<(PathBuf, PathBuf)>,
    calls: Vec<String>,
}

struct FakeIso {
    state: Mutex<IsoState>,
}

impl FakeIso {
    fn new() -> Self {
        Self {
            state: Mutex::new(IsoState::default()),
        }
    }
}

#[async_trait]
impl IsoBuilder for FakeIso {
    fn with_executable(&self, _executable: &Path) -> Result<Arc<dyn IsoBuilder>> {
        self.state.lock().unwrap().calls.push("override".into());
        Ok(Arc::new(FakeIso::new()))
    }

    async fn build(&self, media: &Path, output: &Path) -> Result<()> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(format!("build {}", output.display()));
        fs::write(output, b"offline-iso")?;
        self.state
            .lock()
            .unwrap()
            .builds
            .push((media.to_path_buf(), output.to_path_buf()));
        Ok(())
    }
}

struct FakeFinalizer {
    source_root: PathBuf,
    identity_algorithm: String,
    identity_key: String,
}

#[async_trait]
impl MediaFinalizer for FakeFinalizer {
    async fn finalize(&self, request: &FinalizeRequest) -> Result<serde_json::Value> {
        copy_tree(&self.source_root, &request.output)?;
        if let Some(key) = &request.generate_ssh_key {
            fs::write(key, "CONTROLLED-PRIVATE-KEY-FIXTURE")?;
            fs::write(
                format!("{}.pub", key.display()),
                format!(
                    "{} {} fixture\n",
                    self.identity_algorithm, self.identity_key
                ),
            )?;
        }
        Ok(serde_json::json!({"record": true}))
    }
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            copy_tree(&path, &target.join(entry.file_name()))?;
        } else {
            fs::copy(&path, target.join(entry.file_name()))?;
        }
    }
    Ok(())
}

struct FakeNetwork;

#[async_trait]
impl GuestNetwork for FakeNetwork {
    async fn guest_ip(&self) -> Result<String> {
        Ok("192.168.199.129".into())
    }
}

#[derive(Default)]
struct EnvironmentState {
    registrations: Vec<(String, PathBuf)>,
    doctors: usize,
}

struct FakeEnvironment {
    state: Mutex<EnvironmentState>,
}

impl FakeEnvironment {
    fn new() -> Self {
        Self {
            state: Mutex::new(EnvironmentState::default()),
        }
    }
    fn registrations(&self) -> usize {
        self.state.lock().unwrap().registrations.len()
    }
}

#[async_trait]
impl ProvisionEnvironment for FakeEnvironment {
    fn register(
        &self,
        _connection: &PublicConnection,
        _media: &MediaIdentity,
        guest_ip: &str,
        identity_file: &Path,
    ) -> Result<serde_json::Value> {
        self.state
            .lock()
            .unwrap()
            .registrations
            .push((guest_ip.into(), identity_file.to_path_buf()));
        Ok(serde_json::json!({"environment_id": "windows-vm"}))
    }

    async fn verify(&self, environment_id: &str) -> Result<serde_json::Value> {
        self.state.lock().unwrap().doctors += 1;
        Ok(serde_json::json!({"environment_id": environment_id, "observation": {"os": "windows"}}))
    }
}

struct Deployment {
    directory: tempfile::TempDir,
    state_dir: PathBuf,
    vmx: PathBuf,
    vmx_initial: Vec<u8>,
    disk: PathBuf,
    host: Arc<FakeHost>,
    fixture: fixture::MediaFixture,
}

fn deployment() -> Deployment {
    let directory = tempfile::tempdir().unwrap();
    let state_dir = directory.path().join("state");
    let vmx = directory.path().join("lab.vmx");
    let disk = directory.path().join("Windows 10 x64.vmdk");
    fs::write(&disk, b"fixture disk").unwrap();
    let vmx_initial = b"config.version = \"8\"\nsata0.present = \"TRUE\"\nnvme0.present = \"TRUE\"\nnvme0:0.fileName = \"Windows 10 x64.vmdk\"\nnvme0:0.present = \"TRUE\"\n".to_vec();
    fs::write(&vmx, &vmx_initial).unwrap();
    Deployment {
        directory,
        state_dir,
        vmx,
        vmx_initial,
        disk,
        host: Arc::new(FakeHost::new()),
        fixture: fixture::media(),
    }
}

/// A powered-off deployment whose VM currently sits at the configured baseline
/// snapshot, so its boot disk is the snapshot delta. `committed` emulates the
/// VMware snapshot commit that rewrites the VMX to the base disk.
fn replacement_deployment(committed: bool) -> Deployment {
    let deployment = deployment();
    let base_vmx = deployment.vmx_initial.clone();
    let delta = deployment
        .directory
        .path()
        .join("Windows 10 x64-000001.vmdk");
    fs::write(&delta, b"fixture delta disk").unwrap();
    let delta_vmx = String::from_utf8(base_vmx.clone())
        .unwrap()
        .replace("Windows 10 x64.vmdk", "Windows 10 x64-000001.vmdk")
        .into_bytes();
    fs::write(&deployment.vmx, &delta_vmx).unwrap();
    deployment.host.add_snapshot("baseline");
    deployment.host.watch_state(&deployment.state_dir);
    if committed {
        deployment
            .host
            .rewrite_vmx_on_delete(&deployment.vmx, base_vmx);
    }
    deployment
}

impl Deployment {
    fn config(&self) -> WindowsLabVm {
        WindowsLabVm {
            vmx: self.vmx.clone(),
            vmrun: self.directory.path().join("vmrun.exe"),
            offline: None,
            baseline_snapshot: "baseline".into(),
            snapshot_prefix: "lab".into(),
            operation_timeout_seconds: 30,
            ..Default::default()
        }
    }

    fn store(&self) -> LabStore {
        LabStore::new(&self.state_dir)
    }

    fn state(&self) -> LabState {
        self.store().load("labvm", "baseline").unwrap()
    }

    /// Bind the same configuration identity `WindowsLab::open` computes, so a
    /// hand-written state fixture is accepted by the store.
    fn with_configuration(&self, mut state: LabState) -> LabState {
        let physical =
            crate::adapters::environments::lab_guard::physical_identity(&self.vmx).unwrap();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "physical_vm": physical,
            "config": self.config(),
            "environment": serde_json::Value::Null,
        }))
        .unwrap();
        state.configuration_sha256 = super::super::provision::sha256(&bytes);
        state
    }

    fn lab(&self) -> WindowsLab {
        WindowsLab::open(&self.state_dir, "labvm", &self.config(), self.host.clone()).unwrap()
    }

    fn media_source(&self) -> ProvisionSource {
        ProvisionSource::Media {
            directory: self.fixture.root.clone(),
        }
    }

    fn options(&self, source: ProvisionSource) -> ProvisionOptions {
        ProvisionOptions {
            source,
            iso_tool: None,
            replace_baseline: false,
            register: false,
            verify_environment: false,
            dry_run: false,
            timeout_seconds: 120,
        }
    }

    fn provision_directories(&self) -> Vec<PathBuf> {
        let root = self.state_dir.join("windows-lab/provisions");
        let mut directories = Vec::new();
        if root.is_dir() {
            for entry in fs::read_dir(root).unwrap() {
                let entry = entry.unwrap();
                if entry.path().is_dir() {
                    directories.push(entry.path());
                }
            }
        }
        directories
    }
}

fn wired(deployment: &Deployment, disk: Arc<FakeDisk>, iso: Arc<FakeIso>) -> WindowsLab {
    deployment
        .lab()
        .with_offline(disk)
        .with_iso_builder(iso)
        .with_media_finalizer(Arc::new(FakeFinalizer {
            source_root: deployment.fixture.root.clone(),
            identity_algorithm: deployment.fixture.identity.ssh_public_key.algorithm.clone(),
            identity_key: deployment.fixture.identity.ssh_public_key.key.clone(),
        }))
        .with_guest_network(Arc::new(FakeNetwork))
}

#[tokio::test]
async fn provision_injects_media_and_verifies_the_guest_receipt() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    deployment.host.set_boot_polls(1);
    let lab = wired(&deployment, disk.clone(), iso.clone());
    let outcome = lab
        .provision(deployment.options(deployment.media_source()))
        .await
        .unwrap();
    assert!(!outcome.dry_run);
    let record = outcome.provision.as_ref().unwrap();
    assert_eq!(record.status, RunStatus::Completed);
    assert_eq!(
        record.media.manifest_sha256,
        deployment.fixture.identity.manifest_sha256
    );
    assert!(outcome.receipt.as_ref().unwrap().is_file());
    assert!(outcome.environment_block.is_some());
    let calls = deployment.host.calls();
    assert!(calls.contains(&"start".to_string()));
    assert!(calls.contains(&"snapshot baseline".to_string()));
    let disk_calls = disk.calls();
    assert_eq!(
        disk_calls,
        ["inspect", "write", "patch", "observe"].map(str::to_string)
    );
    assert_eq!(
        fs::read(&deployment.vmx).unwrap(),
        deployment.vmx_initial,
        "VMX must be restored byte for byte"
    );
    let directories = deployment.provision_directories();
    assert_eq!(directories.len(), 1);
    assert!(!directories[0].join("offline.iso").exists());
    assert!(!directories[0].join("vmx.bak").exists());
    assert!(directories[0].join("guest-receipt.json").is_file());
    assert!(directories[0].join("injected/provision.ps1").is_file());
    let state = deployment.state();
    assert_eq!(state.phase, LabPhase::Idle);
    assert_eq!(state.last_provision.unwrap().status, RunStatus::Completed);
}

#[tokio::test]
async fn provision_requires_powered_off_and_tooling() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let lab = deployment.lab();
    let error = lab
        .provision(deployment.options(deployment.media_source()))
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("offline provisioning tooling is not configured"),
        "{error:#}"
    );

    deployment.host.set_running(true);
    let lab = wired(&deployment, disk.clone(), iso);
    let error = lab
        .provision(deployment.options(deployment.media_source()))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("powered off"), "{error:#}");
    assert!(disk.calls().is_empty());
    assert!(deployment.state().last_provision.is_none());
}

#[tokio::test]
async fn provision_refuses_snapshot_chains_but_replaces_only_the_baseline() {
    let first = deployment();
    let disk = Arc::new(FakeDisk::new(first.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&first, disk.clone(), iso.clone());
    first.host.add_snapshot("baseline");
    let error = lab
        .provision(first.options(first.media_source()))
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("replace-baseline"),
        "{error:#}"
    );

    let mut options = first.options(first.media_source());
    options.replace_baseline = true;
    first.host.add_snapshot("unrelated");
    let error = lab.provision(options).await.unwrap_err();
    assert!(format!("{error:#}").contains("snapshot"), "{error:#}");

    let replacement = deployment();
    let disk = Arc::new(FakeDisk::new(replacement.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&replacement, disk, iso);
    replacement.host.add_snapshot("baseline");
    let mut options = replacement.options(replacement.media_source());
    options.replace_baseline = true;
    lab.provision(options).await.unwrap();
    let calls = replacement.host.calls();
    assert!(calls.contains(&"delete baseline".to_string()));
    assert!(
        calls
            .iter()
            .filter(|call| *call == "snapshot baseline")
            .count()
            == 1
    );
    assert!(
        replacement
            .host
            .state
            .lock()
            .unwrap()
            .snapshots
            .contains(&"baseline".to_string())
    );
}

#[tokio::test]
async fn offline_disk_requests_use_canonical_windows_guest_paths() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk.clone(), iso);
    lab.provision(deployment.options(deployment.media_source()))
        .await
        .unwrap();
    let paths = disk.guest_paths();
    assert!(
        paths.contains(&"/Windows/System32/config/SYSTEM".to_string()),
        "{paths:?}"
    );
    assert!(paths.contains(&"/ProgramData".to_string()), "{paths:?}");
    assert!(
        paths.contains(&"/ProgramData/WukongLab".to_string()),
        "{paths:?}"
    );
    assert!(
        paths.iter().any(|path| path.ends_with("/provision.ps1")),
        "{paths:?}"
    );
    assert!(
        paths
            .iter()
            .any(|path| path.ends_with("/provision-plan.json")),
        "{paths:?}"
    );
    assert!(
        paths.iter().any(|path| path.ends_with("/receipt.json")),
        "{paths:?}"
    );
    // Requests carry Windows guest paths, so the adapter sees only the
    // canonical libguestfs form after its single conversion.
    assert!(
        paths
            .iter()
            .all(|path| path.starts_with('/') && !path.contains('\\')),
        "{paths:?}"
    );
}

#[tokio::test]
async fn replace_baseline_is_deleted_before_disk_validation_and_recreated() {
    let deployment = replacement_deployment(true);
    let delta_vmx = fs::read(&deployment.vmx).unwrap();
    assert_ne!(
        delta_vmx, deployment.vmx_initial,
        "the VM starts at the delta"
    );
    let disk = Arc::new(
        FakeDisk::new(deployment.fixture.connection.clone()).with_events(deployment.host.events()),
    );
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk, iso);
    let mut options = deployment.options(deployment.media_source());
    options.replace_baseline = true;
    let outcome = lab.provision(options).await.unwrap();
    assert_eq!(
        outcome.provision.as_ref().unwrap().status,
        RunStatus::Completed
    );

    let events = deployment.host.events();
    let events = events.lock().unwrap();
    let delete = events
        .iter()
        .position(|event| event == "host:delete baseline")
        .unwrap_or_else(|| panic!("baseline was not deleted: {events:?}"));
    let inspect = events
        .iter()
        .position(|event| event == "disk:inspect")
        .unwrap_or_else(|| panic!("disk was not inspected: {events:?}"));
    assert!(
        delete < inspect,
        "baseline deletion must precede disk resolution: {events:?}"
    );
    drop(events);

    // The VMX backup and the persisted intent existed when the baseline was
    // deleted, and no other snapshot was touched.
    let observation = deployment
        .host
        .delete_observation()
        .expect("snapshot deletion was observed");
    assert_eq!(
        observation,
        DeleteObservation {
            vmx_backups: 1,
            phase: Some("provisioning".into()),
            pending_provision: true,
        }
    );
    assert_eq!(
        deployment
            .host
            .calls()
            .iter()
            .filter(|call| *call == "snapshot baseline")
            .count(),
        1
    );
    assert!(
        deployment
            .host
            .state
            .lock()
            .unwrap()
            .snapshots
            .contains(&"baseline".to_string())
    );
    assert_eq!(
        fs::read(&deployment.vmx).unwrap(),
        deployment.vmx_initial,
        "the committed VMX is restored byte for byte"
    );
    let state = deployment.state();
    assert_eq!(state.phase, LabPhase::Idle);
    assert!(state.pending.is_none());
    let record = state.last_provision.expect("provision record");
    assert_eq!(record.status, RunStatus::Completed);
    assert_eq!(record.baseline_snapshot.as_deref(), Some("baseline"));
}

#[tokio::test]
async fn replace_baseline_refuses_extra_snapshots_without_deleting_anything() {
    let deployment = deployment();
    deployment.host.add_snapshot("baseline");
    deployment.host.add_snapshot("lab-unexpected");
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk.clone(), iso);
    let mut options = deployment.options(deployment.media_source());
    options.replace_baseline = true;
    let error = lab.provision(options).await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("snapshot"), "{message}");
    assert!(
        !deployment
            .host
            .calls()
            .iter()
            .any(|call| call.starts_with("delete")),
        "{:?}",
        deployment.host.calls()
    );
    assert!(disk.calls().is_empty());
    assert!(deployment.provision_directories().is_empty());
    assert!(deployment.state().last_provision.is_none());
}

#[tokio::test]
async fn failed_validation_after_baseline_deletion_requires_recovery() {
    // The snapshot is gone but the VMX still points at the delta, so the boot
    // disk cannot be resolved after the forced deletion.
    let deployment = replacement_deployment(false);
    let delta_vmx = fs::read(&deployment.vmx).unwrap();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk.clone(), iso);
    let mut options = deployment.options(deployment.media_source());
    options.replace_baseline = true;
    let error = lab.provision(options).await.unwrap_err();
    assert_eq!(
        error.downcast_ref::<LabError>().unwrap().code,
        "lab_provision_unknown"
    );
    assert!(
        deployment
            .host
            .calls()
            .contains(&"delete baseline".to_string())
    );
    assert!(!deployment.host.calls().contains(&"start".to_string()));
    assert!(disk.calls().is_empty());
    assert!(deployment.host.state.lock().unwrap().snapshots.is_empty());

    let state = deployment.state();
    assert_eq!(state.phase, LabPhase::RecoveryRequired);
    let pending = state
        .pending
        .clone()
        .expect("provision intent stays pending");
    assert_eq!(pending.kind, OperationKind::Provision);
    let spec = pending.provision.expect("provision spec");
    assert!(spec.replaces_baseline);
    assert!(spec.guest_disk.is_none());
    assert_eq!(
        state.last_provision.as_ref().unwrap().status,
        RunStatus::Unknown
    );
    let message = state.last_error.clone().expect("actionable error");
    assert!(message.contains("deleted"), "{message}");
    assert!(
        message.contains(&spec.vmx_backup.display().to_string()),
        "{message}"
    );

    let directories = deployment.provision_directories();
    assert_eq!(directories.len(), 1);
    assert_eq!(
        fs::read(directories[0].join("vmx.bak")).unwrap(),
        delta_vmx,
        "the VMX backup is retained for reconciliation"
    );
}

#[tokio::test]
async fn replace_baseline_without_a_snapshot_is_a_noop() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk, iso);
    let mut options = deployment.options(deployment.media_source());
    options.replace_baseline = true;
    let outcome = lab.provision(options).await.unwrap();
    assert_eq!(outcome.provision.unwrap().status, RunStatus::Completed);
    let calls = deployment.host.calls();
    assert!(
        !calls.iter().any(|call| call.starts_with("delete")),
        "{calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| *call == "snapshot baseline")
            .count(),
        1,
        "the pristine flow creates the baseline after provisioning"
    );
    assert!(
        deployment
            .host
            .state
            .lock()
            .unwrap()
            .snapshots
            .contains(&"baseline".to_string())
    );
    assert_eq!(deployment.state().phase, LabPhase::Idle);
    assert_eq!(fs::read(&deployment.vmx).unwrap(), deployment.vmx_initial);
}

#[tokio::test]
async fn replace_baseline_dry_run_plans_without_deleting_or_inspecting_the_disk() {
    let deployment = replacement_deployment(true);
    let delta_vmx = fs::read(&deployment.vmx).unwrap();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk.clone(), iso.clone());
    let mut options = deployment.options(deployment.media_source());
    options.replace_baseline = true;
    options.dry_run = true;
    let outcome = lab.provision(options).await.unwrap();
    assert!(outcome.dry_run);
    let planned = outcome.planned_actions.join("\n");
    assert!(
        planned.contains("delete the configured baseline snapshot 'baseline'"),
        "{planned}"
    );
    assert!(planned.contains("after the baseline deletion"), "{planned}");
    assert!(
        planned.contains("create the baseline snapshot 'baseline'"),
        "{planned}"
    );
    assert_eq!(fs::read(&deployment.vmx).unwrap(), delta_vmx);
    assert!(
        deployment
            .host
            .state
            .lock()
            .unwrap()
            .snapshots
            .contains(&"baseline".to_string())
    );
    assert!(
        !deployment
            .host
            .calls()
            .iter()
            .any(|call| call.starts_with("delete")),
        "{:?}",
        deployment.host.calls()
    );
    assert!(disk.calls().is_empty());
    assert!(iso.state.lock().unwrap().builds.is_empty());
    assert!(deployment.provision_directories().is_empty());
    assert!(deployment.state().last_provision.is_none());
}

#[tokio::test]
async fn dry_run_plans_without_mutating() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk.clone(), iso.clone());
    let mut options = deployment.options(deployment.media_source());
    options.dry_run = true;
    let outcome = lab.provision(options).await.unwrap();
    assert!(outcome.dry_run);
    assert!(outcome.planned_actions.len() >= 5);
    assert!(outcome.provision.is_none());
    assert_eq!(fs::read(&deployment.vmx).unwrap(), deployment.vmx_initial);
    assert!(deployment.provision_directories().is_empty());
    assert!(deployment.state().last_provision.is_none());
    assert_eq!(disk.calls(), vec!["inspect".to_string()]);
    assert!(iso.state.lock().unwrap().builds.is_empty());
}

#[tokio::test]
async fn template_dry_run_validates_without_finalizing_media() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk.clone(), iso.clone());
    let directory = deployment.directory.path();
    let template = directory.join("template");
    fs::create_dir_all(&template).unwrap();
    let keys = directory.join("keys");
    fs::create_dir_all(&keys).unwrap();
    let spec = directory.join("spec.json");
    fs::write(
        &spec,
        serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "installation_id": deployment.fixture.identity.installation_id,
            "environment_id": deployment.fixture.identity.environment_id,
            "management_peer": "192.168.199.1",
            "ssh_port": 22268,
        }))
        .unwrap(),
    )
    .unwrap();
    let mut options = deployment.options(ProvisionSource::Template {
        template,
        keys,
        spec,
    });
    options.dry_run = true;
    let outcome = lab.provision(options).await.unwrap();
    assert!(outcome.dry_run);
    let planned = outcome.planned_actions.join("\n");
    assert!(planned.contains("scripts.worker.media finalize"));
    assert!(planned.contains("--generate-ssh-key"));
    assert!(planned.contains("WUKONG_OFFLINE"));
    assert!(planned.contains("provision.ps1"));
    assert!(deployment.provision_directories().is_empty());
    assert_eq!(fs::read(&deployment.vmx).unwrap(), deployment.vmx_initial);
    assert_eq!(disk.calls(), vec!["inspect".to_string()]);
    assert!(iso.state.lock().unwrap().builds.is_empty());
}

#[tokio::test]
async fn receipt_mismatch_requires_reconciliation_and_cleanup() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    disk.tamper(Tampered::ReceiptInstallation);
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk, iso);
    let error = lab
        .provision(deployment.options(deployment.media_source()))
        .await
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<LabError>().unwrap().code,
        "lab_provision_unknown"
    );
    let state = deployment.state();
    assert_eq!(state.phase, LabPhase::RecoveryRequired);
    let pending = state.pending.clone().unwrap();
    assert_eq!(pending.kind, OperationKind::Provision);
    let operation = pending.operation_id.clone();
    assert_eq!(
        state.last_provision.as_ref().unwrap().status,
        RunStatus::Unknown
    );
    let again = lab
        .provision(deployment.options(deployment.media_source()))
        .await
        .unwrap_err();
    assert!(format!("{again:#}").contains("reconcile"), "{again:#}");

    // The VMX backup was restored during the run, but the ISO and backup are
    // cleaned deterministically before the operator may complete the operation.
    lab.reconcile(&operation, ReconcileOutcome::Failed)
        .await
        .unwrap();
    let state = deployment.state();
    assert_eq!(state.phase, LabPhase::Failed);
    assert!(state.pending.is_none());
    assert_eq!(fs::read(&deployment.vmx).unwrap(), deployment.vmx_initial);
    assert!(
        !deployment.provision_directories()[0]
            .join("offline.iso")
            .exists()
    );
}

#[tokio::test]
async fn reconcile_completed_resolves_a_leftover_provisioning_service() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    disk.tamper(Tampered::ServiceLeftBehind);
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk, iso);
    lab.provision(deployment.options(deployment.media_source()))
        .await
        .unwrap_err();
    let operation = deployment.state().pending.unwrap().operation_id;
    assert!(
        deployment.provision_directories()[0]
            .join("offline.iso")
            .is_file()
    );
    lab.reconcile(&operation, ReconcileOutcome::Completed)
        .await
        .unwrap();
    let state = deployment.state();
    assert_eq!(state.phase, LabPhase::Idle);
    assert!(state.pending.is_none());
    let record = state.last_provision.unwrap();
    assert_eq!(record.status, RunStatus::Completed);
    assert!(record.settled);
    assert_eq!(fs::read(&deployment.vmx).unwrap(), deployment.vmx_initial);
    assert!(
        !deployment.provision_directories()[0]
            .join("offline.iso")
            .exists()
    );
}

#[tokio::test]
async fn service_left_behind_keeps_recovery_required() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    disk.tamper(Tampered::ServiceLeftBehind);
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk, iso);
    let error = lab
        .provision(deployment.options(deployment.media_source()))
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("one-shot provisioning service"),
        "{error:#}"
    );
    assert_eq!(deployment.state().phase, LabPhase::RecoveryRequired);
}

#[tokio::test]
async fn registering_requires_template_media_with_a_controller_identity() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let lab =
        wired(&deployment, disk, iso).with_environment_registrar(Arc::new(FakeEnvironment::new()));
    let mut options = deployment.options(deployment.media_source());
    options.register = true;
    let error = lab.provision(options).await.unwrap_err();
    assert!(format!("{error:#}").contains("--template"), "{error:#}");
    assert!(deployment.state().last_provision.is_none());
}

#[tokio::test]
async fn template_provisioning_registers_and_powers_off() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let environment = Arc::new(FakeEnvironment::new());
    let lab = wired(&deployment, disk, iso).with_environment_registrar(environment.clone());
    let directory = deployment.directory.path();
    let template = directory.join("template");
    fs::create_dir_all(&template).unwrap();
    let keys = directory.join("keys");
    fs::create_dir_all(&keys).unwrap();
    let spec = directory.join("spec.json");
    fs::write(
        &spec,
        serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "installation_id": deployment.fixture.identity.installation_id,
            "environment_id": deployment.fixture.identity.environment_id,
            "generation": 64,
            "management_peer": "192.168.199.1",
            "ssh_port": 22268,
            "execution_boundary": "disposable_vm",
        }))
        .unwrap(),
    )
    .unwrap();
    let mut options = deployment.options(ProvisionSource::Template {
        template,
        keys,
        spec,
    });
    options.register = true;
    options.verify_environment = true;
    let outcome = lab.provision(options).await.unwrap();
    assert!(outcome.registration.is_some());
    assert!(outcome.verification.is_some());
    assert_eq!(
        outcome.provision.as_ref().unwrap().environment_verified,
        Some(true)
    );
    assert_eq!(environment.registrations(), 1);
    assert!(!deployment.host.state.lock().unwrap().running);
    let calls = deployment.host.calls();
    assert!(calls.contains(&"revert baseline".to_string()));
    assert!(
        environment
            .state
            .lock()
            .unwrap()
            .registrations
            .first()
            .unwrap()
            .1
            .is_file()
    );
}

#[tokio::test]
async fn restart_recovery_covers_provisioning_intent() {
    let deployment = deployment();
    let paths = super::super::provision::provision_paths(
        &deployment.state_dir.join("windows-lab"),
        &"a".repeat(32),
    )
    .unwrap();
    let spec = ProvisioningSpec {
        provision_id: "a".repeat(32),
        generation: 1,
        media: deployment.fixture.identity.clone(),
        source: ProvisionSource::Media {
            directory: deployment.fixture.root.clone(),
        },
        iso: paths.iso.clone(),
        vmx_backup: paths.vmx_backup.clone(),
        guest_disk: Some(deployment.disk.clone()),
        service_name: "WukongLabProvision-a".into(),
        guest_root: GuestPaths::for_provision(&"a".repeat(32)).unwrap().root,
        baseline_snapshot: "baseline".into(),
        replaces_baseline: false,
        identity_file: None,
        injected: vec![],
    };
    let mut state = LabState::fresh("labvm", "baseline");
    state.phase = LabPhase::Provisioning;
    state.generation = 1;
    state.pending = Some(PendingOperation {
        operation_id: "a".repeat(32),
        kind: OperationKind::Provision,
        generation: 1,
        lab_id: String::new(),
        owner: String::new(),
        purpose: String::new(),
        spec_sha256: String::new(),
        spec: None,
        provision: Some(spec),
        package_sha256: None,
        install_sha256: None,
        snapshot: "baseline".into(),
        started_at: "2026-01-01T00:00:00Z".into(),
    });
    state.last_provision = Some(LabProvision {
        provision_id: "a".repeat(32),
        operation_id: "a".repeat(32),
        generation: 1,
        status: RunStatus::Running,
        source: ProvisionSource::Media {
            directory: deployment.fixture.root.clone(),
        },
        media: deployment.fixture.identity.clone(),
        iso_sha256: None,
        iso_size: None,
        vmx_sha256_before: None,
        vmx_sha256_after: None,
        hive_sha256_before: None,
        hive_sha256_after: None,
        identity_file: None,
        guest_receipt: None,
        connection: None,
        environment_verified: None,
        verification: None,
        baseline_snapshot: None,
        settled: false,
        started_at: "2026-01-01T00:00:00Z".into(),
        ended_at: None,
        error: None,
    });
    let state = deployment.with_configuration(state);
    deployment.store().save(&state).unwrap();

    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk, iso);
    let error = lab
        .provision(deployment.options(deployment.media_source()))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("reconcile"), "{error:#}");
    let state = deployment.state();
    assert_eq!(state.phase, LabPhase::RecoveryRequired);
    assert!(state.last_error.unwrap().contains("controller restarted"));
}

#[tokio::test]
async fn scheme_two_state_without_provision_fields_still_loads() {
    let deployment = deployment();
    let state = deployment.with_configuration(LabState::fresh("labvm", "baseline"));
    let mut record = serde_json::to_value(&state).unwrap();
    record.as_object_mut().unwrap().remove("last_provision");
    let path = deployment.state_dir.join("windows-lab").join("labvm.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let lab = deployment.lab();
    let status = lab.status().await.unwrap();
    assert_eq!(status["state"]["last_provision"], serde_json::Value::Null);
    assert_eq!(status["state"]["schema"], 2);
}
