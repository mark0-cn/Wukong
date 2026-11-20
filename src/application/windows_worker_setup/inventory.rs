//! Read-only inventory of recorded worker media and registered environments.
use super::{
    SetupPaths, default_environment_name, environment_claims, read_records,
    state::{self, InstallationStatus},
};
use crate::{
    adapters::environments::EnvironmentConfig, application::worker_media::GenerationRecord,
    config::AppConfig,
};
use anyhow::Result;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

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
}

/// Response of `worker_media_status`.
#[derive(Clone, Debug, Serialize)]
pub struct StatusReport {
    pub environments: Vec<RegisteredEnvironmentView>,
    pub installations: Vec<InstallationView>,
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

/// Report every generated installation and every registered environment.
pub fn status(home: &Path, config: &AppConfig) -> Result<StatusReport> {
    let paths = SetupPaths::discover(home);
    let records = read_records(&paths)?;
    let mut installations = Vec::with_capacity(records.len());
    let mut statuses: BTreeMap<String, InstallationStatus> = BTreeMap::new();
    for record in &records {
        let (sidecar, mut warning) = match state::read(&paths.root, &record.installation_id) {
            Ok(state) => (state, None),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        let registration = matching_registration(config, record);
        let verified = sidecar
            .as_ref()
            .is_some_and(|state| state.status == InstallationStatus::Available);
        let attempted = sidecar
            .as_ref()
            .is_some_and(|state| state.receipt.is_some());
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
    })
}
