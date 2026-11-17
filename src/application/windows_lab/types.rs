//! Typed experiment declarations, leases and durable receipts.
use super::*;

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LabPhase {
    Idle,
    Preparing,
    Ready,
    Restoring,
    Provisioning,
    RecoveryRequired,
    Failed,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Prepare,
    Restore,
    Provision,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaunchKind {
    Exe,
    DllHarness,
    Service,
    Gui,
    Driver,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputChannel {
    Argv,
    Stdin,
    File,
    Network,
    NamedPipe,
    Com,
    Gui,
    Ioctl,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountContext {
    User,
    ElevatedUser,
    System,
    Driver,
}

/// Bound to one experiment generation together with its target and toolchain
/// identities. A valid description is a precondition, not proof of execution.
#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExperimentSpec {
    pub target_sha256: String,
    pub launch: LaunchKind,
    pub input_channel: InputChannel,
    pub account: AccountContext,
    #[serde(default)]
    pub annotations: ExperimentAnnotations,
    #[serde(default)]
    pub recovery: String,
    /// Controller-fetched, hash-verified artifacts assembled into an offline package.
    #[serde(default)]
    pub dependencies: Vec<package::Dependency>,
    /// Guest-side installation and cleanup ownership. Validation only in Phase 1.
    #[serde(default)]
    pub install: Option<install::InstallPlan>,
}

/// Descriptive notes only. These are not executable requirements or evidence
/// of the actual guest build/toolchain; worker receipts carry observations.
#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExperimentAnnotations {
    pub os_build: Option<String>,
    pub toolchain: Option<String>,
}

impl ExperimentSpec {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            hex_digest(&self.target_sha256),
            "experiment target_sha256 must be 64 lowercase hex characters"
        );
        ensure!(
            self.annotations
                .os_build
                .as_ref()
                .is_none_or(|s| s.len() <= 128)
                && self
                    .annotations
                    .toolchain
                    .as_ref()
                    .is_none_or(|s| s.len() <= 512),
            "experiment annotations exceed bounds"
        );
        ensure!(
            self.recovery.len() <= 512,
            "experiment recovery note exceeds 512 characters"
        );
        package::validate_dependencies(&self.dependencies)?;
        if let Some(install) = &self.install {
            install.validate(self.launch)?;
        }
        Ok(())
    }

    pub fn sha256(&self) -> Result<String> {
        self.validate()?;
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(self)?)))
    }
}

fn hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ActiveExperiment {
    pub lab_id: String,
    pub owner: String,
    pub purpose: String,
    pub generation: u64,
    pub spec_sha256: String,
    pub spec: ExperimentSpec,
    #[serde(default)]
    pub snapshot: Option<String>,
    #[serde(default)]
    pub package_sha256: Option<String>,
    #[serde(default)]
    pub install_sha256: Option<String>,
    pub started_at: String,
}

/// Public Ed25519 SSH key as persisted in lab records. The worker package owns
/// the verification logic; this local shape keeps lab state `JsonSchema`-able.
#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SshKey {
    pub algorithm: String,
    pub key: String,
}

/// Controller-side identity of finalized offline worker media. Every hash is
/// re-verified before the powered-off guest disk is touched.
#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MediaIdentity {
    pub media_root: PathBuf,
    /// SHA-256 of the finalized-media receipt (`media.json`).
    pub media_record_sha256: String,
    pub setup_sha256: String,
    pub enrollment_sha256: String,
    pub manifest_sha256: String,
    pub controller_public_key: String,
    pub controller_version: String,
    pub installation_id: String,
    pub environment_id: String,
    pub generation: u64,
    pub ssh_port: u16,
    pub ssh_user: String,
    pub management_peer: String,
    pub ssh_public_key: SshKey,
}

/// How the verified media directory was obtained.
#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProvisionSource {
    /// Operator-supplied finalized media directory.
    Media { directory: PathBuf },
    /// Built by `python -m scripts.worker.media finalize` from a released template.
    Template {
        template: PathBuf,
        keys: PathBuf,
        spec: PathBuf,
    },
}

/// One fixed file written into the guest before boot.
#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InjectedFile {
    pub guest_path: String,
    pub sha256: String,
    pub size: u64,
}

/// Durable provision intent persisted before the first offline mutation. It is
/// sufficient to restore the VMX, remove the generated ISO/media and identify a
/// leftover guest service during `lab reconcile`.
#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProvisioningSpec {
    pub provision_id: String,
    pub generation: u64,
    pub media: MediaIdentity,
    pub source: ProvisionSource,
    /// Generated ISO path (removed after success and during reconciliation).
    pub iso: PathBuf,
    /// Original VMX bytes written here before the CD device edit.
    pub vmx_backup: PathBuf,
    /// Boot disk resolved before the offline writes. Absent only while
    /// `--replace-baseline` has persisted the intent but not yet deleted the
    /// configured baseline: the powered-off VM still points at the snapshot
    /// delta, so the committed disk cannot be resolved before that deletion.
    #[serde(default)]
    pub guest_disk: Option<PathBuf>,
    pub service_name: String,
    pub guest_root: String,
    pub baseline_snapshot: String,
    /// The configured baseline snapshot was deleted and will be recreated.
    pub replaces_baseline: bool,
    /// Controller-managed SSH identity generated for this provisioning.
    pub identity_file: Option<PathBuf>,
    pub injected: Vec<InjectedFile>,
}

/// Durable provisioning receipt published under
/// `<state_dir>/windows-lab/provisions/<provision_id>.json`.
#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LabProvision {
    pub provision_id: String,
    pub operation_id: String,
    pub generation: u64,
    pub status: RunStatus,
    pub source: ProvisionSource,
    pub media: MediaIdentity,
    pub iso_sha256: Option<String>,
    pub iso_size: Option<u64>,
    pub vmx_sha256_before: Option<String>,
    pub vmx_sha256_after: Option<String>,
    /// Legacy evidence from the earlier SYSTEM-hive one-shot service mechanism.
    /// New provisioning never writes a service; these stay `None` and are kept
    /// only so laboratory state written by that version still loads.
    #[serde(default)]
    pub hive_sha256_before: Option<String>,
    #[serde(default)]
    pub hive_sha256_after: Option<String>,
    /// SHA-256 of the injected machine `scripts.ini` (startup entry included).
    #[serde(default)]
    pub scripts_ini_sha256: Option<String>,
    /// SHA-256 of the injected local-GPO `gpt.ini` (bumped computer version).
    #[serde(default)]
    pub gpt_ini_sha256: Option<String>,
    pub identity_file: Option<PathBuf>,
    pub guest_receipt: Option<serde_json::Value>,
    pub connection: Option<serde_json::Value>,
    pub environment_verified: Option<bool>,
    pub verification: Option<serde_json::Value>,
    pub baseline_snapshot: Option<String>,
    pub settled: bool,
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PendingOperation {
    pub operation_id: String,
    pub kind: OperationKind,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lab_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub purpose: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub spec_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<ExperimentSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provision: Option<ProvisioningSpec>,
    #[serde(default)]
    pub package_sha256: Option<String>,
    #[serde(default)]
    pub install_sha256: Option<String>,
    pub snapshot: String,
    pub started_at: String,
}

impl PendingOperation {
    /// Exactly one operation payload matches the operation kind, so a corrupted
    /// or hand-edited record can never be replayed as another host effect.
    pub(crate) fn validate_shape(&self) -> Result<()> {
        match self.kind {
            OperationKind::Prepare | OperationKind::Restore => ensure!(
                self.spec.is_some() && self.provision.is_none(),
                "pending {:?} operation has the wrong payload",
                self.kind
            ),
            OperationKind::Provision => ensure!(
                self.provision.is_some() && self.spec.is_none(),
                "pending provision operation has the wrong payload"
            ),
        }
        Ok(())
    }
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Lost,
    Cancelled,
    Unknown,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LabRun {
    pub run_id: String,
    pub lab_id: String,
    pub generation: u64,
    pub artifact: String,
    pub artifact_sha256: String,
    pub status: RunStatus,
    #[serde(default)]
    pub environment_id: Option<String>,
    #[serde(default)]
    pub installation_id: Option<String>,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    #[serde(default)]
    pub executable_path: Option<String>,
    pub settled: bool,
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LabInstall {
    pub install_id: String,
    pub lab_id: String,
    pub generation: u64,
    pub package_sha256: String,
    pub media_sha256: String,
    pub install_sha256: String,
    pub script_sha256: String,
    pub guest_receipt: serde_json::Value,
    pub verified: bool,
    pub status: RunStatus,
    #[serde(default)]
    pub exit_code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    #[serde(default)]
    pub guest_media: Option<String>,
    #[serde(default)]
    pub guest_script: Option<String>,
    pub settled: bool,
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LabState {
    pub schema: u32,
    pub vm_id: String,
    pub configuration_sha256: String,
    pub generation: u64,
    pub phase: LabPhase,
    pub baseline_snapshot: String,
    pub active: Option<ActiveExperiment>,
    pub pending: Option<PendingOperation>,
    #[serde(default)]
    pub last_run: Option<LabRun>,
    #[serde(default)]
    pub last_install: Option<LabInstall>,
    #[serde(default)]
    pub last_provision: Option<LabProvision>,
    pub last_error: Option<String>,
    pub updated_at: String,
}

impl LabState {
    pub(super) fn fresh(vm_id: &str, baseline: &str) -> Self {
        Self {
            schema: STATE_SCHEMA,
            vm_id: vm_id.to_owned(),
            configuration_sha256: String::new(),
            generation: 0,
            phase: LabPhase::Idle,
            baseline_snapshot: baseline.to_owned(),
            active: None,
            pending: None,
            last_run: None,
            last_install: None,
            last_provision: None,
            last_error: None,
            updated_at: now(),
        }
    }
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug)]
pub struct LabLease {
    pub vm_id: String,
    pub lab_id: String,
    pub generation: u64,
    pub snapshot: String,
    pub spec_sha256: String,
    pub package_sha256: Option<String>,
    pub install_sha256: Option<String>,
    pub owner: String,
}
