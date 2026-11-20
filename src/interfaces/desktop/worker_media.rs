//! Desktop commands for the offline Windows worker setup workflow.
//!
//! These commands are thin orchestration: they validate the workspace epoch,
//! move blocking work off the webview thread, delegate every trust decision to
//! the shared media generator, the signed release catalog and the managed-SSH
//! registration adapter, and only save the configuration after the application
//! layer verified the candidate connection. A failed import therefore never
//! leaves a partially registered environment behind.
use super::{
    configuration::{ConfigDocument, ConfigEdit},
    host::blocking,
    service::Service,
};
use crate::adapters::net::ManagementNetwork;
use crate::{
    application::windows_worker_setup::{
        self as setup, GenerateInput, GenerateReport, ImportEnvironment, ImportInput, ImportReport,
        PreparedImport, StatusReport,
    },
    application::worker_media::APP_VERSION,
};
use anyhow::{Context, Result};
use std::{path::Path, path::PathBuf, sync::Arc};
use tauri::State;

type Reply<T> = std::result::Result<T, crate::i18n::Message>;

fn failure(error: impl Into<anyhow::Error>) -> crate::i18n::Message {
    crate::i18n::Message::from_error(error.into())
}

fn home() -> Result<PathBuf> {
    super::paths::home()
}

/// Report generated installations and registered environments.
#[tauri::command]
pub(crate) async fn worker_media_status(
    service: State<'_, Arc<Service>>,
    epoch: u64,
) -> Reply<StatusReport> {
    blocking(service.inner().clone(), move |service| {
        let document = service.current_configuration(epoch)?;
        setup::status(&home()?, &document.form)
    })
    .await
}

/// Enumerate this host's eligible management networks.
#[tauri::command]
pub(crate) async fn worker_media_networks() -> Reply<Vec<ManagementNetwork>> {
    tauri::async_runtime::spawn_blocking(setup::networks)
        .await
        .map_err(failure)?
        .map_err(failure)
}

/// Generate one finalized, controller-credential-free installation archive.
#[tauri::command]
pub(crate) async fn worker_media_generate(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    template: Option<PathBuf>,
    output: PathBuf,
    environment_name: Option<String>,
    network: Option<String>,
) -> Reply<GenerateReport> {
    blocking(service.inner().clone(), move |service| {
        // Validate the workspace epoch before the long-running generation.
        service.current_configuration(epoch)?;
        setup::generate(
            &home()?,
            GenerateInput {
                template,
                output,
                environment_name,
                network,
            },
        )
    })
    .await
}

/// Import a public connection receipt and register the verified environment.
#[tauri::command]
pub(crate) async fn worker_media_import(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    receipt: String,
    confirmed_fingerprint: String,
    name: Option<String>,
) -> Reply<ImportReport> {
    blocking(service.inner().clone(), move |service| {
        let document = service.current_configuration(epoch)?;
        let home = home()?;
        let prepared = setup::prepare_import(
            &home,
            &document.form,
            ImportInput {
                receipt,
                confirmed_fingerprint,
                name,
            },
        )?;
        commit(service, epoch, &home, document, prepared)
    })
    .await
}

/// Re-run address selection and verification from the previously imported receipt.
#[tauri::command]
pub(crate) async fn worker_media_retry(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    installation_id: String,
) -> Reply<ImportReport> {
    blocking(service.inner().clone(), move |service| {
        let document = service.current_configuration(epoch)?;
        let home = home()?;
        let prepared = setup::prepare_retry(&home, &document.form, &installation_id)?;
        commit(service, epoch, &home, document, prepared)
    })
    .await
}

/// Save the verified environment through the existing configuration pipeline and
/// only then persist the available state. Any failure before the save leaves the
/// previous configuration in force.
fn commit(
    service: &Service,
    epoch: u64,
    home: &Path,
    document: ConfigDocument,
    mut prepared: PreparedImport,
) -> Result<ImportReport> {
    let Some(address) = prepared.address else {
        return Ok(ImportReport::pending(&prepared));
    };
    let environment_id = match &prepared.existing_environment {
        Some(existing) => {
            let mut edits = Vec::new();
            if existing.ssh_host != address.to_string() {
                edits.push(ConfigEdit {
                    path: vec![
                        "environments".into(),
                        existing.environment_id.clone(),
                        "runtime".into(),
                        "ssh_host".into(),
                    ],
                    value: serde_json::json!(address.to_string()),
                });
            }
            if let Some(name) = &prepared.explicit_name
                && *name != existing.name
            {
                edits.push(ConfigEdit {
                    path: vec![
                        "environments".into(),
                        existing.environment_id.clone(),
                        "name".into(),
                    ],
                    value: serde_json::json!(name),
                });
            }
            // No-op when the previously verified address and name still match:
            // re-import must not rewrite unrelated configuration.
            if !edits.is_empty() {
                service.save_configuration(epoch, document, edits)?;
            }
            existing.environment_id.clone()
        }
        None => {
            let connection = prepared
                .connection
                .take()
                .context("the verified import has no registration input")?;
            let mut document = document;
            document.import_windows_connection(home, &connection, &[])?;
            service.save_configuration(epoch, document, Vec::new())?;
            prepared.environment_id.clone()
        }
    };
    setup::mark_available(home, &prepared.installation_id, address)?;
    Ok(ImportReport::verified(
        &prepared,
        address,
        Some(ImportEnvironment {
            environment_id,
            name: prepared.environment_name.clone(),
            ssh_host: Some(address.to_string()),
        }),
    ))
}

fn dialog_title(key: &str, fallback: &str) -> String {
    let locale = super::preferences::locale();
    if locale.has(key) {
        locale.text(key)
    } else {
        fallback.to_owned()
    }
}

/// Pick a local ZIP template (fallback when the release catalog has none).
#[tauri::command]
pub(crate) async fn pick_worker_template() -> Reply<Option<PathBuf>> {
    tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title(dialog_title(
                "dialog-worker-template",
                "Select the Windows worker media template",
            ))
            .add_filter("ZIP archive", &["zip"])
            .pick_file()
    })
    .await
    .map_err(failure)
}

/// Pick the destination path of the generated media ZIP.
#[tauri::command]
pub(crate) async fn pick_worker_output() -> Reply<Option<PathBuf>> {
    tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title(dialog_title(
                "dialog-worker-output",
                "Save the Windows worker media archive",
            ))
            .add_filter("ZIP archive", &["zip"])
            .set_file_name(format!("wukong-windows-worker-media-{APP_VERSION}.zip"))
            .save_file()
    })
    .await
    .map_err(failure)
}

/// Pick the `connection.json` exported by the Windows setup.
#[tauri::command]
pub(crate) async fn pick_worker_receipt() -> Reply<Option<PathBuf>> {
    tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title(dialog_title(
                "dialog-worker-receipt",
                "Select the saved Windows worker connection receipt",
            ))
            .add_filter("JSON", &["json"])
            .pick_file()
    })
    .await
    .map_err(failure)
}
