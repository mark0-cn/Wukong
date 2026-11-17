//! Offline provisioning of the registered lab VM.
//!
//! The worker is installed while the guest is powered off: finalized media is
//! re-verified, a fixed local Group Policy machine-startup script is injected
//! with a backup of the previous policy files, the guest boots once and runs the
//! wrapper without a login, then the controller restores the VMX and reads the
//! guest receipt through libguestfs. A Windows service cannot run PowerShell
//! scripts (SCM requires `StartServiceCtrlDispatcher`), so the boot mechanism is
//! the local-GPO startup script, the same layout used by the earlier manual
//! provisioning. Host tools are fixed, bounded and addressed only from operator
//! configuration; model input never reaches this module.
use super::{MediaIdentity, ProvisionSource, ProvisioningSpec, SshKey};
use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub const MEDIA_LABEL: &str = "WUKONG_OFFLINE";
pub const MEDIA_RECORD: &str = "media.json";
pub const SETUP_EXECUTABLE: &str = "WukongWorkerSetup.exe";
pub const SETUP_RECORD: &str = "setup.json";
pub const ENROLLMENT_RECORD: &str = "enrollment/enrollment.json";
pub const ENROLLMENT_SIGNATURE: &str = "enrollment/enrollment.sig";
pub const MANIFEST_RECORD: &str = "payload/manifest.json";
pub const MANIFEST_SIGNATURE: &str = "payload/manifest.sig";
pub const GUEST_PROGRAM_DATA: &str = r"C:\ProgramData";
pub const GUEST_PROVISION_BASE: &str = r"C:\ProgramData\WukongLab";
pub const GUEST_PROVISION_ROOT: &str = r"C:\ProgramData\WukongLab\provision";
pub const GUEST_SYSTEM_CONFIG: &str = r"C:\Windows\System32\config";
pub const GUEST_SYSTEM_HIVE: &str = r"C:\Windows\System32\config\SYSTEM";
pub const POWERSHELL: &str = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
/// Local Group Policy locations. Windows runs machine startup scripts from
/// `scripts.ini` under `Scripts\Startup` without a user login; this is the same
/// layout the earlier hand-provisioned lab used and it replaces the broken
/// PowerShell-as-a-service one-shot.
pub const GUEST_GPO_ROOT: &str = r"C:\Windows\System32\GroupPolicy";
pub const GUEST_SCRIPTS_DIR: &str = r"C:\Windows\System32\GroupPolicy\Machine\Scripts";
pub const GUEST_SCRIPTS_INI: &str = r"C:\Windows\System32\GroupPolicy\Machine\Scripts\scripts.ini";
pub const GUEST_STARTUP_DIR: &str = r"C:\Windows\System32\GroupPolicy\Machine\Scripts\Startup";
pub const GUEST_GPT_INI: &str = r"C:\Windows\System32\GroupPolicy\gpt.ini";
/// Fixed prefix/suffix of the injected startup entry (`wukong-provision-<id>.cmd`).
pub const STARTUP_ENTRY_PREFIX: &str = "wukong-provision-";
pub const STARTUP_ENTRY_SUFFIX: &str = ".cmd";
/// Fixed suffix of the per-operation policy backups written beside the originals.
pub const POLICY_BACKUP_SUFFIX: &str = ".wukong-lab-";
pub const MAX_MEDIA_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_RECEIPT_BYTES: u64 = 1024 * 1024;
pub const MAX_INSTALLER_OUTPUT: usize = 64 * 1024;
pub const MAX_MEDIA_ENTRIES: usize = 20_000;
pub const MAX_POLICY_BYTES: u64 = 256 * 1024;
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const UTF16_LE_BOM: [u8; 2] = [0xff, 0xfe];
const UTF8_BOM: [u8; 3] = [0xef, 0xbb, 0xbf];

/// Fixed guest paths written before boot and inspected afterwards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestPaths {
    /// Windows form of the per-provision directory.
    pub root: String,
    pub wrapper: String,
    pub plan: String,
    pub receipt: String,
    /// Command line injected under `[Startup]` in the machine `scripts.ini`.
    pub startup_entry: String,
    /// Fixed launcher run by the local Group Policy machine startup script.
    pub startup_script: String,
    pub scripts_ini: String,
    /// Per-operation backup of the previous `scripts.ini` bytes.
    pub scripts_ini_backup: String,
    pub gpt_ini: String,
    /// Per-operation backup of the previous `gpt.ini` bytes.
    pub gpt_ini_backup: String,
    /// Legacy one-shot service name written by earlier attempts. Provisioning
    /// never writes it; guest cleanup may remove that exact key from an old disk.
    pub service: String,
}

impl GuestPaths {
    pub fn for_provision(provision_id: &str) -> Result<Self> {
        ensure!(
            provision_id.len() == 32 && provision_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid provision identity"
        );
        ensure!(
            provision_id.bytes().all(|byte| !byte.is_ascii_uppercase()),
            "invalid provision identity"
        );
        let root = format!(r"{GUEST_PROVISION_ROOT}\{provision_id}");
        let startup_entry = format!("{STARTUP_ENTRY_PREFIX}{provision_id}{STARTUP_ENTRY_SUFFIX}");
        Ok(Self {
            wrapper: format!(r"{root}\provision.ps1"),
            plan: format!(r"{root}\provision-plan.json"),
            receipt: format!(r"{root}\receipt.json"),
            startup_script: format!(r"{GUEST_STARTUP_DIR}\{startup_entry}"),
            startup_entry,
            scripts_ini: GUEST_SCRIPTS_INI.into(),
            scripts_ini_backup: format!(
                r"{GUEST_SCRIPTS_DIR}\scripts.ini{POLICY_BACKUP_SUFFIX}{provision_id}.bak"
            ),
            gpt_ini: GUEST_GPT_INI.into(),
            gpt_ini_backup: format!(
                r"{GUEST_GPO_ROOT}\gpt.ini{POLICY_BACKUP_SUFFIX}{provision_id}.bak"
            ),
            service: format!("WukongLabProvision-{provision_id}"),
            root,
        })
    }

    /// The validated 32 character operation identity of this directory.
    pub fn provision_id(&self) -> &str {
        self.root
            .rsplit('\\')
            .next()
            .expect("provision root always ends with the operation identity")
    }

    /// Convert the canonical Windows guest path (`C:\...`) to the form used by
    /// libguestfs, where the Windows system volume is mounted at `/`.
    ///
    /// This is the only place the two representations may differ: provision
    /// requests and persisted records always carry absolute `C:\` paths, and
    /// host adapters call this once at their boundary. Relative, UNC, drive
    /// aliases, empty components and `.`/`..` traversal are rejected so the
    /// translated path can never leave the guest volume.
    pub fn guestfish(path: &str) -> Result<String> {
        let relative = path
            .strip_prefix(r"C:\")
            .context("guest path must be an absolute C: path")?;
        let mut guest = String::with_capacity(relative.len() + 1);
        guest.push('/');
        for (index, component) in relative.split('\\').enumerate() {
            validate_guest_component(component)?;
            if index > 0 {
                guest.push('/');
            }
            guest.push_str(component);
        }
        Ok(guest)
    }

    /// Inverse of [`GuestPaths::guestfish`]: the canonical `C:\` form of a
    /// libguestfs guest path. Test and evidence helpers use it to prove the two
    /// representations round-trip exactly.
    pub fn windows(guest: &str) -> Result<String> {
        let relative = guest
            .strip_prefix('/')
            .context("guestfish path must be absolute")?;
        ensure!(
            !relative.starts_with('/'),
            "guestfish path may not be a network path"
        );
        ensure!(!relative.is_empty(), "guest path must name a file");
        let mut windows = String::from(r"C:\");
        for (index, component) in relative.split('/').enumerate() {
            validate_guest_component(component)?;
            if index > 0 {
                windows.push('\\');
            }
            windows.push_str(component);
        }
        Ok(windows)
    }
}

/// One guest path component shared by both representations.
fn validate_guest_component(component: &str) -> Result<()> {
    ensure!(
        !component.is_empty(),
        "guest path contains an empty component"
    );
    ensure!(
        component != "." && component != "..",
        "guest path may not contain a relative component"
    );
    ensure!(
        !component.contains(['\n', '\r', '\0', '"', '/', '\\', ':']),
        "guest path contains an unsupported character"
    );
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestFile {
    pub windows_path: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

/// Facts observed on the powered-off system volume before any write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskInspection {
    pub windows_volume: bool,
    pub system_hive_present: bool,
    pub program_data_present: bool,
    /// Current control set from the hive `Select` key.
    pub control_set: Option<u32>,
    /// Any leftover one-shot provisioning service written by an earlier attempt.
    pub existing_service: Option<String>,
    /// Entries left below the fixed provision root directory.
    pub provision_root_entries: Vec<String>,
    /// Existing local Group Policy files and injected startup artifacts.
    pub policy: GuestPolicyFiles,
}

impl DiskInspection {
    /// Operation identities of every guest artifact that looks like an earlier
    /// offline provisioning attempt. An empty list means the guest is pristine.
    pub fn leftover_operation_ids(&self) -> Vec<String> {
        let mut ids = self.policy.leftover_operation_ids();
        if let Some(service) = &self.existing_service
            && let Some(id) = marker_id(service, "WukongLabProvision-", "")
        {
            ids.push(id);
        }
        for entry in &self.provision_root_entries {
            if entry.len() == 32
                && entry
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                ids.push(entry.clone());
            }
        }
        ids.sort();
        ids.dedup();
        ids
    }
}

/// Existing machine policy files read from the powered-off system volume. Text
/// policy files are kept as exact bytes so the controller can preserve them in a
/// per-operation backup and restore them afterwards.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestPolicyFiles {
    pub scripts_ini: Option<Vec<u8>>,
    /// Backup written by this operation (`scripts.ini.wukong-lab-<id>.bak`).
    pub scripts_ini_backup: Option<Vec<u8>>,
    pub gpt_ini: Option<Vec<u8>>,
    /// Backup written by this operation (`gpt.ini.wukong-lab-<id>.bak`).
    pub gpt_ini_backup: Option<Vec<u8>>,
    /// Command lines of the existing `[Startup]` section.
    pub startup_command_lines: Vec<String>,
    /// File names inside `Scripts\Startup`.
    pub startup_directory_entries: Vec<String>,
    /// File names inside `Scripts` (backs up the existing `scripts.ini`).
    pub scripts_directory_entries: Vec<String>,
    /// File names inside the local Group Policy root (backs up `gpt.ini`).
    pub group_policy_entries: Vec<String>,
}

impl GuestPolicyFiles {
    /// Operation identities referenced by any injected startup script or policy
    /// backup of a previous provisioning attempt on this disk.
    pub fn leftover_operation_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        for line in &self.startup_command_lines {
            if let Some(id) = marker_id(line, STARTUP_ENTRY_PREFIX, STARTUP_ENTRY_SUFFIX) {
                ids.push(id);
            }
        }
        for name in self
            .startup_directory_entries
            .iter()
            .chain(self.scripts_directory_entries.iter())
            .chain(self.group_policy_entries.iter())
        {
            for (prefix, suffix) in [
                (STARTUP_ENTRY_PREFIX, STARTUP_ENTRY_SUFFIX),
                ("scripts.ini", ".bak"),
                ("gpt.ini", ".bak"),
            ] {
                if let Some(id) = backup_marker_id(name, prefix, suffix) {
                    ids.push(id);
                }
            }
        }
        ids.sort();
        ids.dedup();
        ids
    }
}

fn marker_id(value: &str, prefix: &str, suffix: &str) -> Option<String> {
    let id = value.trim().strip_prefix(prefix)?.strip_suffix(suffix)?;
    (id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
    .then(|| id.to_owned())
}

/// `scripts.ini.wukong-lab-<id>.bak` and `gpt.ini.wukong-lab-<id>.bak`.
fn backup_marker_id(name: &str, file: &str, suffix: &str) -> Option<String> {
    let id = name
        .strip_prefix(file)?
        .strip_prefix(POLICY_BACKUP_SUFFIX)?
        .strip_suffix(suffix)?;
    (id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
    .then(|| id.to_owned())
}

/// Paths inspected on the powered-off system volume. Every field is an
/// absolute Windows guest path; host adapters translate it once for libguestfs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskInspectionRequest {
    pub system_hive: String,
    pub program_data: String,
    pub provision_root: String,
    pub service_prefix: String,
    pub gpo_root: String,
    pub scripts_dir: String,
    pub startup_dir: String,
    pub scripts_ini: String,
    pub scripts_ini_backup: String,
    pub gpt_ini: String,
    pub gpt_ini_backup: String,
}

impl DiskInspectionRequest {
    /// Probe exactly one operation's fixed paths so cleanup can read that
    /// operation's backups while provisioning reads a fresh (absent) pair.
    pub fn for_provision(paths: &GuestPaths) -> Self {
        Self {
            system_hive: GUEST_SYSTEM_HIVE.into(),
            program_data: GUEST_PROGRAM_DATA.into(),
            provision_root: GUEST_PROVISION_ROOT.into(),
            service_prefix: "WukongLabProvision".into(),
            gpo_root: GUEST_GPO_ROOT.into(),
            scripts_dir: GUEST_SCRIPTS_DIR.into(),
            startup_dir: GUEST_STARTUP_DIR.into(),
            scripts_ini: paths.scripts_ini.clone(),
            scripts_ini_backup: paths.scripts_ini_backup.clone(),
            gpt_ini: paths.gpt_ini.clone(),
            gpt_ini_backup: paths.gpt_ini_backup.clone(),
        }
    }
}

/// Removal of the legacy one-shot provisioning service from a powered-off
/// SYSTEM hive. New provisioning never writes it; this only cleans artifacts of
/// attempts made by the earlier service-based boot mechanism.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceRemoval {
    /// Absolute Windows guest path of the SYSTEM hive inside the system volume.
    pub system_hive: String,
    pub service: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceRemovalEvidence {
    pub service_present: bool,
    pub removed: bool,
}

/// Files and directories removed from the powered-off guest by the explicit
/// cleanup command. Paths are fixed by the provision identity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestRemoval {
    pub directories: Vec<String>,
    pub files: Vec<String>,
}

impl GuestRemoval {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.directories.len() + self.files.len() <= 32,
            "guest cleanup names too many paths"
        );
        for path in self.directories.iter().chain(self.files.iter()) {
            GuestPaths::guestfish(path)?;
        }
        Ok(())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.directories.is_empty() && self.files.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestRemovalEvidence {
    /// Files that were present and are gone afterwards.
    pub removed_files: Vec<String>,
    /// Directories that were present and are gone afterwards.
    pub removed_directories: Vec<String>,
    /// Paths that were already absent.
    pub absent: Vec<String>,
}

/// Fixed guest artifacts read back after the one-shot startup script ran. Every
/// path field is an absolute Windows guest path; adapters translate them once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestObservationRequest {
    pub wrapper: String,
    pub plan: String,
    pub receipt: String,
    pub startup_script: String,
    pub startup_entry: String,
    pub scripts_ini: String,
    pub scripts_ini_backup: String,
    pub gpt_ini_backup: String,
}

impl GuestObservationRequest {
    pub fn for_provision(paths: &GuestPaths) -> Self {
        Self {
            wrapper: paths.wrapper.clone(),
            plan: paths.plan.clone(),
            receipt: paths.receipt.clone(),
            startup_script: paths.startup_script.clone(),
            startup_entry: paths.startup_entry.clone(),
            scripts_ini: paths.scripts_ini.clone(),
            scripts_ini_backup: paths.scripts_ini_backup.clone(),
            gpt_ini_backup: paths.gpt_ini_backup.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestObservation {
    pub receipt: Option<Vec<u8>>,
    pub wrapper_present: bool,
    pub plan_present: bool,
    /// The local-GPO startup launcher must be gone after the wrapper runs.
    pub startup_script_present: bool,
    pub scripts_ini_present: bool,
    /// The injected `[Startup]` command line is still present in `scripts.ini`.
    pub startup_entry_present: bool,
    pub scripts_ini_backup_present: bool,
    pub gpt_ini_backup_present: bool,
}

/// Host-side offline disk editing. Implementations execute fixed libguestfs and
/// hivexsh commands through operator-configured WSL tooling; tests substitute an
/// in-memory fake so no real VM is ever touched.
#[async_trait]
pub trait OfflineDisk: Send + Sync {
    async fn inspect(&self, disk: &Path, request: &DiskInspectionRequest)
    -> Result<DiskInspection>;
    async fn write_guest_files(&self, disk: &Path, files: &[GuestFile]) -> Result<()>;
    /// Delete the fixed legacy one-shot service key if it is present.
    async fn remove_service_key(
        &self,
        disk: &Path,
        removal: &ServiceRemoval,
    ) -> Result<ServiceRemovalEvidence>;
    /// Remove only the named startup launcher, backups and provision directory.
    async fn remove_guest_paths(
        &self,
        disk: &Path,
        removal: &GuestRemoval,
    ) -> Result<GuestRemovalEvidence>;
    async fn observe_guest(
        &self,
        disk: &Path,
        request: &GuestObservationRequest,
    ) -> Result<GuestObservation>;
}

/// Builds the `WUKONG_OFFLINE` ISO image from finalized media.
#[async_trait]
pub trait IsoBuilder: Send + Sync {
    /// The configured builder leaves the executable fixed. `--iso-tool` may
    /// override it for one command without changing configuration.
    fn with_executable(&self, executable: &Path) -> Result<Arc<dyn IsoBuilder>>;
    async fn build(&self, media: &Path, output: &Path) -> Result<()>;
}

/// Reports the guest's IPv4 address from the hypervisor.
#[async_trait]
pub trait GuestNetwork: Send + Sync {
    async fn guest_ip(&self) -> Result<String>;
}

/// Builds finalized media from a released template by invoking the repository's
/// `python -m scripts.worker.media finalize` module.
#[async_trait]
pub trait MediaFinalizer: Send + Sync {
    async fn finalize(&self, request: &FinalizeRequest) -> Result<serde_json::Value>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalizeRequest {
    pub template: PathBuf,
    pub keys: PathBuf,
    pub spec: PathBuf,
    pub output: PathBuf,
    pub generate_ssh_key: Option<PathBuf>,
}

/// Controller-side environment registration and doctor entry used only by
/// `lab provision --register/--verify-environment`.
#[async_trait]
pub trait ProvisionEnvironment: Send + Sync {
    /// Persist the verified connection as a managed-SSH environment.
    fn register(
        &self,
        connection: &PublicConnection,
        media: &MediaIdentity,
        guest_ip: &str,
        identity_file: &Path,
    ) -> Result<serde_json::Value>;
    /// Connect to the installed worker once the VM is reachable.
    async fn verify(&self, environment_id: &str) -> Result<serde_json::Value>;
}

/// Public worker connection receipt emitted by the offline installer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicConnection {
    pub schema: u32,
    pub installed: bool,
    pub installation_id: String,
    pub environment_id: String,
    pub generation: u64,
    pub manifest_sha256: String,
    pub ssh_user: String,
    pub ssh_port: u16,
    pub host_key: SshKey,
    pub host_fingerprint: String,
    pub production_isolation_verified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptExpectation {
    pub provision_id: String,
    pub media: MediaIdentity,
}

/// Operator-supplied `finalize` enrollment specification (media.py schema 1).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FinalizeSpec {
    pub schema: u32,
    pub installation_id: String,
    pub environment_id: String,
    #[serde(default = "default_generation")]
    pub generation: u64,
    pub management_peer: String,
    pub ssh_port: u16,
    #[serde(default = "default_execution_boundary")]
    pub execution_boundary: String,
    #[serde(default)]
    pub ssh_public_key: Option<SshKey>,
}

fn default_generation() -> u64 {
    1
}

fn default_execution_boundary() -> String {
    "disposable_vm".into()
}

impl FinalizeSpec {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema == 1, "unsupported finalize spec schema");
        ensure!(
            lowercase_hex(&self.installation_id, 32),
            "finalize spec installation_id must be 32 lowercase hex characters"
        );
        crate::adapters::environments::validate_id(&self.environment_id)?;
        ensure!(
            self.generation >= 1,
            "finalize spec generation must be positive"
        );
        validate_management_peer(&self.management_peer)?;
        ensure!(self.ssh_port > 0, "finalize spec ssh_port must be positive");
        ensure!(
            self.execution_boundary == "disposable_vm",
            "offline media requires the disposable_vm execution boundary"
        );
        if let Some(key) = &self.ssh_public_key {
            key.validate()?;
        }
        Ok(())
    }
}

pub fn read_finalize_spec(path: &Path) -> Result<FinalizeSpec> {
    ensure!(
        path.is_absolute() && path.is_file(),
        "finalize spec must be an absolute existing file"
    );
    let bytes = read_bounded(path, 64 * 1024)?;
    let spec: FinalizeSpec =
        serde_json::from_slice(&bytes).context("parse offline media finalize spec")?;
    spec.validate()?;
    Ok(spec)
}

/// Planned snapshot transition before provisioning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotPlan {
    /// No snapshots exist; provisioning starts from the powered-off base disk
    /// and creates the baseline after the guest receipt verifies.
    Pristine,
    /// Only the configured baseline exists and `--replace-baseline` was passed.
    /// The caller persists its intent and backs up the VMX, deletes exactly that
    /// snapshot, and only then resolves and validates the committed boot disk.
    ReplaceBaseline,
}

/// A provisioned VM must be powered off with no snapshot chain. The only
/// tolerated snapshot is the configured baseline when it is being replaced;
/// no other snapshot is ever deleted.
///
/// `--replace-baseline` is explicitly a no-op when no snapshot exists: the
/// pristine flow already creates the baseline after the guest receipt verifies,
/// so the caller keeps working and only logs that the flag had no effect.
pub fn plan_snapshot(
    snapshots: &[String],
    baseline: &str,
    replace_baseline: bool,
) -> Result<SnapshotPlan> {
    if snapshots.is_empty() {
        return Ok(SnapshotPlan::Pristine);
    }
    ensure!(
        snapshots.len() == 1 && snapshots[0] == baseline,
        "lab VM must have no snapshots before provisioning; found {snapshots:?} (only the configured baseline {baseline} may be replaced with --replace-baseline)"
    );
    ensure!(
        replace_baseline,
        "baseline snapshot {baseline} already exists; pass --replace-baseline to delete and recreate it"
    );
    Ok(SnapshotPlan::ReplaceBaseline)
}

/// Finalized media identity re-verified before the guest disk is touched.
pub fn verify_media(root: &Path) -> Result<MediaIdentity> {
    ensure!(
        root.is_absolute(),
        "offline media directory must be absolute"
    );
    crate::adapters::files::safety::safe(root)?;
    let metadata = fs::symlink_metadata(root)
        .with_context(|| format!("offline media directory is missing: {}", root.display()))?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "offline media must be a directory, not a link"
    );
    reject_links(root)?;
    let record_bytes = read_bounded(&root.join(MEDIA_RECORD), 1024 * 1024)?;
    let record: MediaRecord =
        serde_json::from_slice(&record_bytes).context("parse offline media receipt")?;
    ensure!(
        record.schema == 1,
        "unsupported offline media receipt schema"
    );
    semver::Version::parse(&record.version).context("invalid offline media version")?;
    let key = wukong_worker_package::PublicKey::from_hex(&record.controller_public_key)
        .context("invalid offline media controller public key")?;
    ensure!(
        lowercase_hex(&record.manifest_sha256, 64)
            && lowercase_hex(&record.setup_sha256, 64)
            && lowercase_hex(&record.enrollment_sha256, 64),
        "offline media receipt contains invalid hashes"
    );
    ensure!(
        lowercase_hex(&record.source_revision, 40),
        "offline media receipt contains an invalid source revision"
    );
    ensure!(
        lowercase_hex(&record.installation_id, 32),
        "offline media receipt contains an invalid installation identity"
    );
    crate::adapters::environments::validate_id(&record.environment_id)?;
    ensure!(record.generation >= 1, "invalid offline media generation");

    let setup = read_bounded(&root.join(SETUP_EXECUTABLE), MAX_MEDIA_BYTES)?;
    ensure!(
        sha256(&setup) == record.setup_sha256,
        "offline media setup executable differs from the receipt"
    );
    let enrollment_bytes = read_bounded(&root.join(ENROLLMENT_RECORD), 64 * 1024)?;
    ensure!(
        sha256(&enrollment_bytes) == record.enrollment_sha256,
        "offline media enrollment record differs from the receipt"
    );
    let manifest_bytes = read_bounded(&root.join(MANIFEST_RECORD), MAX_MANIFEST_BYTES)?;
    ensure!(
        sha256(&manifest_bytes) == record.manifest_sha256,
        "offline media payload manifest differs from the receipt"
    );
    let setup_record: SetupRecord =
        serde_json::from_slice(&read_bounded(&root.join(SETUP_RECORD), 1024 * 1024)?)
            .context("parse offline media setup record")?;
    ensure!(
        setup_record.schema == 1
            && setup_record.controller_public_key == record.controller_public_key
            && setup_record.controller_version == record.version,
        "offline media setup record differs from the receipt"
    );

    let signature: wukong_worker_package::SignatureRecord =
        serde_json::from_slice(&read_bounded(&root.join(ENROLLMENT_SIGNATURE), 16 * 1024)?)
            .context("parse offline media enrollment signature")?;
    let verified =
        wukong_worker_package::VerifiedEnrollment::verify(&enrollment_bytes, &signature, &key)
            .context("offline media enrollment signature is invalid")?;
    let enrollment = verified.enrollment();
    ensure!(
        enrollment.schema == 1
            && enrollment.installation_id == record.installation_id
            && enrollment.environment_id == record.environment_id
            && enrollment.generation == record.generation
            && enrollment.controller_public_key == record.controller_public_key
            && enrollment.payload_manifest_sha256 == record.manifest_sha256
            && enrollment.execution_boundary
                == wukong_worker_package::ExecutionBoundary::DisposableVm,
        "offline media enrollment differs from the receipt"
    );
    let manifest_signature: wukong_worker_package::SignatureRecord =
        serde_json::from_slice(&read_bounded(&root.join(MANIFEST_SIGNATURE), 16 * 1024)?)
            .context("parse offline media payload signature")?;
    let payload =
        wukong_worker_package::VerifiedManifest::verify(&manifest_bytes, &manifest_signature, &key)
            .context("offline media payload signature is invalid")?;
    verified
        .bind_payload(&payload)
        .context("offline media enrollment and payload differ")?;

    let ssh_public_key = SshKey::from_public(&enrollment.ssh_public_key)?;
    let ssh_user = format!("wkgm{}", &record.installation_id[..16]);
    let media = MediaIdentity {
        media_root: root.to_path_buf(),
        media_record_sha256: sha256(&record_bytes),
        setup_sha256: record.setup_sha256,
        enrollment_sha256: record.enrollment_sha256,
        manifest_sha256: record.manifest_sha256,
        controller_public_key: record.controller_public_key,
        controller_version: record.version,
        installation_id: record.installation_id,
        environment_id: record.environment_id,
        generation: record.generation,
        ssh_port: enrollment.ssh_port,
        ssh_user,
        management_peer: enrollment.management_peer.to_string(),
        ssh_public_key,
    };
    media.validate()?;
    Ok(media)
}

impl MediaIdentity {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            lowercase_hex(&self.media_record_sha256, 64)
                && lowercase_hex(&self.setup_sha256, 64)
                && lowercase_hex(&self.enrollment_sha256, 64)
                && lowercase_hex(&self.manifest_sha256, 64)
                && lowercase_hex(&self.controller_public_key, 64),
            "media identity contains invalid hashes"
        );
        ensure!(
            lowercase_hex(&self.installation_id, 32),
            "media identity contains an invalid installation identity"
        );
        ensure!(
            self.ssh_user == format!("wkgm{}", &self.installation_id[..16]),
            "media identity management user differs from the installation identity"
        );
        crate::adapters::environments::validate_id(&self.environment_id)?;
        semver::Version::parse(&self.controller_version)
            .context("media identity has an invalid controller version")?;
        ensure!(self.generation >= 1, "media identity generation is invalid");
        ensure!(self.ssh_port > 0, "media identity ssh port is invalid");
        validate_management_peer(&self.management_peer)?;
        self.ssh_public_key.validate()?;
        Ok(())
    }

    /// Cross-check the media against the operator's finalize specification and
    /// the generated controller SSH identity.
    pub fn check_finalize_spec(&self, spec: &FinalizeSpec) -> Result<()> {
        ensure!(
            self.installation_id == spec.installation_id
                && self.environment_id == spec.environment_id
                && self.generation == spec.generation
                && self.ssh_port == spec.ssh_port
                && self.management_peer == spec.management_peer,
            "finalized media differs from the finalize spec"
        );
        if let Some(key) = &spec.ssh_public_key {
            ensure!(
                self.ssh_public_key == *key,
                "finalized media enrollment differs from the finalize spec SSH key"
            );
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaRecord {
    schema: u32,
    version: String,
    controller_public_key: String,
    manifest_sha256: String,
    setup_sha256: String,
    enrollment_sha256: String,
    installation_id: String,
    environment_id: String,
    generation: u64,
    source_revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupRecord {
    schema: u32,
    controller_public_key: String,
    controller_version: String,
}

/// A disposable VM boot disk resolved from the registered VMX.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestDisk {
    pub path: PathBuf,
    pub device: String,
}

/// Resolve the boot disk from the VMX. Delta, linked, foreign or non-local
/// disks are rejected: offline injection must address exactly one full disk.
pub fn resolve_guest_disk(vmx: &str, vmx_path: &Path) -> Result<GuestDisk> {
    ensure!(
        vmx_path.is_absolute() && vmx_path.is_file(),
        "registered VMX must be an absolute existing file"
    );
    let vmx_path = fs::canonicalize(vmx_path)?;
    let vmx_dir = vmx_path
        .parent()
        .context("registered VMX has no directory")?;
    for device in ["nvme0:0", "scsi0:0", "sata0:0"] {
        let key = format!("{device}.fileName");
        let Some(entry) = vmx_entry(vmx, &key) else {
            continue;
        };
        let name = unquote(entry);
        ensure!(!name.is_empty(), "{device} has an empty backing filename");
        ensure!(
            !name.starts_with(r"\\") && !name.starts_with("//"),
            "guest disk must be a local file"
        );
        ensure!(
            !name.split(['\\', '/']).any(|part| part == ".."),
            "guest disk path may not escape the VMX directory"
        );
        let mut candidate = PathBuf::from(name);
        if candidate.is_relative() {
            candidate = vmx_dir.join(&candidate);
        }
        candidate = fs::canonicalize(&candidate)
            .with_context(|| format!("guest disk {name} is missing"))?;
        ensure!(
            candidate.starts_with(vmx_dir),
            "guest disk must live inside the VMX directory"
        );
        ensure!(candidate != vmx_path, "guest disk names the VMX itself");
        crate::adapters::files::safety::safe(&candidate)?;
        let metadata = fs::symlink_metadata(&candidate)?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "guest disk must be an ordinary file"
        );
        let file_name = candidate
            .file_name()
            .map(|value| value.to_string_lossy().to_ascii_lowercase())
            .context("guest disk has no file name")?;
        ensure!(
            file_name.ends_with(".vmdk"),
            "guest disk must be a VMDK file"
        );
        ensure!(
            !delta_disk(&file_name),
            "guest disk is a snapshot delta; provisioning requires a VM with no snapshot chain"
        );
        return Ok(GuestDisk {
            path: candidate,
            device: device.into(),
        });
    }
    bail!("VMX declares no boot disk on nvme0:0, scsi0:0 or sata0:0")
}

fn delta_disk(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".vmdk") else {
        return false;
    };
    stem.rsplit_once('-').is_some_and(|(_, suffix)| {
        suffix.len() == 6 && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// Result of a planned VMX device edit; `text` is the complete replacement file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VmxEdit {
    pub text: String,
    pub device: String,
}

/// Attach the offline ISO to one free dedicated SATA CD device. The caller
/// backs up the original bytes first and restores them on every exit path.
pub fn attach_offline_iso(vmx: &str, iso: &Path) -> Result<VmxEdit> {
    ensure!(iso.is_absolute(), "offline ISO path must be absolute");
    let iso_text = iso.to_str().context("offline ISO path must be UTF-8")?;
    ensure!(
        !iso_text.contains(['"', '\n', '\r']),
        "offline ISO path contains an unsupported character"
    );
    ensure!(
        vmx_entry(vmx, "sata0.present")
            .is_some_and(|value| unquote(value).eq_ignore_ascii_case("TRUE")),
        "VMX has no enabled SATA controller; offline provisioning attaches its ISO there"
    );
    let slot = dedicated_cdrom_slot(vmx)?;
    let device = format!("sata0:{slot}");
    let desired = [
        (format!("{device}.present"), "\"TRUE\"".to_owned()),
        (format!("{device}.deviceType"), "\"cdrom-image\"".to_owned()),
        (format!("{device}.fileName"), format!("\"{iso_text}\"")),
        (format!("{device}.startConnected"), "\"TRUE\"".to_owned()),
    ];
    let newline = if vmx.contains("\r\n") { "\r\n" } else { "\n" };
    let mut matched = [false; 4];
    let mut text = String::with_capacity(vmx.len() + 256);
    for line in vmx.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let terminator = &line[body.len()..];
        let replacement = vmx_entry_key(body).and_then(|key| {
            desired
                .iter()
                .position(|(name, _)| name == key)
                .map(|index| (index, key))
        });
        if let Some((index, key)) = replacement {
            matched[index] = true;
            text.push_str(&format!("{key} = {}{terminator}", desired[index].1));
        } else {
            text.push_str(line);
        }
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push_str(newline);
    }
    for (index, (key, value)) in desired.iter().enumerate() {
        if !matched[index] {
            text.push_str(&format!("{key} = {value}{newline}"));
        }
    }
    Ok(VmxEdit { text, device })
}

/// Pick an existing empty CD device, otherwise the first unmentioned SATA slot.
fn dedicated_cdrom_slot(vmx: &str) -> Result<u32> {
    let mut cdrom = None;
    for slot in 0..30_u32 {
        let prefix = format!("sata0:{slot}.");
        let present = vmx_entry(vmx, &format!("{prefix}present"))
            .map(unquote)
            .is_some_and(|value| value.eq_ignore_ascii_case("TRUE"));
        if !present {
            let device_type = vmx_entry(vmx, &format!("{prefix}deviceType")).map(unquote);
            if device_type.is_some_and(|value| value.starts_with("cdrom")) {
                return Ok(slot);
            }
            if device_type.is_none()
                && vmx_entry(vmx, &format!("{prefix}fileName")).is_none()
                && cdrom.is_none()
            {
                cdrom = Some(slot);
            }
        }
    }
    if let Some(slot) = cdrom {
        return Ok(slot);
    }
    for slot in 1..30_u32 {
        let prefix = format!("sata0:{slot}.");
        if !vmx.contains(prefix.as_str()) {
            return Ok(slot);
        }
    }
    bail!("VMX has no free SATA device for the offline media")
}

fn vmx_entry_key(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    line.split_once('=')
        .map(|(key, _)| key.trim())
        .filter(|key| !key.is_empty())
}

fn vmx_entry<'a>(vmx: &'a str, wanted: &str) -> Option<&'a str> {
    vmx.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == wanted).then(|| value.trim())
    })
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(value)
}

pub fn validate_service_name(service: &str) -> Result<()> {
    ensure!(
        !service.is_empty()
            && service.len() <= 128
            && service
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid one-shot service name"
    );
    Ok(())
}

/// `hivexsh -w` script that deletes one fixed legacy service key. New
/// provisioning never writes this key; the explicit guest cleanup command uses
/// it to remove artifacts of the earlier service-based boot mechanism.
pub fn hive_remove_service_script(control_set: u32, service: &str) -> Result<String> {
    ensure!(
        (1..=999).contains(&control_set),
        "invalid Windows control set number"
    );
    validate_service_name(service)?;
    Ok(format!(
        "cd \\ControlSet{control_set:03}\\Services\ndel {service}\ncommit\nquit\n"
    ))
}

/// One decoded INI document in its original text encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PolicyEncoding {
    Utf8,
    Utf16Le,
}

fn decode_policy_document(bytes: &[u8], what: &str) -> Result<(PolicyEncoding, String)> {
    ensure!(
        bytes.len() as u64 <= MAX_POLICY_BYTES,
        "{what} exceeds the policy size budget"
    );
    if let Some(body) = bytes.strip_prefix(&UTF16_LE_BOM) {
        ensure!(
            body.len() % 2 == 0,
            "{what} has an odd UTF-16LE byte length"
        );
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let text =
            String::from_utf16(&units).with_context(|| format!("{what} is not valid UTF-16LE"))?;
        return Ok((PolicyEncoding::Utf16Le, text));
    }
    let body = bytes.strip_prefix(&UTF8_BOM).unwrap_or(bytes);
    let text = std::str::from_utf8(body)
        .with_context(|| format!("{what} is not valid UTF-16LE/UTF-8 text"))?;
    ensure!(
        !text.contains('\0'),
        "{what} has no supported policy text encoding"
    );
    Ok((PolicyEncoding::Utf8, text.to_owned()))
}

fn encode_policy_document(encoding: PolicyEncoding, text: &str) -> Vec<u8> {
    match encoding {
        PolicyEncoding::Utf8 => text.as_bytes().to_vec(),
        PolicyEncoding::Utf16Le => {
            let mut bytes = UTF16_LE_BOM.to_vec();
            bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
            bytes
        }
    }
}

fn policy_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_owned())
        .collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

fn policy_newline(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

fn render_policy(lines: &[String], newline: &str) -> String {
    let mut text = lines.join(newline);
    if !lines.is_empty() {
        text.push_str(newline);
    }
    text
}

/// Half-open line range of one `[Section]` body: `(header, end)`.
fn section_bounds(lines: &[String], section: &str) -> Option<(usize, usize)> {
    let mut start = None;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.len() < 2 || !trimmed.starts_with('[') || !trimmed.ends_with(']') {
            continue;
        }
        if let Some(header) = start {
            return Some((header, index));
        }
        if trimmed[1..trimmed.len() - 1]
            .trim()
            .eq_ignore_ascii_case(section)
        {
            start = Some(index);
        }
    }
    start.map(|header| (header, lines.len()))
}

fn startup_entries_from_lines(lines: &[String]) -> Vec<String> {
    let Some((header, end)) = section_bounds(lines, "Startup") else {
        return Vec::new();
    };
    lines[header + 1..end]
        .iter()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            let index = key.strip_suffix("CmdLine")?;
            (!index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| value.trim().to_owned())
        })
        .collect()
}

/// Parse the `[Startup]` command lines from existing `scripts.ini` bytes. An
/// unreadable document is an error: the controller never guesses at policy
/// content it cannot preserve.
pub fn startup_command_lines(bytes: &[u8]) -> Result<Vec<String>> {
    let (_, text) = decode_policy_document(bytes, "guest scripts.ini")?;
    Ok(startup_entries_from_lines(&policy_lines(&text)))
}

fn validate_startup_entry(entry: &str) -> Result<()> {
    ensure!(
        entry.len() == STARTUP_ENTRY_PREFIX.len() + 32 + STARTUP_ENTRY_SUFFIX.len()
            && entry.starts_with(STARTUP_ENTRY_PREFIX)
            && entry.ends_with(STARTUP_ENTRY_SUFFIX),
        "invalid machine startup entry"
    );
    let id = &entry[STARTUP_ENTRY_PREFIX.len()..entry.len() - STARTUP_ENTRY_SUFFIX.len()];
    ensure!(
        id.len() == 32
            && id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "invalid machine startup entry"
    );
    Ok(())
}

fn next_script_index(lines: &[String]) -> u32 {
    lines
        .iter()
        .filter_map(|line| {
            let (key, _) = line.split_once('=')?;
            let key = key.trim();
            let index = key
                .strip_suffix("CmdLine")
                .or_else(|| key.strip_suffix("Parameters"))?;
            index.parse::<u32>().ok()
        })
        .max()
        .map_or(0, |value| value.saturating_add(1))
}

/// Inject one `[Startup]` entry while preserving every existing section. The
/// previous bytes are backed up by the caller before this rewritten document is
/// published, and the wrapper restores that backup after it runs.
pub fn inject_startup_script(original: Option<&[u8]>, entry: &str) -> Result<Vec<u8>> {
    validate_startup_entry(entry)?;
    let (encoding, text) = match original {
        Some(bytes) => decode_policy_document(bytes, "guest scripts.ini")?,
        None => (PolicyEncoding::Utf8, String::new()),
    };
    let newline = if text.is_empty() {
        "\r\n"
    } else {
        policy_newline(&text)
    };
    let mut lines = policy_lines(&text);
    ensure!(
        startup_entries_from_lines(&lines)
            .iter()
            .all(|line| !line.starts_with(STARTUP_ENTRY_PREFIX)),
        "guest scripts.ini already contains a provisioning startup entry"
    );
    match section_bounds(&lines, "Startup") {
        Some((header, end)) => {
            let next = next_script_index(&lines[header + 1..end]);
            let mut insert = end;
            while insert > header + 1 && lines[insert - 1].trim().is_empty() {
                insert -= 1;
            }
            lines.insert(insert, format!("{next}Parameters="));
            lines.insert(insert, format!("{next}CmdLine={entry}"));
        }
        None => {
            if !lines.is_empty() && !lines.last().is_some_and(|line| line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push("[Startup]".into());
            lines.push(format!("0CmdLine={entry}"));
            lines.push("0Parameters=".into());
        }
    }
    Ok(encode_policy_document(
        encoding,
        &render_policy(&lines, newline),
    ))
}

/// Remove this operation's `[Startup]` entry (and its parameters line) while
/// preserving unrelated sections. `None` means the document became empty and the
/// injected `scripts.ini` itself should be deleted.
pub fn remove_startup_script(original: &[u8], entry: &str) -> Result<Option<Vec<u8>>> {
    validate_startup_entry(entry)?;
    let (encoding, text) = decode_policy_document(original, "guest scripts.ini")?;
    let newline = policy_newline(&text);
    let mut lines = policy_lines(&text);
    let Some((header, end)) = section_bounds(&lines, "Startup") else {
        return Ok(Some(original.to_vec()));
    };
    let mut found = None;
    for (line, content) in lines.iter().enumerate().take(end).skip(header + 1) {
        let Some((key, value)) = content.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let Some(number) = key.strip_suffix("CmdLine") else {
            continue;
        };
        if !number.is_empty()
            && number.bytes().all(|byte| byte.is_ascii_digit())
            && value.trim() == entry
        {
            found = Some((line, number.to_owned()));
            break;
        }
    }
    let Some((line, number)) = found else {
        return Ok(Some(original.to_vec()));
    };
    let parameters = format!("{number}Parameters=");
    let mut remove = vec![line];
    if let Some(index) = (header + 1..end).find(|index| lines[*index].trim() == parameters) {
        remove.push(index);
    }
    remove.sort_unstable();
    remove.dedup();
    for index in remove.into_iter().rev() {
        lines.remove(index);
    }
    if let Some((header, end)) = section_bounds(&lines, "Startup")
        && lines[header + 1..end]
            .iter()
            .all(|line| line.trim().is_empty())
    {
        lines.drain(header..end);
    }
    if lines.iter().all(|line| line.trim().is_empty()) {
        return Ok(None);
    }
    Ok(Some(encode_policy_document(
        encoding,
        &render_policy(&lines, newline),
    )))
}

fn parse_version_line(value: &str) -> Option<&str> {
    let rest = value.trim().strip_prefix("Version")?;
    rest.trim_start().strip_prefix('=').map(str::trim)
}

/// Bump the computer half of the local-GPO `Version` DWORD, creating a minimal
/// `gpt.ini` when the guest has none. The original bytes are backed up by the
/// caller and restored by the wrapper; the extension list is left untouched
/// because machine startup scripts are a standard local-GPO extension and the
/// recovered working layout only changed `scripts.ini` plus the startup folder.
pub fn bump_gpt_version(original: Option<&[u8]>) -> Result<Vec<u8>> {
    let (encoding, text) = match original {
        Some(bytes) => decode_policy_document(bytes, "guest gpt.ini")?,
        None => (PolicyEncoding::Utf8, String::new()),
    };
    let newline = if text.is_empty() {
        "\r\n"
    } else {
        policy_newline(&text)
    };
    let mut lines = policy_lines(&text);
    let mut bumped = false;
    if let Some((header, end)) = section_bounds(&lines, "General") {
        for content in lines.iter_mut().take(end).skip(header + 1) {
            let Some(version) = parse_version_line(content) else {
                continue;
            };
            ensure!(
                !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit()),
                "guest gpt.ini Version is not a decimal number"
            );
            let current: u32 = version
                .parse()
                .with_context(|| format!("guest gpt.ini Version is out of range: {version}"))?;
            let user = current & 0xffff_0000;
            let computer = current & 0xffff;
            let computer = if computer == 0xffff { 1 } else { computer + 1 };
            *content = format!("Version={}", user | computer);
            bumped = true;
            break;
        }
        if !bumped {
            let mut insert = end;
            while insert > header + 1 && lines[insert - 1].trim().is_empty() {
                insert -= 1;
            }
            lines.insert(insert, "Version=1".into());
        }
    } else {
        if !lines.is_empty() && !lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("[General]".into());
        lines.push("Version=1".into());
    }
    Ok(encode_policy_document(
        encoding,
        &render_policy(&lines, newline),
    ))
}

/// Local-GPO machine startup injection planned from the existing policy files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupInjection {
    /// Per-operation backups first, then the rewritten policy files.
    pub files: Vec<GuestFile>,
    pub scripts_ini_sha256: String,
    pub gpt_ini_sha256: String,
    pub scripts_ini_preexisting: bool,
    pub gpt_ini_preexisting: bool,
}

/// Plan the policy backups and the injected startup entry. The launcher itself
/// is part of the fixed provision intent and is written with the wrapper.
pub fn plan_startup_injection(
    paths: &GuestPaths,
    policy: &GuestPolicyFiles,
) -> Result<StartupInjection> {
    ensure!(
        policy.scripts_ini_backup.is_none() && policy.gpt_ini_backup.is_none(),
        "guest already contains backups for this operation; clean the previous attempt before provisioning"
    );
    let scripts_preexisting = policy.scripts_ini.is_some();
    let gpt_preexisting = policy.gpt_ini.is_some();
    let mut files = Vec::new();
    if let Some(original) = &policy.scripts_ini {
        files.push(injected_file(&paths.scripts_ini_backup, original.clone()));
    }
    let scripts_ini = inject_startup_script(policy.scripts_ini.as_deref(), &paths.startup_entry)?;
    files.push(injected_file(&paths.scripts_ini, scripts_ini.clone()));
    if let Some(original) = &policy.gpt_ini {
        files.push(injected_file(&paths.gpt_ini_backup, original.clone()));
    }
    let gpt_ini = bump_gpt_version(policy.gpt_ini.as_deref())?;
    files.push(injected_file(&paths.gpt_ini, gpt_ini.clone()));
    Ok(StartupInjection {
        files,
        scripts_ini_sha256: sha256(&scripts_ini),
        gpt_ini_sha256: sha256(&gpt_ini),
        scripts_ini_preexisting: scripts_preexisting,
        gpt_ini_preexisting: gpt_preexisting,
    })
}

/// Fixed `.cmd` launcher executed by the machine startup script. It starts the
/// wrapper detached so the Group Policy startup phase is not held open for the
/// whole worker installation; the wrapper powers the guest off when it finishes.
pub fn startup_launcher(provision_id: &str, wrapper: &str) -> Result<Vec<u8>> {
    let entry = format!("{STARTUP_ENTRY_PREFIX}{provision_id}{STARTUP_ENTRY_SUFFIX}");
    validate_startup_entry(&entry)?;
    ensure!(
        wrapper.starts_with(r"C:\ProgramData\WukongLab\provision\")
            && wrapper.is_ascii()
            && !wrapper.contains(['\n', '\r', '\0', '"']),
        "invalid guest wrapper path"
    );
    let launcher = format!(
        "@echo off\r\nrem Wukong offline worker provisioning launcher. The controller removes this file after the wrapper runs.\r\nstart \"\" /b \"{POWERSHELL}\" -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{wrapper}\"\r\n"
    );
    ensure!(
        launcher.len() <= 4096,
        "machine startup launcher exceeds the guest bound"
    );
    Ok(launcher.into_bytes())
}

/// One explicit cleanup action for the named operation's powered-off guest disk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestCleanupPlan {
    /// Restored policy files to write through the offline disk adapter.
    pub files: Vec<GuestFile>,
    /// Files removed after the restore (backups and the startup launcher).
    pub files_to_remove: Vec<String>,
    /// Directories removed recursively (only this operation's provision tree).
    pub directories_to_remove: Vec<String>,
    /// Legacy one-shot service key from an earlier attempt.
    pub service_key: Option<String>,
    /// Bounded human-readable action list shared by the command and its dry run.
    pub actions: Vec<String>,
}

impl GuestCleanupPlan {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
            && self.files_to_remove.is_empty()
            && self.directories_to_remove.is_empty()
            && self.service_key.is_none()
    }
}

/// Plan removal of exactly one operation's guest leftovers: the startup
/// launcher and policy entry, the per-operation policy backups, the legacy
/// service key written by earlier attempts, and the provision directory. Other
/// policy content, other operation directories and unrelated services are never
/// named here.
pub fn plan_guest_cleanup(
    paths: &GuestPaths,
    inspection: &DiskInspection,
) -> Result<GuestCleanupPlan> {
    ensure!(
        inspection.windows_volume && inspection.system_hive_present,
        "guest disk has no inspectable Windows system volume"
    );
    let mut plan = GuestCleanupPlan::default();
    let policy = &inspection.policy;
    let startup_present = policy
        .startup_directory_entries
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&paths.startup_entry));
    if startup_present {
        plan.files_to_remove.push(paths.startup_script.clone());
        plan.actions.push(format!(
            "delete the injected machine startup launcher {}",
            paths.startup_script
        ));
    }
    if let Some(backup) = &policy.scripts_ini_backup {
        plan.files
            .push(injected_file(&paths.scripts_ini, backup.clone()));
        plan.files_to_remove.push(paths.scripts_ini_backup.clone());
        plan.actions.push(format!(
            "restore the previous machine scripts.ini bytes from {}",
            paths.scripts_ini_backup
        ));
    } else if policy
        .startup_command_lines
        .iter()
        .any(|line| line == &paths.startup_entry)
    {
        let original = policy
            .scripts_ini
            .as_ref()
            .context("guest scripts.ini is missing while its startup entry is present")?;
        match remove_startup_script(original, &paths.startup_entry)? {
            Some(bytes) => {
                plan.files.push(injected_file(&paths.scripts_ini, bytes));
                plan.actions.push(format!(
                    "remove the startup entry from {}",
                    paths.scripts_ini
                ));
            }
            None => {
                plan.files_to_remove.push(paths.scripts_ini.clone());
                plan.actions
                    .push(format!("delete the injected {}", paths.scripts_ini));
            }
        }
    }
    if let Some(backup) = &policy.gpt_ini_backup {
        plan.files
            .push(injected_file(&paths.gpt_ini, backup.clone()));
        plan.files_to_remove.push(paths.gpt_ini_backup.clone());
        plan.actions.push(format!(
            "restore the previous local Group Policy gpt.ini bytes from {}",
            paths.gpt_ini_backup
        ));
    }
    if inspection.existing_service.as_deref() == Some(paths.service.as_str()) {
        plan.service_key = Some(paths.service.clone());
        plan.actions.push(format!(
            "delete the legacy one-shot service key {} from the SYSTEM hive",
            paths.service
        ));
    }
    if inspection
        .provision_root_entries
        .iter()
        .any(|entry| entry == paths.provision_id())
    {
        plan.directories_to_remove.push(paths.root.clone());
        plan.actions
            .push(format!("delete the operation directory {}", paths.root));
    }
    let removal = GuestRemoval {
        directories: plan.directories_to_remove.clone(),
        files: plan.files_to_remove.clone(),
    };
    removal.validate()?;
    Ok(plan)
}

/// JSON plan and the fixed PowerShell wrapper injected under the provision
/// directory. Identities come from verified media only; the wrapper itself is a
/// constant script that reads its plan and writes its receipt beside itself.
pub fn provision_plan(
    provision_id: &str,
    media: &MediaIdentity,
    paths: &GuestPaths,
) -> Result<Vec<u8>> {
    let plan = serde_json::json!({
        "schema": 2,
        "provision_id": provision_id,
        "installation_id": media.installation_id,
        "environment_id": media.environment_id,
        "generation": media.generation,
        "setup_sha256": media.setup_sha256,
        "enrollment_sha256": media.enrollment_sha256,
        "manifest_sha256": media.manifest_sha256,
        "controller_public_key": media.controller_public_key,
        "controller_version": media.controller_version,
        "ssh_port": media.ssh_port,
        "media_label": MEDIA_LABEL,
        "startup_script": paths.startup_script,
        "scripts_ini": paths.scripts_ini,
        "scripts_ini_backup": paths.scripts_ini_backup,
        "gpt_ini": paths.gpt_ini,
        "gpt_ini_backup": paths.gpt_ini_backup,
    });
    let mut bytes = serde_json::to_vec_pretty(&plan)?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= 16 * 1024,
        "provision plan exceeds the guest bound"
    );
    Ok(bytes)
}

pub const PROVISION_WRAPPER: &str = include_str!("provision_wrapper.ps1");

/// The guest receipt is the only evidence read back from the provisioned disk;
/// it must bind the fixed wrapper plan, installer identity and cleanup result.
pub fn verify_guest_receipt(
    bytes: &[u8],
    expectation: &ReceiptExpectation,
) -> Result<PublicConnection> {
    ensure!(
        bytes.len() as u64 <= MAX_RECEIPT_BYTES,
        "guest provisioning receipt exceeds the size budget"
    );
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let receipt: serde_json::Value =
        serde_json::from_slice(bytes).context("parse guest provisioning receipt")?;
    ensure!(
        receipt["schema"] == 2 && receipt["provision_id"] == expectation.provision_id,
        "guest provisioning receipt identity mismatch"
    );
    ensure!(
        receipt["installed"] == true && receipt["exit_code"] == 0,
        "guest worker installation did not succeed: {}",
        receipt["error"].as_str().unwrap_or("unknown error")
    );
    ensure!(
        receipt["error"].is_null(),
        "guest worker installation reported an error: {}",
        receipt["error"]
    );
    ensure!(
        matches!(
            receipt["startup_cleanup"].as_str(),
            Some("restored" | "removed")
        ),
        "guest machine startup policy was not restored: {}",
        receipt["startup_cleanup"]
    );
    ensure!(
        receipt["startup_script_deleted"] == true,
        "guest machine startup launcher was not deleted"
    );
    ensure!(
        receipt["cleanup_scheduled"] == true,
        "guest wrapper did not schedule its own cleanup"
    );
    let connection: PublicConnection = serde_json::from_value(
        receipt
            .get("connection")
            .cloned()
            .context("guest receipt has no connection record")?,
    )
    .context("parse public worker connection receipt")?;
    validate_connection(&connection, &expectation.media)?;
    Ok(connection)
}

pub(crate) fn validate_connection(
    connection: &PublicConnection,
    media: &MediaIdentity,
) -> Result<()> {
    ensure!(
        connection.schema == 1 && connection.installed,
        "public worker connection does not describe an installed worker"
    );
    ensure!(
        connection.installation_id == media.installation_id
            && connection.environment_id == media.environment_id
            && connection.generation == media.generation
            && connection.manifest_sha256 == media.manifest_sha256,
        "public worker connection identity differs from the offline media"
    );
    ensure!(
        connection.ssh_user == media.ssh_user && connection.ssh_port == media.ssh_port,
        "public worker connection SSH identity differs from the offline media"
    );
    connection.host_key.validate()?;
    ensure!(
        connection.host_key == media_ssh_key(media)?,
        "worker host key is not the media SSH identity"
    );
    ensure!(
        connection.host_fingerprint == connection.host_key.fingerprint()?,
        "worker host fingerprint does not match the public host key"
    );
    ensure!(
        !connection.production_isolation_verified,
        "guest receipts cannot certify hypervisor isolation"
    );
    Ok(())
}

fn media_ssh_key(media: &MediaIdentity) -> Result<SshKey> {
    media.ssh_public_key.validate()?;
    Ok(media.ssh_public_key.clone())
}

/// Controller-owned public host pin derived from the installation identity.
pub fn known_hosts_path(
    state_dir: &Path,
    installation_id: &str,
    host_key: &SshKey,
) -> Result<PathBuf> {
    ensure!(
        lowercase_hex(installation_id, 32),
        "invalid installation identity for the public host pin"
    );
    host_key.validate()?;
    let record = format!(
        "wukong-worker-{installation_id} {}\n",
        host_key.authorized_key()?
    );
    Ok(state_dir.join("environments").join(format!(
        "ssh-host-{}.known_hosts",
        sha256(record.as_bytes())
    )))
}

impl SshKey {
    pub fn from_public(key: &wukong_worker_package::SshPublicKey) -> Result<Self> {
        let key = Self {
            algorithm: key.algorithm.clone(),
            key: key.key.clone(),
        };
        key.validate()?;
        Ok(key)
    }

    pub fn validate(&self) -> Result<()> {
        let _ = self.public()?;
        Ok(())
    }

    pub fn public(&self) -> Result<wukong_worker_package::SshPublicKey> {
        let key = wukong_worker_package::SshPublicKey {
            algorithm: self.algorithm.clone(),
            key: self.key.clone(),
        };
        key.fingerprint()?;
        Ok(key)
    }

    pub fn fingerprint(&self) -> Result<String> {
        self.public()?.fingerprint()
    }

    pub fn authorized_key(&self) -> Result<String> {
        Ok(format!("{} {}", self.algorithm, self.key))
    }
}

/// Deterministic `C:\...` to guestfish path plus fixed file identity helpers
/// used by tests and the provision service.
pub fn injected_file(windows_path: &str, bytes: Vec<u8>) -> GuestFile {
    GuestFile {
        sha256: sha256(&bytes),
        windows_path: windows_path.into(),
        bytes,
    }
}

pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Stream a bounded file hash without loading large media into memory.
pub fn sha256_file(path: &Path) -> Result<(String, u64)> {
    crate::adapters::files::safety::safe(path)?;
    let metadata = fs::metadata(path)?;
    ensure!(metadata.is_file(), "hashed path is not a file");
    ensure!(
        metadata.len() <= MAX_MEDIA_BYTES,
        "hashed file exceeds the provisioning size budget"
    );
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut size = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        ensure!(
            size <= MAX_MEDIA_BYTES,
            "hashed file grew beyond the provisioning size budget"
        );
        hasher.update(&buffer[..read]);
    }
    Ok((hex::encode(hasher.finalize()), size))
}

pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    crate::adapters::files::safety::safe(path)?;
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() <= limit,
        "{} exceeds the provisioning size budget",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "{} grew beyond the provisioning size budget",
        path.display()
    );
    Ok(bytes)
}

fn reject_links(root: &Path) -> Result<()> {
    let mut entries = 0_usize;
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            entries += 1;
            ensure!(
                entries <= MAX_MEDIA_ENTRIES,
                "offline media contains too many entries"
            );
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "offline media contains a link: {}",
                path.display()
            );
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(
                    metadata.file_attributes() & 0x400 == 0,
                    "offline media contains a reparse point: {}",
                    path.display()
                );
            }
            if metadata.is_dir() {
                pending.push(path);
            } else {
                total = total
                    .checked_add(metadata.len())
                    .context("offline media size overflow")?;
                ensure!(
                    total <= MAX_MEDIA_BYTES,
                    "offline media exceeds the provisioning size budget"
                );
            }
        }
    }
    let allowed = [
        SETUP_EXECUTABLE,
        SETUP_RECORD,
        "payload",
        "enrollment",
        "recovery",
        MEDIA_RECORD,
    ];
    for entry in fs::read_dir(root)? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        ensure!(
            allowed.contains(&name.as_ref()),
            "offline media contains an unsupported entry: {name}"
        );
    }
    for required in [
        SETUP_EXECUTABLE,
        SETUP_RECORD,
        "payload",
        "enrollment",
        MEDIA_RECORD,
    ] {
        ensure!(
            root.join(required).exists(),
            "offline media is missing {required}"
        );
    }
    Ok(())
}

fn lowercase_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_management_peer(value: &str) -> Result<()> {
    let address: std::net::Ipv4Addr = value
        .parse()
        .context("management peer must be a canonical IPv4 address")?;
    ensure!(
        address.to_string() == value
            && !address.is_unspecified()
            && !address.is_loopback()
            && !address.is_multicast()
            && !address.is_broadcast(),
        "invalid management peer address"
    );
    Ok(())
}

/// Unknown components are rejected so persisted intents can never be replayed
/// against a different VMX or outside the controller state directory.
pub fn require_relative_path(path: &Path, base: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && path.starts_with(base),
        "provisioning artifact {} is outside {}",
        path.display(),
        base.display()
    );
    ensure!(
        !path
            .components()
            .any(|component| matches!(component, Component::ParentDir)),
        "provisioning artifact path may not contain parent components"
    );
    Ok(())
}

pub fn provision_paths(lab_root: &Path, provision_id: &str) -> Result<ProvisionArtifactPaths> {
    GuestPaths::for_provision(provision_id)?;
    ensure!(
        lab_root.is_absolute(),
        "lab state directory must be absolute"
    );
    let root = lab_root.join("provisions").join(provision_id);
    Ok(ProvisionArtifactPaths {
        root: root.clone(),
        vmx_backup: root.join("vmx.bak"),
        iso: root.join("offline.iso"),
        media: root.join("media"),
        injected: root.join("injected"),
        record: lab_root
            .join("provisions")
            .join(format!("{provision_id}.json")),
    })
}

#[derive(Clone, Debug)]
pub struct ProvisionArtifactPaths {
    pub root: PathBuf,
    pub vmx_backup: PathBuf,
    pub iso: PathBuf,
    pub media: PathBuf,
    pub injected: PathBuf,
    pub record: PathBuf,
}

/// Durable provision intent. `guest_disk` is unknown only when
/// `replaces_baseline` is set: the intent must be persisted before the baseline
/// deletion, and the boot disk can only be resolved after the snapshot delta is
/// committed. The service re-records the resolved path once it is known.
#[allow(
    clippy::too_many_arguments,
    reason = "a provisioning intent binds every fixed identity at once"
)]
pub fn provisioning_spec(
    provision_id: &str,
    generation: u64,
    media: MediaIdentity,
    source: ProvisionSource,
    paths: &ProvisionArtifactPaths,
    guest_disk: Option<&Path>,
    guest: &GuestPaths,
    baseline: &str,
    replaces_baseline: bool,
    identity_file: Option<PathBuf>,
    injected: Vec<super::InjectedFile>,
) -> ProvisioningSpec {
    ProvisioningSpec {
        provision_id: provision_id.into(),
        generation,
        media,
        source,
        iso: paths.iso.clone(),
        vmx_backup: paths.vmx_backup.clone(),
        guest_disk: guest_disk.map(Path::to_path_buf),
        service_name: guest.service.clone(),
        guest_root: guest.root.clone(),
        baseline_snapshot: baseline.into(),
        replaces_baseline,
        identity_file,
        injected,
    }
}

#[cfg(test)]
pub(crate) mod fixture;
#[cfg(test)]
mod tests;
