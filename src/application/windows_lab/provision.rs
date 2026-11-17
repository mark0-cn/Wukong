//! Offline provisioning of the registered lab VM.
//!
//! The worker is installed while the guest is powered off: finalized media is
//! re-verified, the guest SYSTEM hive gains one fixed LocalSystem one-shot
//! service, the wrapper runs once at boot, then the controller restores the VMX
//! and reads a guest receipt through libguestfs. Host tools are fixed, bounded
//! and addressed only from operator configuration; model input never reaches
//! this module.
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
pub const GUEST_PROVISION_ROOT: &str = r"C:\ProgramData\WukongLab\provision";
pub const GUEST_SYSTEM_HIVE: &str = "/Windows/System32/config/SYSTEM";
pub const POWERSHELL: &str = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
pub const MAX_MEDIA_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_RECEIPT_BYTES: u64 = 1024 * 1024;
pub const MAX_INSTALLER_OUTPUT: usize = 64 * 1024;
pub const MAX_MEDIA_ENTRIES: usize = 20_000;
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// Fixed guest paths written before boot and inspected afterwards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestPaths {
    /// Windows form of the per-provision directory.
    pub root: String,
    pub wrapper: String,
    pub plan: String,
    pub receipt: String,
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
        Ok(Self {
            wrapper: format!(r"{root}\provision.ps1"),
            plan: format!(r"{root}\provision-plan.json"),
            receipt: format!(r"{root}\receipt.json"),
            service: format!("WukongLabProvision-{provision_id}"),
            root,
        })
    }

    /// Guest-relative path form used by libguestfs (the Windows system volume
    /// is mounted at `/`).
    pub fn guestfish(path: &str) -> Result<String> {
        let relative = path
            .strip_prefix(r"C:\")
            .context("guest path must be an absolute C: path")?;
        ensure!(
            !relative.contains(['\n', '\r', '\0', '"']),
            "guest path contains an unsupported character"
        );
        Ok(format!("/{}", relative.replace('\\', "/")))
    }
}

#[derive(Clone, Debug)]
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
    /// Any leftover one-shot provisioning service.
    pub existing_service: Option<String>,
    /// Entries left below the fixed provision root directory.
    pub provision_root_entries: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskInspectionRequest {
    pub system_hive: String,
    pub program_data: String,
    pub provision_root: String,
    pub service_prefix: String,
}

impl DiskInspectionRequest {
    pub fn new() -> Self {
        Self {
            system_hive: GUEST_SYSTEM_HIVE.into(),
            program_data: "/ProgramData".into(),
            provision_root: "/ProgramData/WukongLab".into(),
            service_prefix: "WukongLabProvision".into(),
        }
    }
}

impl Default for DiskInspectionRequest {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HivePatch {
    pub system_hive: String,
    pub control_set: u32,
    pub script: String,
    pub verify_script: String,
    /// Controller-side copy of the pre-patch hive for recovery evidence.
    pub host_backup: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HivePatchEvidence {
    pub hive_sha256_before: String,
    pub hive_sha256_after: String,
    pub verify_output: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestObservationRequest {
    pub system_hive: String,
    pub control_set: u32,
    pub service_name: String,
    pub wrapper: String,
    pub plan: String,
    pub receipt: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestObservation {
    pub receipt: Option<Vec<u8>>,
    pub service_present: bool,
    pub wrapper_present: bool,
    pub plan_present: bool,
}

/// Host-side offline disk editing. Implementations execute fixed libguestfs and
/// hivexsh commands through operator-configured WSL tooling; tests substitute an
/// in-memory fake so no real VM is ever touched.
#[async_trait]
pub trait OfflineDisk: Send + Sync {
    async fn inspect(&self, disk: &Path, request: &DiskInspectionRequest)
    -> Result<DiskInspection>;
    async fn write_guest_files(&self, disk: &Path, files: &[GuestFile]) -> Result<()>;
    async fn patch_system_hive(&self, disk: &Path, patch: &HivePatch) -> Result<HivePatchEvidence>;
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
    /// No snapshots exist; provisioning starts from the powered-off base disk.
    Pristine,
    /// Only the configured baseline exists and `--replace-baseline` was passed.
    ReplaceBaseline,
}

/// A provisioned VM must be powered off with no snapshot chain. The only
/// tolerated snapshot is the configured baseline when it is being replaced;
/// no other snapshot is ever deleted.
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
        "baseline snapshot {baseline} already exists; pass --replace-baseline to delete and recreate it after all validations"
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

/// Fixed one-shot service hive edit. `hivexsh -w` reads this from stdin.
pub fn hive_patch_script(control_set: u32, service: &str, image_path: &str) -> Result<String> {
    ensure!(
        (1..=999).contains(&control_set),
        "invalid Windows control set number"
    );
    ensure!(
        !service.is_empty()
            && service.len() <= 128
            && service
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid one-shot service name"
    );
    ensure!(
        !image_path.is_empty()
            && image_path.len() <= 1024
            && image_path.is_ascii()
            && !image_path.contains(['\n', '\r', '\0', '\t', '"']),
        "invalid one-shot service image path"
    );
    Ok(format!(
        "cd \\ControlSet{control_set:03}\\Services\nadd {service}\ncd {service}\nsetval 6\nType\ndword:16\nStart\ndword:00000002\nErrorControl\ndword:00000001\nImagePath\nexpandstring:{image_path}\nObjectName\nstring:LocalSystem\nDescription\nstring:Wukong offline one-shot worker provisioning\ncommit\nquit\n"
    ))
}

pub fn hive_verify_script(control_set: u32, service: &str) -> Result<String> {
    ensure!(
        (1..=999).contains(&control_set),
        "invalid Windows control set number"
    );
    ensure!(
        !service.is_empty()
            && service
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid one-shot service name"
    );
    Ok(format!(
        "cd \\ControlSet{control_set:03}\\Services\\{service}\nlsval\nquit\n"
    ))
}

/// Verify the service values read back from the hive. Backslashes are escaped
/// by hivexsh string output.
pub fn verify_hive_output(output: &str, image_path: &str) -> Result<()> {
    let values: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let contains = |expected: &str| values.contains(&expected);
    ensure!(
        contains("\"Type\"=dword:00000010"),
        "one-shot service type was not written"
    );
    ensure!(
        contains("\"Start\"=dword:00000002"),
        "one-shot service start mode was not written"
    );
    ensure!(
        contains("\"ErrorControl\"=dword:00000001"),
        "one-shot service error control was not written"
    );
    ensure!(
        contains("\"ObjectName\"=\"LocalSystem\""),
        "one-shot service account was not written"
    );
    let expected = format!(
        "\"ImagePath\"=str(2):\"{}\"",
        image_path.replace('\\', "\\\\")
    );
    ensure!(
        contains(&expected),
        "one-shot service image path was not written"
    );
    Ok(())
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
        "schema": 1,
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
        "service_name": paths.service,
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

/// Fixed guest wrapper image path built from the validated provision identity.
pub fn service_image_path(wrapper: &str) -> Result<String> {
    ensure!(
        wrapper.starts_with(r"C:\ProgramData\WukongLab\provision\")
            && wrapper.is_ascii()
            && !wrapper.contains(['\n', '\r', '\0', '"']),
        "invalid guest wrapper path"
    );
    Ok(format!(
        "{POWERSHELL} -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File {wrapper}"
    ))
}

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
        receipt["schema"] == 1 && receipt["provision_id"] == expectation.provision_id,
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
        receipt["service_delete"] == "deleted",
        "guest one-shot service was not deleted"
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

/// Persisted intent for one provisioning attempt.
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
    guest_disk: &Path,
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
        guest_disk: guest_disk.to_path_buf(),
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
