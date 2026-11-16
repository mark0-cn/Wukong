//! Typed experiment declarations, leases and durable receipts.
use super::*;

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LabPhase {
    Idle,
    Preparing,
    Ready,
    Restoring,
    RecoveryRequired,
    Failed,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Prepare,
    Restore,
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

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PendingOperation {
    pub operation_id: String,
    pub kind: OperationKind,
    pub generation: u64,
    pub lab_id: String,
    pub owner: String,
    pub purpose: String,
    pub spec_sha256: String,
    pub spec: ExperimentSpec,
    #[serde(default)]
    pub package_sha256: Option<String>,
    #[serde(default)]
    pub install_sha256: Option<String>,
    pub snapshot: String,
    pub started_at: String,
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
