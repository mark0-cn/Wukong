//! Read-only inventory of recorded worker media, registered environments and
//! deployment diagnostics.
//!
//! Each record is read by its owning module: the media registry owns immutable
//! generation records, [`super::state`] owns the controller-side recovery
//! record with the imported receipt, and the configuration owns the current
//! environment registration. This module only projects those results for the
//! desktop; it never executes the import/verification workflow.
use super::{
    SetupPaths, default_environment_name, environment_claims,
    state::{self, InstallationStatus},
};
use crate::{
    adapters::environments::EnvironmentConfig,
    application::worker_media::{GenerationRecord, scan_generation_records},
    config::AppConfig,
};
use anyhow::Result;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Read-only state of one deployment record or inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    /// The owning module read and validated the record.
    Present,
    /// No such record exists yet.
    Missing,
    /// The record exists but cannot be read or validated.
    Corrupt,
    /// A record this value derives from is corrupt, so the value is unknown.
    Unknown,
}

/// Verification state of the managed connection to one installation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    /// No install receipt has been imported, so verification never ran.
    NotStarted,
    /// A receipt, registration or stale verification exists but is not complete.
    Incomplete,
    /// A registered environment matches the verified installation.
    Verified,
    /// The owning recovery record is corrupt, so verification cannot be judged.
    Unknown,
}

/// One unreadable generation record kept for read-only diagnostics.
#[derive(Clone, Debug, Serialize)]
pub struct DeploymentRecordIssue {
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation_id: Option<String>,
    pub error: String,
}

/// Report-level state of the installation inventory sources.
#[derive(Clone, Debug, Serialize)]
pub struct DeploymentDiagnostics {
    /// Registry directory: present, missing, or unreadable.
    pub inventory: RecordState,
    pub unreadable_generation_records: Vec<DeploymentRecordIssue>,
    /// Bounded registry read error; never translated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Read-only per-installation deployment diagnostics.
#[derive(Clone, Debug, Serialize)]
pub struct InstallationDiagnostics {
    /// Immutable generation record read from the media registry.
    pub generation_record: RecordState,
    /// Controller-side recovery record: imported receipt, host-key pin and last address.
    pub recovery_record: RecordState,
    /// Public install receipt exported by the Windows setup.
    pub receipt: RecordState,
    /// Current configuration registration for this installation.
    pub registration: RecordState,
    /// Managed-connection verification.
    pub verification: VerificationState,
    /// Bounded read/validation error; never translated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// One registered environment as reported to the setup GUI.
#[derive(Clone, Debug, Serialize)]
pub struct RegisteredEnvironmentView {
    pub environment_id: String,
    pub name: String,
    pub platform: String,
    pub installation_id: Option<String>,
    pub ssh_host: Option<String>,
    pub managed_ssh: bool,
    pub worker_media_state: Option<InstallationStatus>,
}

/// One generated installation as reported to the setup GUI.
#[derive(Clone, Debug, Serialize)]
pub struct InstallationView {
    pub installation_id: String,
    pub environment_id: String,
    pub environment_name: String,
    pub state: InstallationStatus,
    pub app_version: String,
    pub ssh_port: u16,
    pub management_peer: String,
    pub network_prefix_len: u8,
    pub zip_path: PathBuf,
    pub zip_present: bool,
    pub media_path: PathBuf,
    pub media_present: bool,
    pub created_at: String,
    pub registered_environment_id: Option<String>,
    pub registered_environment_name: Option<String>,
    pub last_address: Option<String>,
    pub candidate_addresses: Vec<String>,
    pub warning: Option<String>,
    pub diagnostics: InstallationDiagnostics,
}

/// Response of `worker_media_status`.
#[derive(Clone, Debug, Serialize)]
pub struct StatusReport {
    pub environments: Vec<RegisteredEnvironmentView>,
    pub installations: Vec<InstallationView>,
    pub diagnostics: DeploymentDiagnostics,
}

fn matching_registration<'a>(
    config: &'a AppConfig,
    record: &GenerationRecord,
) -> Option<(&'a String, &'a EnvironmentConfig)> {
    config
        .environments
        .iter()
        .find(|(_, environment)| environment_claims(environment, record))
}

/// Read the deployment diagnostics without touching the import workflow.
fn read_inventory(paths: &SetupPaths) -> (Vec<GenerationRecord>, DeploymentDiagnostics) {
    match scan_generation_records(&paths.registry) {
        Ok(scan) => (
            scan.records,
            DeploymentDiagnostics {
                inventory: if scan.registry_present {
                    RecordState::Present
                } else {
                    RecordState::Missing
                },
                unreadable_generation_records: scan
                    .unreadable
                    .into_iter()
                    .map(|record| DeploymentRecordIssue {
                        path: record.path,
                        installation_id: record.installation_id,
                        error: record.error,
                    })
                    .collect(),
                detail: None,
            },
        ),
        Err(error) => (
            Vec::new(),
            DeploymentDiagnostics {
                inventory: RecordState::Corrupt,
                unreadable_generation_records: Vec::new(),
                detail: Some(format!("{error:#}")),
            },
        ),
    }
}

/// Report every generated installation and every registered environment.
pub fn status(home: &Path, config: &AppConfig) -> Result<StatusReport> {
    let paths = SetupPaths::discover(home);
    let (records, diagnostics) = read_inventory(&paths);
    let mut installations = Vec::with_capacity(records.len());
    let mut statuses: BTreeMap<String, InstallationStatus> = BTreeMap::new();
    for record in &records {
        let (sidecar, recovery_record, recovery_detail) =
            match state::read(&paths.root, &record.installation_id) {
                Ok(Some(state)) => (Some(state), RecordState::Present, None),
                Ok(None) => (None, RecordState::Missing, None),
                Err(error) => (None, RecordState::Corrupt, Some(format!("{error:#}"))),
            };
        let receipt = match (&sidecar, recovery_record) {
            (Some(state), _) if state.receipt.is_some() => RecordState::Present,
            (Some(_), _) => RecordState::Missing,
            (None, RecordState::Corrupt) => RecordState::Unknown,
            (None, _) => RecordState::Missing,
        };
        let registration = matching_registration(config, record);
        let registration_state = if registration.is_some() {
            RecordState::Present
        } else {
            RecordState::Missing
        };
        let verified = sidecar
            .as_ref()
            .is_some_and(|state| state.status == InstallationStatus::Available);
        let attempted = sidecar
            .as_ref()
            .is_some_and(|state| state.receipt.is_some());
        let verification = if recovery_record == RecordState::Corrupt {
            VerificationState::Unknown
        } else if verified && registration.is_some() {
            VerificationState::Verified
        } else if attempted || registration.is_some() {
            VerificationState::Incomplete
        } else {
            VerificationState::NotStarted
        };
        let mut warning = recovery_detail.clone();
        let computed = if verified && registration.is_some() {
            InstallationStatus::Available
        } else if verified {
            warning = Some(
                "the registered environment for this installation is missing; retry the connection or import the receipt again"
                    .into(),
            );
            InstallationStatus::InstalledPending
        } else if registration.is_some() || attempted {
            InstallationStatus::InstalledPending
        } else {
            InstallationStatus::Generated
        };
        statuses.insert(record.installation_id.clone(), computed);
        installations.push(InstallationView {
            installation_id: record.installation_id.clone(),
            environment_id: record.environment_id.clone(),
            environment_name: sidecar
                .as_ref()
                .map(|state| state.environment_name.clone())
                .unwrap_or_else(|| default_environment_name(&record.installation_id)),
            state: computed,
            app_version: record.app_version.clone(),
            ssh_port: record.ssh_port,
            management_peer: record.management_peer.to_string(),
            network_prefix_len: record.network_prefix_len,
            zip_path: record.zip_path.clone(),
            zip_present: record.zip_path.is_file(),
            media_path: record.media_path.clone(),
            media_present: record.media_path.is_dir(),
            created_at: record.created_at.clone(),
            registered_environment_id: registration.map(|(id, _)| (*id).clone()),
            registered_environment_name: registration
                .map(|(_, environment)| environment.name.clone()),
            last_address: sidecar
                .as_ref()
                .and_then(|state| state.last_address.clone()),
            candidate_addresses: sidecar
                .as_ref()
                .and_then(|state| state.receipt.as_ref())
                .map(|receipt| receipt.candidate_addresses.clone())
                .unwrap_or_default(),
            warning,
            diagnostics: InstallationDiagnostics {
                generation_record: RecordState::Present,
                recovery_record,
                receipt,
                registration: registration_state,
                verification,
                detail: recovery_detail,
            },
        });
    }
    let environments = config
        .environments
        .iter()
        .map(|(id, environment)| {
            let ssh = environment.runtime.managed_ssh();
            let worker_media_state = records
                .iter()
                .find(|record| environment_claims(environment, record))
                .and_then(|record| statuses.get(&record.installation_id).copied());
            RegisteredEnvironmentView {
                environment_id: id.clone(),
                name: environment.name.clone(),
                platform: environment.platform.as_str().to_owned(),
                installation_id: environment
                    .installation_id
                    .clone()
                    .or_else(|| ssh.map(|ssh| ssh.installation_id.clone())),
                ssh_host: environment.runtime.ssh_host().map(str::to_owned),
                managed_ssh: ssh.is_some(),
                worker_media_state,
            }
        })
        .collect();
    Ok(StatusReport {
        environments,
        installations,
        diagnostics,
    })
}
