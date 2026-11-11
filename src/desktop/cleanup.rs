//! Isolated review window: no Store, model, worker startup or workspace commands.
use crate::cleanup::{Cleanup, Plan, Selection};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::{path::PathBuf, sync::Mutex};
use tauri::State;
struct Session {
    cleanup: Cleanup,
    output: PathBuf,
    plan: Mutex<Option<Plan>>,
    revision: std::sync::atomic::AtomicU64,
}
type Reply<T> = std::result::Result<T, String>;
#[derive(Serialize)]
struct Preview {
    fingerprint: String,
    files: usize,
    bytes: u64,
    paths: Vec<PathBuf>,
    resources: Vec<crate::cleanup::Resource>,
    retained: Vec<crate::cleanup::Retained>,
    errors: Vec<String>,
    truncated: bool,
}
#[tauri::command]
async fn cleanup_preview(session: State<'_, Session>, selection: Selection) -> Reply<Preview> {
    let revision = session
        .revision
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        + 1;
    let cleanup = Cleanup::at(&session.cleanup.home).map_err(|e| e.to_string())?;
    let plan = tauri::async_runtime::spawn_blocking(move || cleanup.plan(selection))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let paths: std::collections::BTreeSet<_> = plan
        .files
        .iter()
        .filter_map(|entry| entry.path.parent().map(PathBuf::from))
        .collect();
    let preview = Preview {
        fingerprint: plan.fingerprint.clone(),
        files: plan.files.len(),
        bytes: plan.bytes,
        truncated: paths.len() > 200 || plan.resources.len() > 200 || plan.retained.len() > 200,
        paths: paths.into_iter().take(200).collect(),
        resources: plan.resources.iter().take(200).cloned().collect(),
        retained: plan.retained.iter().take(200).cloned().collect(),
        errors: plan.errors.clone(),
    };
    let mut current = session.plan.lock().unwrap();
    if revision != session.revision.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("outdated cleanup preview".into());
    }
    *current = Some(plan);
    Ok(preview)
}
#[tauri::command]
fn cleanup_confirm(
    session: State<'_, Session>,
    fingerprint: String,
    app: tauri::AppHandle,
) -> Reply<()> {
    let guard = session.plan.lock().unwrap();
    let plan = guard.as_ref().ok_or("review the cleanup scope first")?;
    if plan.fingerprint != fingerprint || !plan.errors.is_empty() {
        return Err("cleanup preview changed or contains errors; refresh the review".into());
    }
    crate::cleanup::check_output(plan, &session.output).map_err(|e| e.to_string())?;
    crate::workbench::write_json(&session.output, plan).map_err(|e| e.to_string())?;
    let mask = i32::from(plan.selection.data)
        + 2 * i32::from(plan.selection.tools)
        + 4 * i32::from(plan.selection.challenges);
    app.exit(32 + mask);
    Ok(())
}
#[tauri::command]
fn cleanup_cancel(app: tauri::AppHandle) {
    app.exit(64);
}
#[tauri::command]
fn preferences_read() -> Reply<super::preferences::Snapshot> {
    super::preferences::read().map_err(|e| e.to_string())
}
pub fn window(output: PathBuf, initial_data: bool, browser: PathBuf) -> Result<()> {
    ensure!(
        output.is_absolute(),
        "review output must be an absolute path outside application data"
    );
    let cleanup = Cleanup::current()?;
    ensure!(
        !output.starts_with(&cleanup.home),
        "review output must be outside application data"
    );
    crate::cleanup::review::validate_profile(&browser)?;
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    tauri::Builder::default()
        .manage(Session {
            cleanup,
            output,
            plan: Mutex::new(None),
            revision: std::sync::atomic::AtomicU64::new(0),
        })
        .invoke_handler(tauri::generate_handler![
            preferences_read,
            cleanup_preview,
            cleanup_confirm,
            cleanup_cancel
        ])
        .setup(move |app| {
            let development = app.config().build.dev_url.clone();
            tauri::WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])?
                .title("Wukong · Uninstall / 卸载")
                .inner_size(860., 680.)
                .min_inner_size(640., 480.)
                .data_directory(browser)
                .initialization_script(format!(
                    "window.__WUKONG_CLEANUP__={{initialData:{initial_data}}};"
                ))
                .on_navigation(move |url| {
                    (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                        || (matches!(url.scheme(), "http" | "https")
                            && url.host_str() == Some("tauri.localhost"))
                        || (cfg!(debug_assertions)
                            && development
                                .as_ref()
                                .is_some_and(|dev| dev.origin() == url.origin()))
                })
                .build()?;
            Ok(())
        })
        .build(tauri::generate_context!())?
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } = event
            {
                api.prevent_exit();
                app.exit(64);
            }
        });
    Ok(())
}
