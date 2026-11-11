//! A small native boundary: blocking database/dialog work never runs on the webview thread.
use super::{
    configuration::{ConfigDocument, ConfigEdit},
    service::{EvidenceView, Service, WorkspaceEnvelope},
    workspace::WorkspaceRequest,
};
use crate::{config::AppConfig, domain::*, queries::FindingDetails, store::Store};
use serde::Serialize;
use std::{path::PathBuf, sync::Arc};
use tauri::{Emitter, Manager, State};

type Reply<T> = std::result::Result<T, crate::i18n::Message>;
fn failure(error: impl Into<anyhow::Error>) -> crate::i18n::Message {
    crate::i18n::Message::from_error(error.into())
}
#[tauri::command]
async fn preferences_read() -> Reply<super::preferences::Snapshot> {
    super::preferences::read().map_err(failure)
}
#[tauri::command]
async fn preferences_save(
    preferences: super::preferences::Preferences,
    app: tauri::AppHandle,
) -> Reply<super::preferences::Snapshot> {
    let saved = super::preferences::write(&preferences).map_err(failure)?;
    if let Some(window) = app.get_webview_window("main") {
        window
            .set_title(
                &preferences
                    .ui_language
                    .resolve(saved.system_locale)
                    .text("app-title"),
            )
            .map_err(failure)?;
    }
    Ok(saved)
}
async fn blocking<T: Send + 'static>(
    service: Arc<Service>,
    work: impl FnOnce(&Service) -> anyhow::Result<T> + Send + 'static,
) -> Reply<T> {
    tauri::async_runtime::spawn_blocking(move || work(&service))
        .await
        .map_err(failure)?
        .map_err(failure)
}
#[tauri::command]
async fn workbench_action(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    request: crate::workbench::DesktopRequest,
) -> Reply<serde_json::Value> {
    blocking(service.inner().clone(), move |s| {
        s.workbench(epoch, request)
    })
    .await
}
#[tauri::command]
async fn update_action(
    service: State<'_, Arc<Service>>,
    controller: State<'_, Arc<super::updates::Controller>>,
    app: tauri::AppHandle,
    epoch: u64,
    action: super::updates::Action,
) -> Reply<super::updates::Status> {
    controller
        .inner()
        .action(action, epoch, service.inner().clone(), app)
        .await
        .map_err(failure)
}
#[tauri::command]
async fn update_cancel(
    controller: State<'_, Arc<super::updates::Controller>>,
    operation_id: String,
) -> Reply<super::updates::Status> {
    controller.cancel(&operation_id).map_err(failure)
}
#[tauri::command]
async fn update_log_directory(
    controller: State<'_, Arc<super::updates::Controller>>,
    app: tauri::AppHandle,
    operation_id: String,
) -> Reply<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = controller.log_directory(&operation_id).map_err(failure)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(failure)
}
#[tauri::command]
async fn update_diagnostics(
    controller: State<'_, Arc<super::updates::Controller>>,
    operation_id: String,
) -> Reply<Option<PathBuf>> {
    let state = controller
        .diagnostic_snapshot(&operation_id)
        .map_err(failure)?;
    tokio::task::spawn_blocking(move || super::update_diagnostics::export(&state))
        .await
        .map_err(failure)?
        .map_err(failure)
}
#[tauri::command]
async fn update_storage(service: State<'_, Arc<Service>>, epoch: u64) -> Reply<serde_json::Value> {
    service.current_configuration(epoch).map_err(failure)?;
    Ok(
        serde_json::json!({"data_dir":service.status().state_dir, "cache_dir":crate::updates::storage::cache_root().map_err(failure)?}),
    )
}
#[tauri::command]
async fn update_cache_directory(
    service: State<'_, Arc<Service>>,
    controller: State<'_, Arc<super::updates::Controller>>,
    epoch: u64,
    destination: PathBuf,
) -> Reply<PathBuf> {
    controller
        .relocate_cache(destination, epoch, service.inner().clone())
        .await
        .map_err(failure)
}
#[tauri::command]
async fn pick_directory() -> Reply<Option<PathBuf>> {
    tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title(super::preferences::locale().text("dialog-directory"))
            .pick_folder()
    })
    .await
    .map_err(failure)
}
#[tauri::command]
async fn update_data_directory(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    destination: PathBuf,
) -> Reply<()> {
    blocking(service.inner().clone(), move |s| {
        s.set_data_directory(epoch, destination)
    })
    .await
}
#[tauri::command]
async fn workspace_read(
    service: State<'_, Arc<Service>>,
    request: WorkspaceRequest,
) -> Reply<WorkspaceEnvelope> {
    blocking(service.inner().clone(), move |s| s.read(request)).await
}
#[tauri::command]
async fn start_run(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    target: String,
    task: String,
) -> Reply<String> {
    service
        .inner()
        .clone()
        .start(epoch, target, task, None)
        .await
        .map_err(failure)
}
#[tauri::command]
async fn resume_run(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    run: String,
    budget: Budget,
) -> Reply<String> {
    service
        .inner()
        .clone()
        .start(epoch, String::new(), String::new(), Some((run, budget)))
        .await
        .map_err(failure)
}
#[tauri::command]
async fn pause_run(service: State<'_, Arc<Service>>, epoch: u64, run: String) -> Reply<()> {
    blocking(service.inner().clone(), move |s| s.pause(epoch, &run)).await
}
#[tauri::command]
async fn stop_agent(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    run: String,
    agent: String,
) -> Reply<()> {
    blocking(service.inner().clone(), move |s| {
        s.stop(epoch, &run, &agent)
    })
    .await
}
#[tauri::command]
async fn send_message(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    run: String,
    agent: String,
    text: String,
    follow_up: bool,
    generation: u64,
) -> Reply<MessageReceipt> {
    blocking(service.inner().clone(), move |s| {
        s.message(epoch, &run, &agent, &text, follow_up, generation)
    })
    .await
}
#[tauri::command]
async fn pick_target() -> Reply<Option<PathBuf>> {
    tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title(super::preferences::locale().text("dialog-target"))
            .pick_file()
    })
    .await
    .map_err(failure)
}
#[tauri::command]
async fn export_report(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    run: String,
) -> Reply<Option<PathBuf>> {
    blocking(service.inner().clone(), move |s| s.export(epoch, &run)).await
}
#[tauri::command]
async fn config_read(service: State<'_, Arc<Service>>, epoch: u64) -> Reply<ConfigDocument> {
    blocking(service.inner().clone(), move |s| {
        s.current_configuration(epoch)
    })
    .await
}
#[tauri::command]
async fn config_open(
    service: State<'_, Arc<Service>>,
    epoch: u64,
) -> Reply<Option<ConfigDocument>> {
    blocking(service.inner().clone(), move |s| {
        s.configuration(epoch, None)
    })
    .await
}
#[tauri::command]
async fn config_preview(
    mut document: ConfigDocument,
    edits: Vec<ConfigEdit>,
) -> Reply<ConfigDocument> {
    tauri::async_runtime::spawn_blocking(move || {
        document.patch(&edits)?;
        Ok::<_, anyhow::Error>(document)
    })
    .await
    .map_err(failure)?
    .map_err(failure)
}
#[tauri::command]
async fn config_save(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    document: ConfigDocument,
    edits: Vec<ConfigEdit>,
) -> Reply<ConfigDocument> {
    blocking(service.inner().clone(), move |s| {
        s.save_configuration(epoch, document, edits)
    })
    .await
}
#[tauri::command]
async fn finding_details(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    run: String,
    id: String,
) -> Reply<FindingDetails> {
    blocking(service.inner().clone(), move |s| {
        s.finding(epoch, &run, &id)
    })
    .await
}
#[tauri::command]
async fn evidence_preview(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    run: String,
    id: String,
    offset: u64,
) -> Reply<EvidenceView> {
    blocking(service.inner().clone(), move |s| {
        s.evidence(epoch, &run, &id, offset)
    })
    .await
}
#[tauri::command]
async fn tool_details(
    service: State<'_, Arc<Service>>,
    epoch: u64,
    run: String,
    id: String,
) -> Reply<ToolCallRecord> {
    blocking(service.inner().clone(), move |s| s.call(epoch, &run, &id)).await
}
#[tauri::command]
async fn shutdown_workspace(service: State<'_, Arc<Service>>, app: tauri::AppHandle) -> Reply<()> {
    service.shutdown().await.map_err(failure)?;
    app.exit(0);
    Ok(())
}
#[derive(Clone, Serialize)]
struct CloseRequest {
    busy: bool,
}
pub fn run(
    config: AppConfig,
    store: Store,
    selected: Option<String>,
    path: Option<PathBuf>,
    state_override: Option<PathBuf>,
) -> anyhow::Result<()> {
    if let Some(id) = &selected {
        store.require::<RunRecord>("run", id)?;
    }
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let controller = Arc::new(super::updates::Controller::default());
    controller.initialization(&config);
    let service = Service::new(config, store, selected, path, state_override);
    let updates = service.clone();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_updater::Builder::new()
                .pubkey(crate::updates::manifest::PUBLIC_KEY.trim())
                .build(),
        )
        .manage(controller.clone())
        .manage(service)
        .invoke_handler(tauri::generate_handler![
            preferences_read,
            preferences_save,
            workspace_read,
            update_action,
            update_cancel,
            update_log_directory,
            update_diagnostics,
            update_storage,
            update_cache_directory,
            update_data_directory,
            pick_directory,
            workbench_action,
            start_run,
            resume_run,
            pause_run,
            stop_agent,
            send_message,
            pick_target,
            export_report,
            config_read,
            config_open,
            config_preview,
            config_save,
            finding_details,
            evidence_preview,
            tool_details,
            shutdown_workspace
        ])
        .setup(move |app| {
            let confirmation_service = updates.clone();
            tauri::async_runtime::spawn(async move {
                controller.confirm(confirmation_service).await;
            });
            let development = app.config().build.dev_url.clone();
            tauri::WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])?
                .title(super::preferences::locale().text("app-title"))
                .on_navigation(move |url| {
                    (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                        || (matches!(url.scheme(), "http" | "https")
                            && url.host_str() == Some("tauri.localhost"))
                        || (cfg!(debug_assertions)
                            && development
                                .as_ref()
                                .is_some_and(|development| url.origin() == development.origin()))
                })
                .build()?;
            let handle = app.handle().clone();
            let mut changed = updates.subscribe();
            tauri::async_runtime::spawn(async move {
                while changed.changed().await.is_ok() {
                    // Notifications carry no payload; the webview requests a scoped snapshot.
                    let _ = handle.emit("workspace-changed", ());
                    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let busy = window.state::<Arc<Service>>().busy();
                let _ = window.emit("close-requested", CloseRequest { busy });
            }
        })
        .build(tauri::generate_context!())?;
    app.run(|app, event| {
        // OS Quit / Cmd+Q follows the same save boundary as closing the window.
        // An explicit shutdown command exits with Some(0) after awaiting cleanup.
        if let tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            api.prevent_exit();
            let busy = app.state::<Arc<Service>>().busy();
            let _ = app.emit("close-requested", CloseRequest { busy });
        }
    });
    Ok(())
}
