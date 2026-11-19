//! `lab provision` state-machine tests with fake host, offline disk and ISO
//! adapters. No test executes vmrun, WSL, libguestfs or a real VM. The fake disk
//! is a small in-memory guest filesystem so the machine policy injection, the
//! wrapper's offline observation and the explicit guest cleanup can be exercised
//! end to end without a guest.
use super::*;
use crate::adapters::environments::EnvironmentConfig;
use crate::application::windows_lab::MediaIdentity;
use crate::application::windows_lab::provision;
use crate::application::windows_lab::provision::GuestFile;
use crate::application::windows_lab::provision::fixture;
use crate::application::windows_lab::provision::{
    BOOT_LAUNCHER, BOOT_TASK_FILE, BOOT_TASK_GUID, BOOT_TASK_NAME, BOOT_TASK_XML,
    BOOT_TASK_XML_SHA256, BootTaskCacheState, BootTaskEvidence, BootTaskInjection, BootTaskRemoval,
    BootTaskRemovalEvidence, DiskInspection, DiskInspectionRequest, GUEST_PROVISION_ROOT,
    GUEST_STARTUP_DIR, GuestNetwork, GuestObservation, GuestObservationRequest, GuestPaths,
    GuestPolicyFiles, GuestRemoval, GuestRemovalEvidence, IsoBuilder, MediaFinalizer, OfflineDisk,
    ProvisionEnvironment, RegisteredEnvironment, ServiceRemoval, ServiceRemovalEvidence,
    boot_launcher, boot_task_injection, startup_command_lines, verify_media,
};
use crate::application::windows_lab::types::{
    LabPhase, LabProvision, LabState, OperationKind, PendingOperation, ProvisionSource,
    ProvisioningSpec, RunStatus,
};
use crate::application::windows_lab::{LabError, LabStore, ReconcileOutcome, VmHost};
use crate::application::worker_media::{FinalizeMediaRequest, FinalizedMedia};
use crate::config::WindowsLabVm;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use std::{
    collections::{BTreeMap, BTreeSet},
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
    /// Tiny in-memory guest filesystem keyed by canonical `C:\` path.
    files: BTreeMap<String, Vec<u8>>,
    /// Directories present on the guest disk (provision trees).
    directories: BTreeSet<String>,
    /// Legacy one-shot service key still present in the fake SYSTEM hive.
    service: Option<String>,
    /// Fixed scheduled-task registration in the fake SOFTWARE hive.
    boot_task: BootTaskCacheState,
    /// Fixed scheduled-task injections requested by the controller.
    boot_task_injections: Vec<BootTaskInjection>,
    boot_task_removals: Vec<BootTaskRemoval>,
    tamper: Option<Tampered>,
    removals: Vec<GuestRemoval>,
    service_removals: Vec<ServiceRemoval>,
    /// Test-only fault injection for the legacy service-key removal.
    fail_service_removal: bool,
    /// Test-only fault injection for leftover-file removal.
    fail_file_removal: bool,
    /// Test-only fault injection for the fixed scheduled-task hive edits.
    fail_boot_task_injection: bool,
    fail_boot_task_removal: bool,
}

#[derive(Clone, Copy)]
enum Tampered {
    ReceiptInstallation,
    BootTaskLeftBehind,
}
struct FakeDisk {
    state: Mutex<DiskState>,
    connection: ConnectionReceipt,
    events: Option<Events>,
}

fn guest_path(windows: &str) -> Result<String> {
    GuestPaths::guestfish(windows)
        .context("offline disk requests must carry absolute Windows guest paths")
}

fn under(windows: &str, directory: &str) -> bool {
    windows
        .rsplit_once('\\')
        .is_some_and(|(parent, _)| parent == directory)
}

impl FakeDisk {
    fn new(connection: ConnectionReceipt) -> Self {
        Self {
            state: Mutex::new(DiskState::default()),
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
    fn file(&self, windows: &str) -> Option<Vec<u8>> {
        self.state.lock().unwrap().files.get(windows).cloned()
    }
    fn set_file(&self, windows: &str, bytes: impl Into<Vec<u8>>) {
        self.state
            .lock()
            .unwrap()
            .files
            .insert(windows.into(), bytes.into());
    }
    fn set_directories(&self, directories: impl IntoIterator<Item = String>) {
        self.state.lock().unwrap().directories.extend(directories);
    }
    fn set_service(&self, service: Option<String>) {
        self.state.lock().unwrap().service = service;
    }
    fn removals(&self) -> Vec<GuestRemoval> {
        self.state.lock().unwrap().removals.clone()
    }
    fn service_removals(&self) -> Vec<ServiceRemoval> {
        self.state.lock().unwrap().service_removals.clone()
    }
    fn fail_service_removal(&self, fail: bool) {
        self.state.lock().unwrap().fail_service_removal = fail;
    }
    fn fail_file_removal(&self, fail: bool) {
        self.state.lock().unwrap().fail_file_removal = fail;
    }
    fn directories(&self) -> BTreeSet<String> {
        self.state.lock().unwrap().directories.clone()
    }

    /// Seed leftovers of one failed provisioning attempt on the fake guest.
    /// Seed leftovers of one failed provisioning attempt on the fake guest: the
    /// fixed boot task registration and launcher, the provision directory, and
    /// the legacy one-shot service key from an earlier mechanism.
    fn seed_provision_leftover(&self, paths: &GuestPaths) {
        let mut state = self.state.lock().unwrap();
        state.directories.insert(paths.root.clone());
        state.service = Some(paths.service.clone());
        state.boot_task = BootTaskCacheState {
            tree: true,
            tasks: true,
            boot: true,
        };
        state
            .files
            .insert(paths.task_file.clone(), BOOT_TASK_XML.to_vec());
        state.files.insert(
            paths.launcher.clone(),
            boot_launcher(paths.provision_id(), &paths.wrapper).unwrap(),
        );
    }

    fn boot_task_state(&self) -> BootTaskCacheState {
        self.state.lock().unwrap().boot_task
    }
    fn boot_task_injections(&self) -> Vec<BootTaskInjection> {
        self.state.lock().unwrap().boot_task_injections.clone()
    }
    fn boot_task_removals(&self) -> Vec<BootTaskRemoval> {
        self.state.lock().unwrap().boot_task_removals.clone()
    }
    fn fail_boot_task_injection(&self, fail: bool) {
        self.state.lock().unwrap().fail_boot_task_injection = fail;
    }
    fn fail_boot_task_removal(&self, fail: bool) {
        self.state.lock().unwrap().fail_boot_task_removal = fail;
    }
}

fn plan_json(written: &[GuestFile]) -> serde_json::Value {
    let plan = written
        .iter()
        .find(|file| file.windows_path.ends_with("provision-plan.json"))
        .expect("provision plan was injected");
    serde_json::from_slice(&plan.bytes).unwrap()
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
            &request.software_hive,
            &request.program_data,
            &request.provision_root,
            &request.tasks_dir,
            &request.task_file,
            &request.launcher,
            &request.scripts_dir,
            &request.startup_dir,
            &request.scripts_ini,
            &request.scripts_ini_backup,
            &request.gpt_ini,
            &request.gpt_ini_backup,
        ] {
            state.guest_paths.push(guest_path(windows)?);
        }
        let files = state.files.clone();
        let scripts_ini = files.get(&request.scripts_ini).cloned();
        let startup_command_lines = scripts_ini
            .as_deref()
            .map(startup_command_lines)
            .transpose()?
            .unwrap_or_default();
        let mut policy = GuestPolicyFiles {
            scripts_ini,
            scripts_ini_backup: files.get(&request.scripts_ini_backup).cloned(),
            gpt_ini: files.get(&request.gpt_ini).cloned(),
            gpt_ini_backup: files.get(&request.gpt_ini_backup).cloned(),
            startup_command_lines,
            ..Default::default()
        };
        for windows in files.keys() {
            if under(windows, &request.startup_dir) {
                policy
                    .startup_directory_entries
                    .push(windows.rsplit('\\').next().unwrap_or_default().to_owned());
            }
            if under(windows, &request.scripts_dir) {
                policy
                    .scripts_directory_entries
                    .push(windows.rsplit('\\').next().unwrap_or_default().to_owned());
            }
            if under(windows, &request.gpo_root) {
                policy
                    .group_policy_entries
                    .push(windows.rsplit('\\').next().unwrap_or_default().to_owned());
            }
        }
        let provision_root_entries = state
            .directories
            .iter()
            .filter_map(|directory| {
                directory.strip_prefix(&format!("{}\\", request.provision_root))
            })
            .filter(|name| !name.contains('\\'))
            .map(str::to_owned)
            .collect();
        Ok(DiskInspection {
            windows_volume: true,
            system_hive_present: true,
            software_hive_present: true,
            program_data_present: true,
            control_set: Some(1),
            existing_service: state.service.clone(),
            provision_root_entries,
            task_file_present: files.contains_key(&request.task_file),
            boot_task: state.boot_task,
            launcher: files.get(&request.launcher).cloned(),
            policy,
        })
    }
    async fn write_guest_files(&self, _disk: &Path, files: &[GuestFile]) -> Result<()> {
        self.record("write");
        let mut state = self.state.lock().unwrap();
        state.calls.push("write".into());
        for file in files {
            let guest = guest_path(&file.windows_path)?;
            state.guest_paths.push(guest);
            state
                .files
                .insert(file.windows_path.clone(), file.bytes.clone());
        }
        state.written.extend(files.iter().map(|file| GuestFile {
            windows_path: file.windows_path.clone(),
            bytes: file.bytes.clone(),
            sha256: file.sha256.clone(),
        }));
        Ok(())
    }

    async fn remove_service_key(
        &self,
        _disk: &Path,
        removal: &ServiceRemoval,
    ) -> Result<ServiceRemovalEvidence> {
        self.record("remove-service");
        let mut state = self.state.lock().unwrap();
        state.calls.push("remove-service".into());
        state.guest_paths.push(guest_path(&removal.system_hive)?);
        state.service_removals.push(removal.clone());
        if state.fail_service_removal {
            bail!("hivexsh failed: hivexsh: 'del' command should not be given arguments");
        }
        let present = state.service.as_deref() == Some(removal.service.as_str());
        if present {
            state.service = None;
        }
        Ok(ServiceRemovalEvidence {
            service_present: present,
            removed: present,
        })
    }

    async fn remove_guest_paths(
        &self,
        _disk: &Path,
        removal: &GuestRemoval,
    ) -> Result<GuestRemovalEvidence> {
        self.record("remove-paths");
        let mut state = self.state.lock().unwrap();
        state.calls.push("remove-paths".into());
        state.removals.push(removal.clone());
        if state.fail_file_removal && !removal.files.is_empty() {
            bail!("guestfish failed: simulated leftover-file removal failure");
        }
        let mut evidence = GuestRemovalEvidence::default();
        for directory in &removal.directories {
            state.guest_paths.push(guest_path(directory)?);
            let prefix = format!("{directory}\\");
            if state.directories.remove(directory)
                || state
                    .directories
                    .iter()
                    .any(|path| path.starts_with(&prefix))
            {
                state
                    .directories
                    .retain(|path| !path.starts_with(&prefix) && path != directory);
                evidence.removed_directories.push(directory.clone());
            } else {
                evidence.absent.push(directory.clone());
            }
        }
        for file in &removal.files {
            state.guest_paths.push(guest_path(file)?);
            if state.files.remove(file).is_some() {
                evidence.removed_files.push(file.clone());
            } else {
                evidence.absent.push(file.clone());
            }
        }
        Ok(evidence)
    }

    async fn inject_boot_task(
        &self,
        _disk: &Path,
        injection: &BootTaskInjection,
    ) -> Result<BootTaskEvidence> {
        self.record("inject-boot-task");
        let mut state = self.state.lock().unwrap();
        state.calls.push("inject-boot-task".into());
        if state.fail_boot_task_injection {
            bail!("hivexsh failed: simulated boot-task registration failure");
        }
        assert!(
            !state.boot_task.is_registered(),
            "the controller must not overwrite an existing fixed boot task"
        );
        state.boot_task_injections.push(injection.clone());
        state.boot_task = BootTaskCacheState {
            tree: true,
            tasks: true,
            boot: true,
        };
        Ok(BootTaskEvidence {
            hive_sha256_before: "1".repeat(64),
            hive_sha256_after: "2".repeat(64),
        })
    }

    async fn remove_boot_task(
        &self,
        _disk: &Path,
        removal: &BootTaskRemoval,
    ) -> Result<BootTaskRemovalEvidence> {
        self.record("remove-boot-task");
        let mut state = self.state.lock().unwrap();
        state.calls.push("remove-boot-task".into());
        state.boot_task_removals.push(removal.clone());
        if state.fail_boot_task_removal {
            bail!("hivexsh failed: simulated boot-task removal failure");
        }
        let present = state.boot_task.is_registered();
        state.boot_task = BootTaskCacheState::default();
        Ok(BootTaskRemovalEvidence {
            present,
            removed: present,
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
                &request.wrapper,
                &request.plan,
                &request.receipt,
                &request.launcher,
                &request.task_file,
                &request.software_hive,
                &request.scripts_ini,
                &request.scripts_ini_backup,
                &request.gpt_ini_backup,
            ] {
                state.guest_paths.push(guest_path(windows)?);
            }
        }
        let written = self.written();
        let plan = plan_json(&written);
        let provision_id = plan["provision_id"].as_str().unwrap_or_default().to_owned();
        let mut connection = self.connection.clone();
        connection.installation_id = plan["installation_id"].as_str().unwrap().into();
        connection.environment_id = plan["environment_id"].as_str().unwrap().into();
        connection.generation = plan["generation"].as_u64().unwrap();
        connection.manifest_sha256 = plan["manifest_sha256"].as_str().unwrap().into();
        connection.ssh_port = u16::try_from(plan["ssh_port"].as_u64().unwrap()).unwrap();
        let mut state = self.state.lock().unwrap();
        if matches!(state.tamper, Some(Tampered::ReceiptInstallation)) {
            connection.installation_id = "0".repeat(32);
        }
        let left_behind = matches!(state.tamper, Some(Tampered::BootTaskLeftBehind));
        // Emulate the wrapper's finally block: it writes the receipt first, then
        // removes the fixed scheduled-task registration, its launcher, the
        // wrapper and the plan before shutting the guest down.
        state.files.insert(
            request.receipt.clone(),
            fixture::guest_receipt(&provision_id, &connection),
        );
        if !left_behind {
            state.boot_task = BootTaskCacheState::default();
            state.files.remove(&request.task_file);
            state.files.remove(&request.launcher);
            state.files.remove(&request.wrapper);
            state.files.remove(&request.plan);
        }
        let startup_entry_present = state
            .files
            .get(&request.scripts_ini)
            .map(|bytes| startup_command_lines(bytes))
            .transpose()?
            .unwrap_or_default()
            .iter()
            .any(|line| line == &request.startup_entry);
        Ok(GuestObservation {
            receipt: state.files.get(&request.receipt).cloned(),
            wrapper_present: state.files.contains_key(&request.wrapper),
            plan_present: state.files.contains_key(&request.plan),
            task_file_present: state.files.contains_key(&request.task_file),
            boot_task_registered: state.boot_task.is_registered(),
            launcher_present: state.files.contains_key(&request.launcher),
            scripts_ini_present: state.files.contains_key(&request.scripts_ini),
            startup_entry_present,
            scripts_ini_backup_present: state.files.contains_key(&request.scripts_ini_backup),
            gpt_ini_backup_present: state.files.contains_key(&request.gpt_ini_backup),
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
    async fn finalize(&self, request: &FinalizeMediaRequest) -> Result<FinalizedMedia> {
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
        let record = serde_json::from_slice(&fs::read(request.output.join("media.json"))?)?;
        let media = verify_media(&request.output)?;
        Ok(FinalizedMedia {
            record,
            media_path: request.output.clone(),
            template_sha256: "d".repeat(64),
            ssh_public_key: media.ssh_public_key.public()?,
            ssh_identity_file: request.generate_ssh_key.clone(),
        })
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
    connections: Vec<ConnectionReceipt>,
    doctors: usize,
    /// Environment entry returned by the last successful registration, i.e. the
    /// value the lab adopted as its configuration identity.
    registered: Option<EnvironmentConfig>,
    /// Persisted lab configuration hash read when `register` was called, so a
    /// test can prove adoption happens only after the rewrite succeeds.
    hash_when_registered: Option<String>,
}

struct FakeEnvironment {
    state: Mutex<EnvironmentState>,
    /// State directory observed during registration (none for noise-free fakes).
    state_dir: Option<PathBuf>,
    /// Simulated registration failure; nothing is written and no environment is
    /// returned, matching the `ProvisionEnvironment` contract.
    failure: Option<String>,
}

impl FakeEnvironment {
    fn new() -> Self {
        Self {
            state: Mutex::new(EnvironmentState::default()),
            state_dir: None,
            failure: None,
        }
    }
    fn failing(mut self, message: &str) -> Self {
        self.failure = Some(message.into());
        self
    }
    fn observing(mut self, state_dir: &Path) -> Self {
        self.state_dir = Some(state_dir.to_path_buf());
        self
    }
    fn registrations(&self) -> usize {
        self.state.lock().unwrap().registrations.len()
    }
    fn connections(&self) -> Vec<ConnectionReceipt> {
        self.state.lock().unwrap().connections.clone()
    }
    fn registered(&self) -> Option<EnvironmentConfig> {
        self.state.lock().unwrap().registered.clone()
    }
    fn hash_when_registered(&self) -> Option<String> {
        self.state.lock().unwrap().hash_when_registered.clone()
    }
}

/// The entry a real registration read back from the rewritten configuration:
/// the verified connection bound to the guest address and controller identity.
fn registered_environment(
    connection: &ConnectionReceipt,
    media: &MediaIdentity,
    guest_ip: &str,
    identity_file: &Path,
) -> EnvironmentConfig {
    serde_json::from_value(serde_json::json!({
        "name": format!("Windows worker {}", media.environment_id),
        "platform": "windows",
        "installation_id": media.installation_id,
        "runtime": {
            "backend": "managed_ssh",
            "ssh_host": guest_ip,
            "request_timeout_secs": 300,
            "managed_ssh": {
                "installation_id": media.installation_id,
                "user": media.ssh_user,
                "port": media.ssh_port,
                "identity_file": identity_file,
                "known_hosts_file": identity_file.with_extension("known_hosts"),
                "host_key": connection.host_key,
            }
        }
    }))
    .expect("fake registered environment configuration")
}

#[async_trait]
impl ProvisionEnvironment for FakeEnvironment {
    fn register(
        &self,
        connection: &ConnectionReceipt,
        media: &MediaIdentity,
        guest_ip: &str,
        identity_file: &Path,
    ) -> Result<RegisteredEnvironment> {
        let mut state = self.state.lock().unwrap();
        state
            .registrations
            .push((guest_ip.into(), identity_file.to_path_buf()));
        state.connections.push(connection.clone());
        if let Some(directory) = &self.state_dir {
            state.hash_when_registered = fs::read(directory.join("windows-lab/labvm.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["configuration_sha256"].as_str().map(str::to_owned));
        }
        if let Some(message) = &self.failure {
            bail!("{message}");
        }
        let environment = registered_environment(connection, media, guest_ip, identity_file);
        state.registered = Some(environment.clone());
        Ok(RegisteredEnvironment {
            environment_id: media.environment_id.clone(),
            environment,
            report: serde_json::json!({"environment_id": media.environment_id}),
        })
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

    /// Configuration identity the mapped environment contributes to the guard.
    fn environment(&self, guest_ip: &str) -> EnvironmentConfig {
        registered_environment(
            &self.fixture.connection,
            &self.fixture.identity,
            guest_ip,
            &self.directory.path().join("management_ed25519"),
        )
    }

    /// Lab opened the way a configured VM that maps an environment opens, so a
    /// `--register` rewrite changes its configuration identity.
    fn lab_mapping(&self, environment: &EnvironmentConfig) -> WindowsLab {
        let mut config = self.config();
        config.environment = Some(self.fixture.identity.environment_id.clone());
        WindowsLab::open_with_environment(
            &self.state_dir,
            "labvm",
            &config,
            Some(environment),
            self.host.clone(),
        )
        .unwrap()
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
    wired_with(deployment, disk, iso, deployment.lab())
}

fn wired_with(
    deployment: &Deployment,
    disk: Arc<FakeDisk>,
    iso: Arc<FakeIso>,
    lab: WindowsLab,
) -> WindowsLab {
    lab.with_offline(disk)
        .with_iso_builder(iso)
        .with_media_finalizer(Arc::new(FakeFinalizer {
            source_root: deployment.fixture.root.clone(),
            identity_algorithm: deployment.fixture.identity.ssh_public_key.algorithm.clone(),
            identity_key: deployment.fixture.identity.ssh_public_key.key.clone(),
        }))
        .with_guest_network(Arc::new(FakeNetwork))
}

/// Template-mode options: `--register` requires finalized media that can carry
/// the controller-managed SSH identity, which media mode does not generate.
fn template_options(deployment: &Deployment) -> ProvisionOptions {
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
    deployment.options(ProvisionSource::Template {
        template,
        keys,
        spec,
    })
}

/// Open the lab like the next CLI command after a registration: from the
/// configured VM and the environment entry the rewritten configuration reports.
fn reopened(deployment: &Deployment, environment: &EnvironmentConfig) -> Result<WindowsLab> {
    let mut config = deployment.config();
    config.environment = Some(deployment.fixture.identity.environment_id.clone());
    WindowsLab::open_with_environment(
        &deployment.state_dir,
        "labvm",
        &config,
        Some(environment),
        deployment.host.clone(),
    )
}

/// Rendered error of an open the guard must refuse (`WindowsLab` is not
/// `Debug`, so the tests read the error chain instead of using `unwrap_err`).
fn reopen_error(deployment: &Deployment, environment: &EnvironmentConfig) -> String {
    format!(
        "{:#}",
        reopened(deployment, environment)
            .err()
            .expect("lab must refuse the changed configuration")
    )
}

fn persisted_configuration_hash(deployment: &Deployment) -> String {
    deployment.state().configuration_sha256
}

fn vm_physical_identity(deployment: &Deployment) -> String {
    crate::adapters::environments::lab_guard::physical_identity(&deployment.vmx).unwrap()
}

/// Persist a failed provisioning record so the explicit guest cleanup accepts
/// that operation identity as belonging to this VM.
fn record_failed_provision(deployment: &Deployment, operation_id: &str) -> GuestPaths {
    let paths = GuestPaths::for_provision(operation_id).unwrap();
    let mut state = deployment.with_configuration(LabState::fresh("labvm", "baseline"));
    state.phase = LabPhase::Failed;
    state.generation = 1;
    state.last_provision = Some(LabProvision {
        provision_id: operation_id.into(),
        operation_id: operation_id.into(),
        generation: 1,
        status: RunStatus::Failed,
        source: deployment.media_source(),
        media: deployment.fixture.identity.clone(),
        iso_sha256: None,
        iso_size: None,
        vmx_sha256_before: None,
        vmx_sha256_after: None,
        hive_sha256_before: None,
        hive_sha256_after: None,
        injected: Vec::new(),
        boot_task_sha256: None,
        launcher_sha256: None,
        scripts_ini_sha256: None,
        gpt_ini_sha256: None,
        identity_file: None,
        guest_receipt: None,
        connection: None,
        environment_verified: None,
        verification: None,
        baseline_snapshot: None,
        settled: true,
        started_at: "2026-01-01T00:00:00Z".into(),
        ended_at: None,
        error: Some("operator recorded failure".into()),
    });
    deployment.store().save(&state).unwrap();
    paths
}

/// Emulate the guest state a failed attempt left behind: injected startup
/// policy, its launcher, the provision directory and the legacy service key.
/// Emulate the guest state a failed attempt left behind: the fixed boot-task
/// registration and launcher, the provision directory, an injected retired
/// startup policy (optionally with backups) and the legacy service key.
fn seed_leftovers(disk: &FakeDisk, paths: &GuestPaths, with_backup: bool) {
    disk.seed_provision_leftover(paths);
    disk.set_file(&paths.startup_script, b"@echo off\r\n");
    disk.set_file(
        &paths.scripts_ini,
        format!(
            "[Startup]\r\n0CmdLine=unrelated.cmd\r\n0Parameters=\r\n1CmdLine={}\r\n1Parameters=\r\n",
            paths.startup_entry
        )
        .into_bytes(),
    );
    disk.set_file(&paths.plan, b"{\"schema\":3}\n");
    if with_backup {
        disk.set_file(
            &paths.scripts_ini_backup,
            b"[Startup]\r\n0CmdLine=unrelated.cmd\r\n0Parameters=\r\n".to_vec(),
        );
        disk.set_file(
            &paths.gpt_ini_backup,
            b"[General]\r\nVersion=65539\r\n".to_vec(),
        );
        disk.set_file(&paths.gpt_ini, b"[General]\r\nVersion=65540\r\n".to_vec());
    }
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
        ["inspect", "write", "inject-boot-task", "observe"].map(str::to_string),
        "provisioning never writes a SYSTEM service key or a local-GPO script"
    );
    assert!(
        disk.service_removals().is_empty(),
        "provisioning must not touch the legacy service hive path"
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
    for name in ["provision.ps1", "provision-plan.json", "launch.ps1"] {
        assert!(
            directories[0].join("injected").join(name).is_file(),
            "{name} is retained as host evidence"
        );
    }
    let state = deployment.state();
    assert_eq!(state.phase, LabPhase::Idle);
    let record = state.last_provision.unwrap();
    assert_eq!(record.status, RunStatus::Completed);
    assert_eq!(
        record.boot_task_sha256.as_deref(),
        Some(BOOT_TASK_XML_SHA256)
    );
    assert!(record.launcher_sha256.is_some());
    assert_eq!(
        record.hive_sha256_before.as_deref(),
        Some("1".repeat(64).as_str())
    );
    assert_eq!(
        record.hive_sha256_after.as_deref(),
        Some("2".repeat(64).as_str())
    );
    assert!(record.scripts_ini_sha256.is_none());
    assert!(record.gpt_ini_sha256.is_none());
    let injected = record.injected.clone();
    assert!(
        injected.iter().any(|file| file.guest_path == BOOT_TASK_FILE
            && !file.preexisting
            && file.sha256 == BOOT_TASK_XML_SHA256),
        "the record persists the created scheduled-task file: {injected:?}"
    );
    assert!(
        injected
            .iter()
            .any(|file| file.guest_path == BOOT_LAUNCHER && !file.preexisting),
        "the record persists the created boot launcher"
    );
    assert!(
        injected.iter().all(|file| !file.preexisting),
        "the scheduled-task mechanism creates every guest file it writes"
    );
}
#[tokio::test]
async fn boot_task_injection_failure_requires_recovery() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    disk.fail_boot_task_injection(true);
    deployment.host.set_boot_polls(1);
    let lab = wired(&deployment, disk.clone(), iso);
    let error = lab
        .provision(deployment.options(deployment.media_source()))
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("simulated boot-task registration failure"),
        "{error:#}"
    );
    assert_eq!(deployment.state().phase, LabPhase::RecoveryRequired);
    assert!(!deployment.host.calls().contains(&"start".to_string()));
    assert!(!disk.boot_task_state().is_registered());
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
    for expected in [
        "/Windows/System32/config/SYSTEM",
        "/Windows/System32/config/SOFTWARE",
        "/ProgramData",
        "/ProgramData/WukongLab/provision",
        "/ProgramData/WukongLab/provision/launch.ps1",
        "/Windows/System32/Tasks/WukongLabProvision",
    ] {
        assert!(
            paths.contains(&expected.to_string()),
            "{expected}: {paths:?}"
        );
    }
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
async fn reconcile_completed_resolves_a_leftover_boot_task() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    disk.tamper(Tampered::BootTaskLeftBehind);
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
async fn boot_task_left_behind_keeps_recovery_required() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    disk.tamper(Tampered::BootTaskLeftBehind);
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk, iso);
    let error = lab
        .provision(deployment.options(deployment.media_source()))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("scheduled-task"), "{error:#}");
    assert_eq!(deployment.state().phase, LabPhase::RecoveryRequired);
}
#[tokio::test]
async fn provision_injects_the_canonical_scheduled_task_and_removes_it() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    deployment.host.set_boot_polls(1);
    let lab = wired(&deployment, disk.clone(), iso);
    lab.provision(deployment.options(deployment.media_source()))
        .await
        .unwrap();

    let injections = disk.boot_task_injections();
    assert_eq!(injections.len(), 1);
    assert_eq!(injections[0], boot_task_injection().unwrap());
    // The guest wrapper (emulated by observation) unregisters the task itself;
    // the controller-side `remove_boot_task` is only used by `clean-guest`.
    assert!(disk.boot_task_removals().is_empty());
    assert!(
        !disk.boot_task_state().is_registered(),
        "the fake wrapper unregistered the boot task"
    );

    let written = disk.written();
    assert!(
        written
            .iter()
            .any(|file| file.windows_path == BOOT_TASK_FILE && file.bytes == BOOT_TASK_XML)
    );
    assert!(
        written
            .iter()
            .any(|file| file.windows_path == BOOT_LAUNCHER)
    );
    assert!(
        written
            .iter()
            .all(|file| !file.windows_path.contains("GroupPolicy")),
        "provisioning must not write local Group Policy files"
    );
    let plan = plan_json(&written);
    assert_eq!(plan["schema"], 3);
    assert_eq!(plan["task_name"], BOOT_TASK_NAME);
    assert_eq!(plan["task_guid"], BOOT_TASK_GUID);
    assert!(plan.get("scripts_ini").is_none());
    assert!(plan.get("gpt_ini").is_none());
}
#[tokio::test]
async fn stale_guest_leftovers_are_refused_with_the_cleanup_command() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let stale = "9".repeat(32);
    let paths = GuestPaths::for_provision(&stale).unwrap();
    seed_leftovers(&disk, &paths, false);
    let lab = wired(&deployment, disk.clone(), iso.clone());
    let error = lab
        .provision(deployment.options(deployment.media_source()))
        .await
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains(&format!(
            "wukong lab clean-guest --vm labvm --operation {stale}"
        )),
        "{message}"
    );
    assert!(!deployment.host.calls().contains(&"start".to_string()));
    assert!(iso.state.lock().unwrap().builds.is_empty());
    assert!(deployment.provision_directories().is_empty());
    assert!(deployment.state().last_provision.is_none());
}

#[tokio::test]
async fn clean_guest_removes_exactly_the_recorded_operation_artifacts() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let operation = "9".repeat(32);
    let paths = record_failed_provision(&deployment, &operation);
    seed_leftovers(&disk, &paths, true);
    // Unrelated content must survive: another operation's provision directory
    // and an unrelated retired startup script are never named by the plan.
    let unrelated = format!("{GUEST_PROVISION_ROOT}\\{}", "8".repeat(32));
    disk.set_directories([unrelated.clone()]);
    disk.set_file(&format!("{GUEST_STARTUP_DIR}\\other.cmd"), b"@echo off\r\n");
    let lab = wired(&deployment, disk.clone(), iso);
    let outcome = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation.clone(),
            dry_run: false,
        })
        .await
        .unwrap();
    assert_eq!(outcome.operation_id, operation);
    assert!(!outcome.dry_run);
    assert_eq!(outcome.files_written, 2, "scripts.ini and gpt.ini restored");
    assert!(outcome.boot_task_removed);
    assert!(outcome.files_removed.contains(&paths.task_file));
    assert!(outcome.files_removed.contains(&paths.launcher));
    assert!(outcome.files_removed.contains(&paths.startup_script));
    assert!(outcome.files_removed.contains(&paths.scripts_ini_backup));
    assert!(outcome.files_removed.contains(&paths.gpt_ini_backup));
    assert_eq!(outcome.directories_removed, vec![paths.root.clone()]);
    assert_eq!(outcome.service_key_removed, Some(paths.service.clone()));
    assert!(!disk.boot_task_state().is_registered());
    // Restored policy keeps the unrelated startup entry and drops ours; the
    // backup bytes are written verbatim.
    let restored = disk
        .file(&paths.scripts_ini)
        .expect("scripts.ini was restored");
    assert_eq!(
        restored,
        b"[Startup]\r\n0CmdLine=unrelated.cmd\r\n0Parameters=\r\n".to_vec(),
        "restore is byte exact"
    );
    assert_eq!(
        disk.file(&paths.gpt_ini).unwrap(),
        b"[General]\r\nVersion=65539\r\n".to_vec()
    );
    assert!(disk.file(&paths.startup_script).is_none());
    assert!(disk.file(&paths.task_file).is_none());
    assert!(disk.file(&paths.launcher).is_none());
    assert!(
        disk.file(&format!("{GUEST_STARTUP_DIR}\\other.cmd"))
            .is_some()
    );
    let removals = disk.removals();
    assert_eq!(removals.len(), 2, "leftover files and the directory");
    assert!(removals[0].directories.is_empty());
    assert_eq!(
        removals[0].files,
        vec![
            paths.task_file.clone(),
            paths.launcher.clone(),
            paths.startup_script.clone(),
            paths.scripts_ini_backup.clone(),
            paths.gpt_ini_backup.clone(),
        ]
    );
    assert!(removals[1].files.is_empty());
    assert_eq!(removals[1].directories, vec![paths.root.clone()]);
    assert_eq!(disk.service_removals().len(), 1);
    let calls = disk.calls();
    assert_eq!(
        calls,
        [
            "inspect",
            "remove-boot-task",
            "remove-service",
            "write",
            "remove-paths",
            "remove-paths"
        ]
        .map(str::to_string),
        "the boot registration is removed first and the provision directory last"
    );
    // A second cleanup run is an idempotent no-op.
    let again = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation,
            dry_run: false,
        })
        .await
        .unwrap();
    assert_eq!(again.files_written, 0);
    assert!(again.files_removed.is_empty());
    assert!(again.directories_removed.is_empty());
    assert!(!again.boot_task_removed);
    assert!(again.service_key_removed.is_none());
    assert!(
        again
            .actions
            .iter()
            .any(|action| action.contains("no offline provisioning leftovers")),
        "{:?}",
        again.actions
    );
}
#[tokio::test]
async fn clean_guest_hive_failure_stops_before_removing_anything_else() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let operation = "b".repeat(32);
    let paths = record_failed_provision(&deployment, &operation);
    seed_leftovers(&disk, &paths, true);
    disk.fail_service_removal(true);
    let lab = wired(&deployment, disk.clone(), iso);
    let error = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation.clone(),
            dry_run: false,
        })
        .await
        .unwrap_err();
    let message = format!("{error:#}");
    // The failure names the exact step, its underlying hivexsh cause and what
    // was deliberately not attempted; nothing after the hive deletion ran.
    assert!(
        message.contains("remove the legacy one-shot provisioning service key"),
        "{message}"
    );
    assert!(
        message.contains("hivexsh: 'del' command should not be given arguments"),
        "{message}"
    );
    assert!(
        message.contains("completed: [remove the fixed scheduled-task boot registration]"),
        "{message}"
    );
    for step in [
        "restore the machine startup policy",
        "remove the operation's leftover guest files",
        "remove the operation's provision directory",
    ] {
        assert!(
            message.contains(step),
            "not-attempted step {step:?} is reported: {message}"
        );
    }
    assert_eq!(
        disk.calls(),
        ["inspect", "remove-boot-task", "remove-service"].map(str::to_string)
    );
    assert!(disk.directories().contains(&paths.root));
    assert!(disk.file(&paths.startup_script).is_some());
    assert!(
        !disk
            .removals()
            .iter()
            .any(|removal| removal.directories.contains(&paths.root)),
        "the provision directory must survive a hive failure"
    );
    // The ordering makes the retry safe: clear the transient hive failure and
    // the same command finishes the job without recreating anything.
    disk.fail_service_removal(false);
    let outcome = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation,
            dry_run: false,
        })
        .await
        .unwrap();
    assert_eq!(outcome.service_key_removed, Some(paths.service.clone()));
    assert_eq!(outcome.directories_removed, vec![paths.root.clone()]);
    assert!(disk.directories().is_empty());
}
#[tokio::test]
async fn clean_guest_file_failure_keeps_the_provision_directory() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let operation = "a".repeat(32);
    let paths = record_failed_provision(&deployment, &operation);
    seed_leftovers(&disk, &paths, true);
    disk.fail_file_removal(true);
    let lab = wired(&deployment, disk.clone(), iso);
    let error = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation.clone(),
            dry_run: false,
        })
        .await
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("remove the operation's leftover guest files"),
        "{message}"
    );
    assert!(
        message.contains("not attempted: [remove the operation's provision directory]"),
        "{message}"
    );
    assert_eq!(
        disk.calls(),
        [
            "inspect",
            "remove-boot-task",
            "remove-service",
            "write",
            "remove-paths"
        ]
        .map(str::to_string)
    );
    assert!(disk.directories().contains(&paths.root));
    // The retry only repeats the pending file and directory removals.
    disk.fail_file_removal(false);
    let outcome = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation,
            dry_run: false,
        })
        .await
        .unwrap();
    assert_eq!(outcome.directories_removed, vec![paths.root.clone()]);
    assert!(outcome.service_key_removed.is_none());
}
#[tokio::test]
async fn clean_guest_without_a_legacy_service_key_is_idempotent() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let operation = "c".repeat(32);
    let paths = record_failed_provision(&deployment, &operation);
    seed_leftovers(&disk, &paths, true);
    disk.set_service(None);
    let lab = wired(&deployment, disk.clone(), iso);
    let outcome = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation.clone(),
            dry_run: false,
        })
        .await
        .unwrap();
    assert!(
        disk.service_removals().is_empty(),
        "a missing legacy key must not open the SYSTEM hive"
    );
    assert!(outcome.service_key_removed.is_none());
    assert!(outcome.boot_task_removed);
    assert_eq!(outcome.directories_removed, vec![paths.root.clone()]);
    assert_eq!(
        disk.calls(),
        [
            "inspect",
            "remove-boot-task",
            "write",
            "remove-paths",
            "remove-paths"
        ]
        .map(str::to_string)
    );
    let again = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation,
            dry_run: false,
        })
        .await
        .unwrap();
    assert!(
        again
            .actions
            .iter()
            .any(|action| action.contains("no offline provisioning leftovers")),
        "{:?}",
        again.actions
    );
    assert!(disk.service_removals().is_empty());
}
#[tokio::test]
async fn clean_guest_without_a_backup_removes_only_the_startup_entry() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let operation = "7".repeat(32);
    let paths = record_failed_provision(&deployment, &operation);
    seed_leftovers(&disk, &paths, false);
    let lab = wired(&deployment, disk.clone(), iso);
    let outcome = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation,
            dry_run: false,
        })
        .await
        .unwrap();
    assert_eq!(
        outcome.files_written, 1,
        "scripts.ini is rewritten in place"
    );
    let restored = disk.file(&paths.scripts_ini).expect("scripts.ini survives");
    let text = String::from_utf8(restored).unwrap();
    assert!(text.contains("0CmdLine=unrelated.cmd"), "{text}");
    assert!(!text.contains(&paths.startup_entry), "{text}");
    assert!(outcome.boot_task_removed);
    assert!(outcome.files_removed.contains(&paths.task_file));
    assert!(outcome.files_removed.contains(&paths.launcher));
    assert!(outcome.files_removed.contains(&paths.startup_script));
    assert_eq!(outcome.directories_removed, vec![paths.root.clone()]);
}

#[tokio::test]
async fn clean_guest_deletes_a_cli_created_gpt_ini_for_the_last_operation() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let operation = "2".repeat(32);
    let paths = record_failed_provision(&deployment, &operation);
    // The retired CLI created gpt.ini from nothing: no backup exists and the
    // document is still exactly the created form, so cleanup may delete it.
    disk.set_file(&paths.gpt_ini, provision::LEGACY_CREATED_GPT_INI.to_vec());
    let lab = wired(&deployment, disk.clone(), iso);
    let outcome = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation,
            dry_run: false,
        })
        .await
        .unwrap();
    assert!(outcome.files_removed.contains(&paths.gpt_ini));
    assert!(disk.file(&paths.gpt_ini).is_none());
}
#[tokio::test]
async fn clean_guest_boot_task_failure_stops_before_removing_guest_paths() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let operation = "d".repeat(32);
    let paths = record_failed_provision(&deployment, &operation);
    seed_leftovers(&disk, &paths, true);
    disk.fail_boot_task_removal(true);
    let lab = wired(&deployment, disk.clone(), iso);
    let error = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation.clone(),
            dry_run: false,
        })
        .await
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("remove the fixed scheduled-task boot registration"),
        "{message}"
    );
    assert!(message.contains("completed: [none]"), "{message}");
    assert_eq!(
        disk.calls(),
        ["inspect", "remove-boot-task"].map(str::to_string)
    );
    assert!(disk.boot_task_state().is_registered());
    assert!(disk.file(&paths.task_file).is_some());
    assert!(disk.file(&paths.startup_script).is_some());
    assert!(disk.directories().contains(&paths.root));

    disk.fail_boot_task_removal(false);
    let outcome = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation,
            dry_run: false,
        })
        .await
        .unwrap();
    assert!(outcome.boot_task_removed);
    assert_eq!(outcome.directories_removed, vec![paths.root.clone()]);
    assert!(!disk.boot_task_state().is_registered());
}

#[tokio::test]
async fn clean_guest_dry_run_is_read_only() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let operation = "6".repeat(32);
    let paths = record_failed_provision(&deployment, &operation);
    seed_leftovers(&disk, &paths, false);
    let lab = wired(&deployment, disk.clone(), iso);
    let outcome = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation,
            dry_run: true,
        })
        .await
        .unwrap();
    assert!(outcome.dry_run);
    assert_eq!(disk.calls(), vec!["inspect".to_string()]);
    assert!(outcome.files_written == 0 && outcome.files_removed.is_empty());
    assert!(!outcome.actions.is_empty());
    assert!(disk.file(&paths.scripts_ini).is_some());
    assert!(disk.file(&paths.startup_script).is_some());
    assert!(disk.file(&paths.task_file).is_some());
    assert!(disk.boot_task_state().is_registered());
}
#[tokio::test]
async fn clean_guest_never_touches_an_unrelated_legacy_service() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let operation = "3".repeat(32);
    let paths = record_failed_provision(&deployment, &operation);
    let unrelated = format!("WukongLabProvision-{}", "2".repeat(32));
    disk.set_directories([paths.root.clone()]);
    disk.set_service(Some(unrelated.clone()));
    let lab = wired(&deployment, disk.clone(), iso);
    let outcome = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation,
            dry_run: false,
        })
        .await
        .unwrap();
    assert_eq!(outcome.directories_removed, vec![paths.root.clone()]);
    assert!(outcome.service_key_removed.is_none());
    assert!(disk.service_removals().is_empty());
    assert_eq!(
        disk.state.lock().unwrap().service.as_deref(),
        Some(unrelated.as_str())
    );
}

#[tokio::test]
async fn clean_guest_refuses_unknown_or_foreign_operations() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let lab = wired(&deployment, disk.clone(), iso);
    let foreign = "5".repeat(32);
    let error = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: foreign.clone(),
            dry_run: false,
        })
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("not a recorded offline provisioning"),
        "{error:#}"
    );
    assert!(format!("{error:#}").contains(&foreign), "{error:#}");
    let error = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: "short".into(),
            dry_run: false,
        })
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("offline provisioning operation"),
        "{error:#}"
    );
    assert!(disk.calls().is_empty());
}

#[tokio::test]
async fn clean_guest_requires_a_powered_off_vm() {
    let deployment = deployment();
    let disk = Arc::new(FakeDisk::new(deployment.fixture.connection.clone()));
    let iso = Arc::new(FakeIso::new());
    let operation = "4".repeat(32);
    record_failed_provision(&deployment, &operation);
    deployment.host.set_running(true);
    let lab = wired(&deployment, disk.clone(), iso);
    let error = lab
        .clean_guest_leftovers(CleanGuestOptions {
            operation_id: operation,
            dry_run: false,
        })
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("power off"), "{error:#}");
    assert!(disk.calls().is_empty());
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
    // Regression gate: a successful install reports the installer-generated
    // sshd host key, which differs from the controller SSH client identity from
    // the enrollment. Registration must receive that receipt key; the previous
    // equality check against the media client key failed here.
    let connections = environment.connections();
    assert_eq!(connections.len(), 1);
    let connection = &connections[0];
    assert_eq!(connection.host_key, deployment.fixture.connection.host_key);
    assert_ne!(
        connection.host_key,
        deployment.fixture.identity.ssh_public_key.public().unwrap()
    );
    assert_eq!(
        connection.host_fingerprint,
        connection.host_key.fingerprint().unwrap()
    );
    assert_eq!(
        connection.host_fingerprint,
        deployment.fixture.connection.host_fingerprint
    );
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

/// Defect #9 regression: `--register` rewrites the mapped `[environments.*]`
/// entry, so the persisted lab configuration identity must move with it. A
/// successful registration whose configuration hash stayed stale made every
/// following command fail at open with the configuration-change guard.
#[tokio::test]
async fn successful_registration_rebinds_the_persisted_configuration_identity() {
    let deployment = deployment();
    let old_environment = deployment.environment("192.168.199.128");
    let pre_registration_hash = deployment
        .lab_mapping(&old_environment)
        .store
        .configuration_sha256
        .clone()
        .expect("lab configuration hash");
    let environment = Arc::new(FakeEnvironment::new().observing(&deployment.state_dir));
    let lab = wired_with(
        &deployment,
        Arc::new(FakeDisk::new(deployment.fixture.connection.clone())),
        Arc::new(FakeIso::new()),
        deployment.lab_mapping(&old_environment),
    )
    .with_environment_registrar(environment.clone());
    let mut options = template_options(&deployment);
    options.register = true;
    options.verify_environment = true;
    lab.provision(options).await.unwrap();

    // The rewrite is durable when the registrar returns; only after that may
    // the state carry the new identity.
    assert_eq!(
        environment.hash_when_registered().as_deref(),
        Some(pre_registration_hash.as_str())
    );
    let registered = environment
        .registered()
        .expect("registration returns the rewritten environment entry");
    let configuration_sha256 = persisted_configuration_hash(&deployment);
    assert_ne!(configuration_sha256, pre_registration_hash);

    // The physical-VM identity is part of admission (`LabStore::lock`), so it
    // must move with the state or the next mutating command would be refused.
    let identity_path = deployment.state_dir.join("windows-lab").join(format!(
        "{}.identity.json",
        vm_physical_identity(&deployment)
    ));
    let identity: serde_json::Value =
        serde_json::from_slice(&fs::read(&identity_path).unwrap()).unwrap();
    assert_eq!(
        identity["configuration_sha256"],
        serde_json::json!(configuration_sha256)
    );
    assert_eq!(identity["vm_id"], serde_json::json!("labvm"));

    // Subsequent commands open cleanly: status and the replace-baseline dry
    // run that the real acceptance could not execute after `--register`.
    let status = reopened(&deployment, &registered)
        .unwrap()
        .status()
        .await
        .unwrap();
    assert_eq!(status["vm_id"], "labvm");
    let dry_run_lab = wired_with(
        &deployment,
        Arc::new(FakeDisk::new(deployment.fixture.connection.clone())),
        Arc::new(FakeIso::new()),
        reopened(&deployment, &registered).unwrap(),
    );
    let mut dry_run = deployment.options(deployment.media_source());
    dry_run.dry_run = true;
    dry_run.replace_baseline = true;
    let planned = dry_run_lab.provision(dry_run).await.unwrap();
    assert!(planned.dry_run);
    assert!(
        planned
            .planned_actions
            .iter()
            .any(|action| action.contains("delete the configured baseline snapshot"))
    );
}

/// The guard still refuses a mapped environment or unrelated configuration
/// changed outside the registration that adopted the rewrite.
#[tokio::test]
async fn foreign_configuration_changes_are_still_refused_after_registration() {
    let deployment = deployment();
    let old_environment = deployment.environment("192.168.199.128");
    let environment = Arc::new(FakeEnvironment::new());
    let lab = wired_with(
        &deployment,
        Arc::new(FakeDisk::new(deployment.fixture.connection.clone())),
        Arc::new(FakeIso::new()),
        deployment.lab_mapping(&old_environment),
    )
    .with_environment_registrar(environment.clone());
    let mut options = template_options(&deployment);
    options.register = true;
    lab.provision(options).await.unwrap();
    let registered = environment.registered().unwrap();

    // A manually edited mapped environment entry is not accepted.
    let mut edited = registered.clone();
    let mut runtime = serde_json::to_value(&edited.runtime).unwrap();
    runtime["ssh_host"] = serde_json::json!("192.168.199.130");
    edited.runtime = serde_json::from_value(runtime).unwrap();
    let error = reopen_error(&deployment, &edited);
    assert!(error.contains("configuration changed"), "{error}");

    // Neither is the pre-registration environment the operation replaced.
    let error = reopen_error(&deployment, &old_environment);
    assert!(error.contains("configuration changed"), "{error}");

    // An unrelated `[windows_lab.vms.<alias>]` edit keeps being refused.
    let mut foreign_config = deployment.config();
    foreign_config.environment = Some(deployment.fixture.identity.environment_id.clone());
    foreign_config.vmrun = deployment.directory.path().join("other-vmrun.exe");
    let error = format!(
        "{:#}",
        WindowsLab::open_with_environment(
            &deployment.state_dir,
            "labvm",
            &foreign_config,
            Some(&registered),
            deployment.host.clone(),
        )
        .err()
        .expect("lab must refuse an unrelated configuration change")
    );
    assert!(error.contains("configuration changed"), "{error}");
}

/// A failed registration writes no configuration entry and returns no
/// environment, so the persisted identity must stay exactly as it was and the
/// guard must keep refusing the entry a failed attempt would have written.
#[tokio::test]
async fn failed_registration_keeps_the_previous_configuration_identity() {
    let deployment = deployment();
    let old_environment = deployment.environment("192.168.199.128");
    let pre_registration_hash = deployment
        .lab_mapping(&old_environment)
        .store
        .configuration_sha256
        .clone()
        .expect("lab configuration hash");
    let environment = Arc::new(
        FakeEnvironment::new()
            .failing("simulated configuration rewrite failure")
            .observing(&deployment.state_dir),
    );
    let lab = wired_with(
        &deployment,
        Arc::new(FakeDisk::new(deployment.fixture.connection.clone())),
        Arc::new(FakeIso::new()),
        deployment.lab_mapping(&old_environment),
    )
    .with_environment_registrar(environment.clone());
    let mut options = template_options(&deployment);
    options.register = true;
    let error = lab.provision(options).await.unwrap_err();
    assert!(
        format!("{error:#}").contains("simulated configuration rewrite failure"),
        "{error:#}"
    );

    assert_eq!(
        environment.hash_when_registered().as_deref(),
        Some(pre_registration_hash.as_str())
    );
    assert_eq!(
        persisted_configuration_hash(&deployment),
        pre_registration_hash
    );
    // The unchanged identity still opens the lab...
    reopened(&deployment, &old_environment)
        .unwrap()
        .status()
        .await
        .unwrap();
    // ...while the entry the failed rewrite would have written is not accepted:
    // no false assumption that the failed operation changed the configuration.
    let error = reopen_error(&deployment, &deployment.environment("192.168.199.129"));
    assert!(error.contains("configuration changed"), "{error}");
}

/// Registration is the only step allowed to move the identity: the rewrite it
/// commits is adopted even when a later step of the same command (here the
/// baseline revert) fails, because the configuration file is already changed.
#[tokio::test]
async fn registration_is_adopted_when_a_later_step_of_the_command_fails() {
    let deployment = deployment();
    let old_environment = deployment.environment("192.168.199.128");
    let pre_registration_hash = deployment
        .lab_mapping(&old_environment)
        .store
        .configuration_sha256
        .clone()
        .expect("lab configuration hash");
    let environment = Arc::new(FakeEnvironment::new());
    let lab = wired_with(
        &deployment,
        Arc::new(FakeDisk::new(deployment.fixture.connection.clone())),
        Arc::new(FakeIso::new()),
        deployment.lab_mapping(&old_environment),
    )
    .with_environment_registrar(environment.clone());
    let mut options = template_options(&deployment);
    options.register = true;
    deployment.host.state.lock().unwrap().fail_revert = true;
    let error = lab.provision(options).await.unwrap_err();
    assert!(format!("{error:#}").contains("revert"), "{error:#}");
    let registered = environment
        .registered()
        .expect("the configuration rewrite succeeded before the revert failed");
    assert_ne!(
        persisted_configuration_hash(&deployment),
        pre_registration_hash
    );
    reopened(&deployment, &registered)
        .unwrap()
        .status()
        .await
        .unwrap();
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
        injected: Vec::new(),
        boot_task_sha256: None,
        launcher_sha256: None,
        scripts_ini_sha256: None,
        gpt_ini_sha256: None,
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
    let state_path = deployment.store().state_path("labvm");
    let before = fs::read(&state_path).unwrap();
    let mut dry_run = deployment.options(deployment.media_source());
    dry_run.dry_run = true;
    assert!(lab.provision(dry_run).await.is_err());
    assert_eq!(fs::read(&state_path).unwrap(), before);

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
