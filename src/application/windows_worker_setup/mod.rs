//! Desktop workflow for the offline Windows worker setup.
//!
//! The two operator actions are *generate install media* and *import the
//! connection receipt*. Trust decisions stay in the building blocks: the shared
//! media generator seals and verifies the media, the signed release catalog
//! authenticates the downloaded template, and the existing Windows connection
//! import derives the managed-SSH environment and public host pin. This module
//! only orchestrates those pieces: it resolves the template, selects the
//! management network, cross-checks the receipt against the immutable generation
//! record, verifies candidate addresses through the existing managed SSH doctor,
//! and records the public workflow state for the GUI.
//!
//! The command layer in `crate::interfaces::desktop::worker_media` performs the
//! configuration save after verification, so a failed import never leaves a
//! partially registered environment behind.
mod candidates;
mod inventory;
mod probe;
mod receipt;
mod state;
mod template;

pub use inventory::{
    DeploymentDiagnostics, DeploymentRecordIssue, InstallationDiagnostics, InstallationView,
    RecordState, RegisteredEnvironmentView, StatusReport, VerificationState, status,
};
pub use receipt::{ConnectionReceipt, MAX_RECEIPT_BYTES};
pub use state::InstallationStatus;
pub use template::TemplateSource;

use crate::adapters::environments::registration::WindowsConnection;
use crate::adapters::environments::{EnvironmentConfig, Platform, validate_id};
use crate::adapters::net::{ManagementNetwork, SystemAdapters};
use crate::application::worker_media::{self, GenerationRecord};
use crate::config::AppConfig;
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

/// Root directory of controller-local worker media state below the profile home.
pub const ROOT_DIR: &str = "worker-media";
/// Generation registry written by the shared media generator.
pub const REGISTRY_DIR: &str = "registry";
/// Protected controller signing identity directory.
pub const KEYS_DIR: &str = "keys";
/// Environment id prefix; each installation gets its own id so multiple workers
/// can be registered side by side. The shared generator accepts any valid id.
pub const DEFAULT_ENVIRONMENT_PREFIX: &str = "windows";
/// Setup executable inside the generated media.
pub const SETUP_EXECUTABLE: &str = "WukongWorkerSetup.exe";
/// Public receipt file exported by the Windows setup.
pub const RECEIPT_FILENAME: &str = "connection.json";

/// Paths of the controller-local worker media tree.
#[derive(Clone, Debug)]
pub struct SetupPaths {
    pub home: PathBuf,
    pub root: PathBuf,
    pub registry: PathBuf,
    pub keys: PathBuf,
}

impl SetupPaths {
    pub fn discover(home: &Path) -> Self {
        let root = home.join(ROOT_DIR);
        Self {
            home: home.to_owned(),
            registry: root.join(REGISTRY_DIR),
            keys: root.join(KEYS_DIR),
            root,
        }
    }
}

/// Environment id for one installation: `windows-<first 8 hex chars>`.
pub fn environment_id_for(installation_id: &str) -> Result<String> {
    ensure!(
        installation_id.len() == 32,
        "worker media installation identity is invalid"
    );
    let short: String = installation_id.chars().take(8).collect();
    let id = format!("{DEFAULT_ENVIRONMENT_PREFIX}-{short}");
    validate_id(&id)?;
    Ok(id)
}

/// User-facing default name for one installation.
pub fn default_environment_name(installation_id: &str) -> String {
    let short: String = installation_id.chars().take(8).collect();
    format!("Windows Worker {short}")
}

fn validate_environment_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    ensure!(
        !trimmed.is_empty() && trimmed.len() <= 256,
        "the environment name must contain 1-256 characters"
    );
    Ok(trimmed.to_owned())
}

/// Enumerate the host's eligible management networks.
pub fn networks() -> Result<Vec<ManagementNetwork>> {
    crate::adapters::net::management_networks(&SystemAdapters)
}

/// Read every generation record in this controller's registry.
fn read_records(paths: &SetupPaths) -> Result<Vec<GenerationRecord>> {
    if !paths.registry.try_exists()? {
        return Ok(Vec::new());
    }
    ensure!(
        paths.registry.is_dir(),
        "the worker media registry is not a directory: {}",
        paths.registry.display()
    );
    worker_media::read_generation_records(&paths.registry)
}

fn environment_claims(environment: &EnvironmentConfig, record: &GenerationRecord) -> bool {
    environment.installation_id.as_deref() == Some(record.installation_id.as_str())
        || environment.runtime.managed_ssh().is_some_and(|ssh| {
            ssh.installation_id == record.installation_id
                || ssh.identity_file == record.ssh_identity_file
        })
}

/// Input of one media generation request.
#[derive(Clone, Debug)]
pub struct GenerateInput {
    /// Operator-selected local template file; otherwise the signed release
    /// catalog is used to download the version-matched template.
    pub template: Option<PathBuf>,
    /// New controller-credential-free media ZIP to write.
    pub output: PathBuf,
    /// Editable environment name stored in the installation sidecar.
    pub environment_name: Option<String>,
    /// Selected management network id, `a.b.c.d` or `a.b.c.d/prefix`.
    pub network: Option<String>,
}

/// Template identity used for one generation.
#[derive(Clone, Debug, Serialize)]
pub struct TemplateInfo {
    pub path: PathBuf,
    pub source: TemplateSource,
    pub version: Option<String>,
    pub sha256: Option<String>,
}

/// Data the GUI shows after a successful generation.
#[derive(Clone, Debug, Serialize)]
pub struct GenerateInstructions {
    pub summary: String,
    pub setup_executable: String,
    pub receipt_filename: String,
    pub steps: Vec<String>,
}

/// Response of `worker_media_generate`.
#[derive(Clone, Debug, Serialize)]
pub struct GenerateReport {
    pub installation_id: String,
    pub environment_id: String,
    pub environment_name: String,
    pub installation_state: InstallationStatus,
    pub app_version: String,
    pub zip_path: PathBuf,
    pub zip_sha256: String,
    pub zip_size: u64,
    pub zip_entries: usize,
    pub media_path: PathBuf,
    pub registry_path: PathBuf,
    pub ssh_port: u16,
    pub management_peer: String,
    pub network_prefix_len: u8,
    pub network: ManagementNetwork,
    pub template: TemplateInfo,
    pub instructions: GenerateInstructions,
}

/// Select the management network for one generation.
///
/// An explicit GUI selection always wins. Without one, a single eligible network
/// is used automatically; otherwise the operator must choose, so a controller
/// with several adapters never guesses which address the guest can reach.
pub(crate) fn select_network<'a>(
    networks: &'a [ManagementNetwork],
    requested: Option<&str>,
) -> Result<&'a ManagementNetwork> {
    if let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        let matched = networks.iter().find(|network| {
            network.id == requested
                || network.ipv4.to_string() == requested
                || format!("{}/{}", network.ipv4, network.prefix_len) == requested
        });
        return matched.with_context(|| {
            format!(
                "the selected management network is not available: {requested}; available networks: {}",
                network_choices(networks)
            )
        });
    }
    match networks {
        [] => bail!(
            "no eligible IPv4 management network was found on this controller; connect the disposable VM's host-only network adapter and refresh the network list"
        ),
        [single] => Ok(single),
        _ => bail!(
            "multiple IPv4 management networks are available on this controller; choose one: {}",
            network_choices(networks)
        ),
    }
}

fn network_choices(networks: &[ManagementNetwork]) -> String {
    if networks.is_empty() {
        return "none".into();
    }
    networks
        .iter()
        .map(|network| {
            format!(
                "{} — {}/{} (id {}){}",
                network.name,
                network.ipv4,
                network.prefix_len,
                network.id,
                if network.recommended {
                    " [recommended]"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn resolve_template(
    paths: &SetupPaths,
    local: Option<PathBuf>,
) -> Result<template::ResolvedTemplate> {
    if let Some(local) = local {
        let path = std::path::absolute(&local)
            .with_context(|| format!("resolve the worker media template {}", local.display()))?;
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("the worker media template is missing: {}", path.display()))?;
        ensure!(
            metadata.is_file() || metadata.is_dir(),
            "the worker media template is not a file or directory: {}",
            path.display()
        );
        if metadata.is_file() {
            template::ensure_template_archive(&path)?;
        }
        return Ok(template::ResolvedTemplate {
            path,
            source: TemplateSource::Local,
            version: None,
            sha256: None,
        });
    }
    fs::create_dir_all(paths.root.join(template::TEMPLATE_CACHE_DIR))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("prepare the worker media template download runtime")?;
    runtime.block_on(template::resolve_downloaded(
        &paths.root,
        worker_media::APP_VERSION,
    ))
}

/// Generate one finalized, controller-credential-free installation archive.
pub fn generate(home: &Path, input: GenerateInput) -> Result<GenerateReport> {
    let paths = SetupPaths::discover(home);
    crate::adapters::files::owned::create_owned_directory(&paths.root)?;
    let resolved = resolve_template(&paths, input.template)?;
    let network = select_network(&networks()?, input.network.as_deref())?.clone();
    let installation_id = uuid::Uuid::new_v4().simple().to_string();
    let environment_id = environment_id_for(&installation_id)?;
    let environment_name = match input.environment_name.as_deref() {
        Some(name) => validate_environment_name(name)?,
        None => default_environment_name(&installation_id),
    };
    let output = std::path::absolute(&input.output).with_context(|| {
        format!(
            "resolve the worker media output archive {}",
            input.output.display()
        )
    })?;
    ensure!(
        !output.exists(),
        "the selected output archive already exists; choose another file: {}",
        output.display()
    );
    let outcome = worker_media::generate(&worker_media::GenerateRequest {
        template: resolved.path.clone(),
        output_zip: output.clone(),
        registry: paths.registry.clone(),
        keys: paths.keys.clone(),
        environment_id: Some(environment_id.clone()),
        management_peer: Some(network.ipv4),
        ssh_port: None,
        controller_version: None,
        installation_id: Some(installation_id.clone()),
    })?;
    let state = state::generated(&paths.root, &outcome.record, &environment_name)?;
    Ok(GenerateReport {
        installation_id,
        environment_id,
        environment_name,
        installation_state: state.status,
        app_version: outcome.record.app_version.clone(),
        zip_path: output,
        zip_sha256: outcome.zip_sha256,
        zip_size: outcome.zip_size,
        zip_entries: outcome.zip_entries,
        media_path: outcome.record.media_path.clone(),
        registry_path: outcome.record_path,
        ssh_port: outcome.record.ssh_port,
        management_peer: network.ipv4.to_string(),
        network_prefix_len: outcome.record.network_prefix_len,
        network,
        template: TemplateInfo {
            path: resolved.path,
            source: resolved.source,
            version: resolved.version,
            sha256: resolved.sha256,
        },
        instructions: GenerateInstructions {
            summary: "This archive contains the offline worker installer and enrollment for exactly this controller; it holds no controller credentials. Run it only in a disposable Windows VM.".into(),
            setup_executable: SETUP_EXECUTABLE.into(),
            receipt_filename: RECEIPT_FILENAME.into(),
            steps: vec![
                "Copy the ZIP archive into the disposable Windows VM and extract it.".into(),
                format!("Run {SETUP_EXECUTABLE} as administrator and confirm the installation summary."),
                format!(
                    "Save the exported {RECEIPT_FILENAME} file and compare its SSH host fingerprint with the setup dialog."
                ),
                "Return to the controller and import the saved receipt to register and verify the worker.".into(),
            ],
        },
    })
}

/// Input of one connection receipt import.
#[derive(Clone, Debug)]
pub struct ImportInput {
    /// Receipt JSON document or an absolute/relative path to `connection.json`.
    pub receipt: String,
    /// Fingerprint the operator independently confirmed.
    pub confirmed_fingerprint: String,
    /// Optional editable environment name.
    pub name: Option<String>,
}

/// An environment that already serves this installation and must be updated
/// instead of duplicated.
#[derive(Clone, Debug)]
pub struct ExistingEnvironment {
    pub environment_id: String,
    pub name: String,
    pub ssh_host: String,
}

/// A verified import waiting for the command layer to save the environment.
#[derive(Debug)]
pub struct PreparedImport {
    pub installation_id: String,
    pub environment_id: String,
    pub environment_name: String,
    /// Verified management address; `None` means the receipt has no usable
    /// candidate and the installation stays `installed_pending`.
    pub address: Option<Ipv4Addr>,
    pub candidate_addresses: Vec<String>,
    pub attempts: usize,
    pub existing_environment: Option<ExistingEnvironment>,
    /// Operator-supplied name that differs from the registered environment name.
    pub explicit_name: Option<String>,
    /// Connection input for registering a new environment; `None` when an
    /// existing environment only needs its address updated.
    pub connection: Option<WindowsConnection>,
    pub guidance: Option<String>,
}

impl PreparedImport {
    pub fn is_pending(&self) -> bool {
        self.address.is_none()
    }
}

/// Response of `worker_media_import` and `worker_media_retry`.
#[derive(Clone, Debug, Serialize)]
pub struct ImportEnvironment {
    pub environment_id: String,
    pub name: String,
    pub ssh_host: Option<String>,
}

/// Response of `worker_media_import` and `worker_media_retry`.
#[derive(Clone, Debug, Serialize)]
pub struct ImportReport {
    pub installation_id: String,
    pub environment_id: String,
    pub state: InstallationStatus,
    pub environment: Option<ImportEnvironment>,
    pub address: Option<String>,
    pub candidate_count: usize,
    pub guidance: Option<String>,
}

impl ImportReport {
    pub fn pending(prepared: &PreparedImport) -> Self {
        Self {
            installation_id: prepared.installation_id.clone(),
            environment_id: prepared.environment_id.clone(),
            state: InstallationStatus::InstalledPending,
            environment: prepared
                .existing_environment
                .as_ref()
                .map(|existing| ImportEnvironment {
                    environment_id: existing.environment_id.clone(),
                    name: existing.name.clone(),
                    ssh_host: Some(existing.ssh_host.clone()),
                }),
            address: None,
            candidate_count: prepared.candidate_addresses.len(),
            guidance: prepared.guidance.clone(),
        }
    }

    pub fn verified(
        prepared: &PreparedImport,
        address: Ipv4Addr,
        environment: Option<ImportEnvironment>,
    ) -> Self {
        Self {
            installation_id: prepared.installation_id.clone(),
            environment_id: prepared.environment_id.clone(),
            state: InstallationStatus::Available,
            environment,
            address: Some(address.to_string()),
            candidate_count: prepared.candidate_addresses.len(),
            guidance: None,
        }
    }
}

#[derive(Debug)]
struct RegistrationTarget {
    existing: Option<ExistingEnvironment>,
    environment_name: String,
}

fn registration_target(
    config: &AppConfig,
    record: &GenerationRecord,
    receipt: &ConnectionReceipt,
    requested_name: Option<&str>,
    fallback_name: Option<&str>,
) -> Result<RegistrationTarget> {
    let matching: Vec<(&String, &EnvironmentConfig)> = config
        .environments
        .iter()
        .filter(|(_, environment)| environment_claims(environment, record))
        .collect();
    ensure!(
        matching.len() <= 1,
        "multiple registered environments claim worker installation {}; resolve the duplicate before importing",
        record.installation_id
    );
    let explicit_name = requested_name
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if let Some((environment_id, environment)) = matching.first() {
        ensure!(
            environment.platform == Platform::Windows,
            "the registered environment for this installation is not a Windows environment; it was not replaced"
        );
        let ssh = environment.runtime.managed_ssh().context(
            "the registered environment for this installation is not a managed SSH environment; it was not replaced",
        )?;
        ensure!(
            ssh.host_key.algorithm == receipt.host_key.algorithm
                && ssh.host_key.key == receipt.host_key.key,
            "a registered environment pins a different worker host key; the connection was not replaced"
        );
        ensure!(
            ssh.port == receipt.ssh_port && ssh.user == receipt.ssh_user,
            "a registered environment uses different SSH enrollment settings than the receipt; it was not replaced"
        );
        ensure!(
            ssh.identity_file == record.ssh_identity_file,
            "a registered environment uses a different SSH identity than the generated media; it was not replaced"
        );
        let environment_name = match explicit_name {
            Some(name) => validate_environment_name(name)?,
            None => validate_environment_name(&environment.name)?,
        };
        Ok(RegistrationTarget {
            existing: Some(ExistingEnvironment {
                environment_id: (*environment_id).clone(),
                name: environment.name.clone(),
                ssh_host: environment
                    .runtime
                    .ssh_host()
                    .unwrap_or_default()
                    .to_owned(),
            }),
            environment_name,
        })
    } else {
        ensure!(
            !config.environments.contains_key(&receipt.environment_id),
            "environment {} is already registered with different settings; it was not replaced",
            receipt.environment_id
        );
        let environment_name = match explicit_name.or(fallback_name) {
            Some(name) => validate_environment_name(name)?,
            None => default_environment_name(&record.installation_id),
        };
        Ok(RegistrationTarget {
            existing: None,
            environment_name,
        })
    }
}

fn probe_environment_map(
    config: &AppConfig,
    existing: Option<&ExistingEnvironment>,
) -> BTreeMap<String, EnvironmentConfig> {
    let mut environments = config.environments.clone();
    if let Some(existing) = existing {
        environments.remove(&existing.environment_id);
    }
    environments
}

fn import_for_record(
    paths: &SetupPaths,
    config: &AppConfig,
    record: &GenerationRecord,
    receipt: ConnectionReceipt,
    requested_name: Option<&str>,
    previous: Option<state::InstallationState>,
) -> Result<PreparedImport> {
    receipt::require_pinned_key(
        &receipt,
        previous
            .as_ref()
            .and_then(|state| state.pinned_host_key.as_ref()),
    )?;
    let target = registration_target(
        config,
        record,
        &receipt,
        requested_name,
        previous
            .as_ref()
            .map(|state| state.environment_name.as_str()),
    )?;
    // Persist the public receipt before any network work so a failed or
    // interrupted verification can be retried without asking for the file again.
    state::remember_receipt(&paths.root, record, &target.environment_name, &receipt)?;
    let preferred = previous
        .as_ref()
        .and_then(|state| state.last_address.as_deref())
        .or_else(|| {
            target
                .existing
                .as_ref()
                .map(|existing| existing.ssh_host.as_str())
        });
    let addresses = candidates::candidates_for(record, &receipt, preferred);
    if addresses.is_empty() {
        return Ok(PreparedImport {
            installation_id: record.installation_id.clone(),
            environment_id: record.environment_id.clone(),
            environment_name: target.environment_name,
            address: None,
            candidate_addresses: receipt.candidate_addresses.clone(),
            attempts: 0,
            existing_environment: target.existing,
            explicit_name: requested_name
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned),
            connection: None,
            guidance: Some(
                "The saved receipt lists no address candidate on this installation's management network. Start the VM setup again without arguments to export a refreshed install result with its current addresses, then import that file."
                    .into(),
            ),
        });
    }
    let receipt_json = serde_json::to_string(&receipt)
        .context("serialize the public Windows connection receipt")?;
    let existing_map = probe_environment_map(config, target.existing.as_ref());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("prepare the managed SSH verification runtime")?;
    let outcome = runtime.block_on(probe::verify_management_address(
        &paths.home,
        &receipt,
        record,
        &addresses,
        &target.environment_name,
        &existing_map,
    ))?;
    let connection = target.existing.is_none().then(|| WindowsConnection {
        connection_json: receipt_json,
        name: target.environment_name.clone(),
        host: outcome.address.to_string(),
        identity_file: record.ssh_identity_file.clone(),
        confirmed_fingerprint: receipt.host_fingerprint.clone(),
        make_default: false,
    });
    Ok(PreparedImport {
        installation_id: record.installation_id.clone(),
        environment_id: record.environment_id.clone(),
        environment_name: target.environment_name,
        address: Some(outcome.address),
        candidate_addresses: receipt.candidate_addresses.clone(),
        attempts: outcome.attempts,
        existing_environment: target.existing,
        explicit_name: requested_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
        connection,
        guidance: None,
    })
}

/// Validate and verify one imported connection receipt.
pub fn prepare_import(
    home: &Path,
    config: &AppConfig,
    input: ImportInput,
) -> Result<PreparedImport> {
    let paths = SetupPaths::discover(home);
    let records = read_records(&paths)?;
    let bytes = receipt::read_input(&input.receipt)?;
    let receipt = receipt::parse(&bytes)?;
    receipt.confirm(&input.confirmed_fingerprint)?;
    let record = receipt::match_generation(&receipt, &records)?;
    import_for_record(
        &paths,
        config,
        &record,
        receipt,
        input.name.as_deref(),
        None,
    )
}

/// Re-run address selection and verification from the stored receipt.
pub fn prepare_retry(
    home: &Path,
    config: &AppConfig,
    installation_id: &str,
) -> Result<PreparedImport> {
    ensure!(
        installation_id.len() == 32
            && installation_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid worker installation identity"
    );
    let paths = SetupPaths::discover(home);
    let records = read_records(&paths)?;
    let previous = state::read(&paths.root, installation_id)?.with_context(|| {
        format!(
            "no worker media installation state exists for installation {installation_id}; import the connection receipt first"
        )
    })?;
    let receipt = receipt::retry_receipt(&previous)?;
    let confirmed = previous.confirmed_fingerprint.as_deref().context(
        "the stored installation state has no confirmed fingerprint; import the receipt again",
    )?;
    receipt.confirm(confirmed)?;
    let record = receipt::match_generation(&receipt, &records)?;
    import_for_record(&paths, config, &record, receipt, None, Some(previous))
}

/// Persist the verified connection state after the environment was saved.
pub fn mark_available(
    home: &Path,
    installation_id: &str,
    address: Ipv4Addr,
) -> Result<InstallationStatus> {
    let paths = SetupPaths::discover(home);
    Ok(state::mark_available(&paths.root, installation_id, &address.to_string())?.status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use wukong_worker_package::SshPublicKey;

    const INSTALLATION_ID: &str = "0123456789abcdef0123456789abcdef";

    fn host_key(fill: u8) -> SshPublicKey {
        let mut blob = vec![0, 0, 0, 11];
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&[0, 0, 0, 32]);
        blob.extend_from_slice(&[fill; 32]);
        SshPublicKey {
            algorithm: "ssh-ed25519".into(),
            key: base64::engine::general_purpose::STANDARD.encode(blob),
        }
    }

    fn record(root: &Path, installation_id: &str) -> GenerationRecord {
        GenerationRecord {
            schema: 1,
            installation_id: installation_id.into(),
            environment_id: environment_id_for(installation_id).expect("fixture environment id"),
            app_version: worker_media::APP_VERSION.into(),
            template_sha256: "a".repeat(64),
            payload_manifest_sha256: "b".repeat(64),
            controller_public_key: "c".repeat(64),
            ssh_identity_file: root
                .join("worker-media/registry")
                .join(installation_id)
                .join("ssh/management_ed25519"),
            management_peer: Ipv4Addr::new(192, 168, 199, 100),
            network_prefix_len: 24,
            ssh_port: 22_240,
            media_path: root
                .join("worker-media/registry")
                .join(installation_id)
                .join("media"),
            zip_path: root.join("out").join(format!("{installation_id}.zip")),
            created_at: "2026-01-01T00:00:00+00:00".into(),
        }
    }

    fn write_record(root: &Path, record: &GenerationRecord) -> Result<()> {
        let registry = root.join("worker-media/registry");
        fs::create_dir_all(&registry)?;
        fs::write(
            registry.join(format!("{}.json", record.installation_id)),
            serde_json::to_vec(record)?,
        )?;
        Ok(())
    }

    fn receipt(record: &GenerationRecord) -> ConnectionReceipt {
        let key = host_key(7);
        let fingerprint = key.fingerprint().expect("fixture fingerprint");
        ConnectionReceipt {
            schema: 1,
            receipt_version: 2,
            installed: true,
            installation_id: record.installation_id.clone(),
            environment_id: record.environment_id.clone(),
            generation: 1,
            manifest_sha256: record.payload_manifest_sha256.clone(),
            ssh_user: format!("wkgm{}", &record.installation_id[..16]),
            ssh_port: record.ssh_port,
            host_key: key,
            host_fingerprint: fingerprint,
            candidate_addresses: vec!["192.168.199.40".into()],
            production_isolation_verified: false,
        }
    }

    fn environment(record: &GenerationRecord, host: &str) -> EnvironmentConfig {
        let ssh = host_key(7);
        serde_json::from_value(serde_json::json!({
            "name": format!("Windows Worker {}", &record.installation_id[..8]),
            "platform": "windows",
            "installation_id": record.installation_id,
            "runtime": {
                "backend": "managed_ssh",
                "ssh_host": host,
                "managed_ssh": {
                    "installation_id": record.installation_id,
                    "user": format!("wkgm{}", &record.installation_id[..16]),
                    "port": record.ssh_port,
                    "identity_file": record.ssh_identity_file,
                    "known_hosts_file": format!("C:\\state\\ssh-host-{}.known_hosts", record.installation_id),
                    "host_key": ssh,
                },
            },
        }))
        .expect("fixture environment")
    }

    fn network(id: &str, ipv4: &str, prefix_len: u8, recommended: bool) -> ManagementNetwork {
        ManagementNetwork {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            ipv4: ipv4.parse().expect("fixture address"),
            prefix_len,
            rfc1918: true,
            virtual_adapter: recommended,
            recommended,
        }
    }

    #[test]
    fn environment_ids_and_default_names_use_the_installation_identity() {
        assert_eq!(
            environment_id_for(INSTALLATION_ID).expect("environment id"),
            "windows-01234567"
        );
        assert_eq!(
            default_environment_name(INSTALLATION_ID),
            "Windows Worker 01234567"
        );
        assert!(environment_id_for("short").is_err());
        assert!(validate_environment_name("   ").is_err());
        assert!(validate_environment_name(&"n".repeat(257)).is_err());
    }

    #[test]
    fn network_selection_is_explicit_when_several_networks_exist() {
        let one = vec![network("vmnet8", "192.168.199.1", 24, true)];
        assert_eq!(
            select_network(&one, None).expect("single network").id,
            "vmnet8"
        );
        let several = vec![
            network("vmnet8", "192.168.199.1", 24, true),
            network("wifi", "192.168.1.20", 24, false),
        ];
        let error = select_network(&several, None).unwrap_err();
        assert!(format!("{error:#}").contains("multiple IPv4 management networks"));
        assert_eq!(
            select_network(&several, Some("wifi"))
                .expect("explicit id")
                .ipv4
                .to_string(),
            "192.168.1.20"
        );
        assert_eq!(
            select_network(&several, Some("192.168.199.1"))
                .expect("explicit ipv4")
                .id,
            "vmnet8"
        );
        assert_eq!(
            select_network(&several, Some("192.168.199.1/24"))
                .expect("explicit cidr")
                .id,
            "vmnet8"
        );
        assert!(select_network(&several, Some("10.0.0.1")).is_err());
        assert!(select_network(&[], None).is_err());
    }

    #[test]
    fn registration_targets_update_an_installation_and_refuse_foreign_ones() -> Result<()> {
        let root = tempfile::tempdir()?;
        let record = record(root.path(), INSTALLATION_ID);
        let receipt = receipt(&record);
        let mut config = AppConfig::default();
        let target = registration_target(&config, &record, &receipt, None, None)?;
        assert!(target.existing.is_none());
        assert_eq!(target.environment_name, "Windows Worker 01234567");

        config.environments.insert(
            "windows-01234567".into(),
            environment(&record, "192.168.199.40"),
        );
        config.default_environment = Some("windows-01234567".into());
        let target = registration_target(&config, &record, &receipt, Some("VM 7"), None)?;
        assert_eq!(target.environment_name, "VM 7");
        assert_eq!(
            target
                .existing
                .as_ref()
                .map(|existing| existing.environment_id.as_str()),
            Some("windows-01234567")
        );

        let mut foreign = config.clone();
        let mut mutated = environment(&record, "192.168.199.40");
        mutated
            .runtime
            .managed_ssh_mut()
            .expect("managed ssh")
            .host_key = host_key(9);
        foreign
            .environments
            .insert("windows-01234567".into(), mutated);
        let error = registration_target(&foreign, &record, &receipt, None, None).unwrap_err();
        assert!(format!("{error:#}").contains("different worker host key"));

        // A registration under another environment id still serves the same
        // installation and is updated instead of duplicated.
        let mut renamed = AppConfig::default();
        renamed
            .environments
            .insert("legacy".into(), environment(&record, "192.168.199.40"));
        renamed.default_environment = Some("legacy".into());
        let target = registration_target(&renamed, &record, &receipt, None, None)?;
        assert_eq!(
            target
                .existing
                .as_ref()
                .map(|existing| existing.environment_id.as_str()),
            Some("legacy")
        );
        Ok(())
    }

    #[test]
    fn status_reports_the_generated_pending_and_available_states() -> Result<()> {
        let root = tempfile::tempdir()?;
        let record = record(root.path(), INSTALLATION_ID);
        write_record(root.path(), &record)?;
        let mut config = AppConfig::default();
        let report = status(root.path(), &config)?;
        assert_eq!(report.environments.len(), 0);
        assert_eq!(report.installations.len(), 1);
        assert_eq!(report.installations[0].state, InstallationStatus::Generated);
        assert!(!report.installations[0].zip_present);
        assert_eq!(
            report.installations[0].environment_name,
            "Windows Worker 01234567"
        );

        fs::create_dir_all(&record.media_path)?;
        fs::create_dir_all(record.zip_path.parent().expect("fixture zip parent"))?;
        fs::write(&record.zip_path, b"archive")?;
        let report = status(root.path(), &config)?;
        assert!(report.installations[0].zip_present);
        assert!(report.installations[0].media_present);

        state::generated(
            &root.path().join(ROOT_DIR),
            &record,
            "Windows Worker 01234567",
        )?;
        state::remember_receipt(
            &root.path().join(ROOT_DIR),
            &record,
            "Windows Worker 01234567",
            &receipt(&record),
        )?;
        let report = status(root.path(), &config)?;
        assert_eq!(
            report.installations[0].state,
            InstallationStatus::InstalledPending
        );
        assert_eq!(
            report.installations[0].candidate_addresses,
            vec!["192.168.199.40".to_string()]
        );

        config.environments.insert(
            record.environment_id.clone(),
            environment(&record, "192.168.199.40"),
        );
        config.default_environment = Some(record.environment_id.clone());
        state::mark_available(
            &root.path().join(ROOT_DIR),
            &record.installation_id,
            "192.168.199.40",
        )?;
        let report = status(root.path(), &config)?;
        assert_eq!(report.installations[0].state, InstallationStatus::Available);
        assert_eq!(
            report.installations[0].registered_environment_id.as_deref(),
            Some("windows-01234567")
        );
        assert_eq!(
            report.environments[0].worker_media_state,
            Some(InstallationStatus::Available)
        );

        // Removing the registered environment downgrades a stale verification.
        config.environments.clear();
        config.default_environment = None;
        let report = status(root.path(), &config)?;
        assert_eq!(
            report.installations[0].state,
            InstallationStatus::InstalledPending
        );
        assert!(report.installations[0].warning.is_some());
        Ok(())
    }

    #[test]
    fn status_distinguishes_missing_corrupt_and_unverified_deployments() -> Result<()> {
        let root = tempfile::tempdir()?;
        let paths = SetupPaths::discover(root.path());
        let config = AppConfig::default();
        // No registry yet: an empty inventory, not an unreadable one.
        let report = status(root.path(), &config)?;
        assert_eq!(report.diagnostics.inventory, RecordState::Missing);
        assert!(report.diagnostics.unreadable_generation_records.is_empty());
        assert!(report.installations.is_empty());

        let record = record(root.path(), INSTALLATION_ID);
        write_record(root.path(), &record)?;
        let report = status(root.path(), &config)?;
        assert_eq!(report.diagnostics.inventory, RecordState::Present);
        let view = &report.installations[0];
        assert_eq!(view.diagnostics.generation_record, RecordState::Present);
        assert_eq!(view.diagnostics.recovery_record, RecordState::Missing);
        assert_eq!(view.diagnostics.receipt, RecordState::Missing);
        assert_eq!(view.diagnostics.registration, RecordState::Missing);
        assert_eq!(view.diagnostics.verification, VerificationState::NotStarted);
        assert!(view.diagnostics.detail.is_none());

        // Corruption is distinct from absence and never hides the generation record.
        let sidecar = state::state_path(&paths.root, INSTALLATION_ID);
        fs::create_dir_all(sidecar.parent().expect("sidecar directory"))?;
        fs::write(&sidecar, b"{not-json")?;
        let report = status(root.path(), &config)?;
        let view = &report.installations[0];
        assert_eq!(view.diagnostics.recovery_record, RecordState::Corrupt);
        assert_eq!(view.diagnostics.receipt, RecordState::Unknown);
        assert_eq!(view.diagnostics.verification, VerificationState::Unknown);
        assert!(view.diagnostics.detail.is_some());
        assert!(view.warning.is_some());
        fs::remove_file(&sidecar)?;

        // An imported receipt without a matching registration is unverified.
        state::generated(&paths.root, &record, "Windows Worker 01234567")?;
        state::remember_receipt(
            &paths.root,
            &record,
            "Windows Worker 01234567",
            &receipt(&record),
        )?;
        let report = status(root.path(), &config)?;
        let view = &report.installations[0];
        assert_eq!(view.diagnostics.recovery_record, RecordState::Present);
        assert_eq!(view.diagnostics.receipt, RecordState::Present);
        assert_eq!(view.diagnostics.registration, RecordState::Missing);
        assert_eq!(view.diagnostics.verification, VerificationState::Incomplete);
        assert!(view.diagnostics.detail.is_none());

        // A matching registered environment completes verification.
        state::mark_available(&paths.root, &record.installation_id, "192.168.199.40")?;
        let mut config = AppConfig::default();
        config.environments.insert(
            record.environment_id.clone(),
            environment(&record, "192.168.199.40"),
        );
        config.default_environment = Some(record.environment_id.clone());
        let report = status(root.path(), &config)?;
        let view = &report.installations[0];
        assert_eq!(view.diagnostics.registration, RecordState::Present);
        assert_eq!(view.diagnostics.verification, VerificationState::Verified);

        // One corrupt generation record is listed without hiding the readable one.
        let unreadable = paths.registry.join(format!("{}.json", "f".repeat(32)));
        fs::write(&unreadable, b"not-json")?;
        let report = status(root.path(), &config)?;
        assert_eq!(report.diagnostics.inventory, RecordState::Present);
        assert_eq!(report.diagnostics.unreadable_generation_records.len(), 1);
        assert_eq!(
            report.diagnostics.unreadable_generation_records[0]
                .installation_id
                .as_deref(),
            Some("ffffffffffffffffffffffffffffffff")
        );
        assert_eq!(report.installations.len(), 1);
        assert_eq!(
            report.installations[0].diagnostics.verification,
            VerificationState::Verified
        );
        Ok(())
    }

    #[test]
    fn a_retry_without_a_stored_receipt_explains_the_next_step() -> Result<()> {
        let root = tempfile::tempdir()?;
        let record = record(root.path(), INSTALLATION_ID);
        write_record(root.path(), &record)?;
        let config = AppConfig::default();
        let error = prepare_retry(root.path(), &config, &record.installation_id).unwrap_err();
        assert!(format!("{error:#}").contains("no worker media installation state"));
        state::generated(
            &root.path().join(ROOT_DIR),
            &record,
            "Windows Worker 01234567",
        )?;
        let error = prepare_retry(root.path(), &config, &record.installation_id).unwrap_err();
        assert!(format!("{error:#}").contains("no imported connection receipt"));
        assert!(prepare_retry(root.path(), &config, "not-an-installation").is_err());
        Ok(())
    }
}
