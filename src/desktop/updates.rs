//! Desktop update orchestration. Network preparation never switches the live worker.
use super::service::Service;
use crate::i18n::Message as UiMessage;
use crate::updates::{
    environment, maintenance,
    manifest::{self, Manifest},
    recovery,
};
use crate::{config::AppConfig, workbench::Workbench};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri_plugin_updater::UpdaterExt;

#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub operation_id: Option<String>,
    pub diagnosis: Option<super::update_diagnostics::Diagnosis>,
    pub log_available: bool,
    #[serde(skip)]
    diagnostic_log: Option<PathBuf>,
    pub revision: u64,
    pub busy: bool,
    pub cancellable: bool,
    pub cancelling: bool,
    #[serde(skip)]
    cancel: tokio_util::sync::CancellationToken,
    pub action: Option<Action>,
    pub version: String,
    pub phase: String,
    pub message: UiMessage,
    pub available: Option<String>,
    pub notes: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub prepared: Option<String>,
    pub recovery: Option<PathBuf>,
    pub needs_setup: bool,
    pub environment_step: Option<String>,
    pub image_progress: Option<crate::workbench::pull_progress::PullProgress>,
    pub image_version: Option<String>,
    pub pending: bool,
    pub health: super::update_health::Health,
}
pub struct Controller {
    status: Mutex<Status>,
    operation: tokio::sync::Mutex<()>,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Status,
    Health,
    Doctor,
    StartTools,
    StopTools,
    SelectTools,
    Check,
    Prepare,
    Setup,
    Install,
    Recover,
    Integrate,
    CleanCache,
    CleanImages,
}

struct StatusEdit<'a>(std::sync::MutexGuard<'a, Status>);
impl std::ops::Deref for StatusEdit<'_> {
    type Target = Status;
    fn deref(&self) -> &Status {
        &self.0
    }
}
impl std::ops::DerefMut for StatusEdit<'_> {
    fn deref_mut(&mut self) -> &mut Status {
        &mut self.0
    }
}
impl Drop for StatusEdit<'_> {
    fn drop(&mut self) {
        self.0.revision += 1;
    }
}

impl Default for Controller {
    fn default() -> Self {
        Self {
            status: Mutex::new(Status {
                version: env!("CARGO_PKG_VERSION").into(),
                phase: "idle".into(),
                ..Default::default()
            }),
            operation: tokio::sync::Mutex::new(()),
        }
    }
}
impl Controller {
    pub async fn relocate_cache(
        &self,
        destination: PathBuf,
        epoch: u64,
        service: Arc<Service>,
    ) -> Result<PathBuf> {
        let _serial = self.operation.try_lock().context(UiMessage::new(
            "system-an-update-operation-is-already-running",
        ))?;
        let (_config, _path, _operation) = service.maintenance_context(epoch)?;
        self.begin(None);
        self.phase(
            "storage",
            UiMessage::new("system-validating-and-copying-the-update-cache"),
        );
        let result =
            tokio::task::spawn_blocking(move || crate::updates::storage::set_cache(&destination))
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result);
        self.edit().busy = false;
        match &result {
            Ok(_) => self.phase(
                "idle",
                UiMessage::new("system-cache-location-saved-original-files-are-retained-for"),
            ),
            Err(error) => self.phase(
                "error",
                UiMessage::new("system-cache-failed").detail(format!("{error:#}")),
            ),
        }
        result
    }
    fn phase(&self, phase: &str, message: UiMessage) {
        let mut state = self.edit();
        state.phase = phase.into();
        state.message = message;
        if matches!(phase, "docker" | "pulling" | "validating" | "activating") {
            state.environment_step = Some(phase.into());
        }
    }
    fn edit(&self) -> StatusEdit<'_> {
        StatusEdit(self.status.lock().unwrap())
    }
    fn begin(&self, action: Option<Action>) {
        let mut state = self.edit();
        state.operation_id = Some(uuid::Uuid::new_v4().to_string());
        state.action = action;
        state.busy = true;
        state.cancellable = matches!(action, Some(Action::Setup | Action::Prepare));
        state.cancelling = false;
        state.cancel = tokio_util::sync::CancellationToken::new();
        state.phase = "starting".into();
        state.message = UiMessage::default();
        state.diagnosis = None;
        state.diagnostic_log = None;
        state.log_available = false;
    }
    pub fn diagnostic_snapshot(&self, operation_id: &str) -> Result<Status> {
        let state = self.status();
        ensure!(
            state.operation_id.as_deref() == Some(operation_id),
            "diagnostic operation changed; refresh status"
        );
        Ok(state)
    }
    pub fn log_directory(&self, operation_id: &str) -> Result<PathBuf> {
        let state = self.diagnostic_snapshot(operation_id)?;
        super::update_diagnostics::log_directory(
            state
                .diagnostic_log
                .as_deref()
                .context("no local log for this operation")?,
            &crate::updates::storage::cache_root()?,
        )
    }
    pub fn cancel(&self, operation_id: &str) -> Result<Status> {
        let mut state = self.edit();
        ensure!(
            state.operation_id.as_deref() == Some(operation_id) && state.busy && state.cancellable,
            UiMessage::new("system-operation-not-cancellable")
        );
        state.cancel.cancel();
        state.cancelling = true;
        drop(state);
        Ok(self.status())
    }
    fn seal_preparation(&self) -> Result<()> {
        let mut state = self.edit();
        crate::workbench::cancellation::check(Some(&state.cancel))?;
        state.cancellable = false;
        Ok(())
    }
    async fn cancellable<T>(
        &self,
        future: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        let token = self.status().cancel;
        tokio::select! {
            biased;
            _ = token.cancelled() => Err(crate::workbench::cancellation::Cancelled.into()),
            result = future => result,
        }
    }
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn initialization(&self, config: &AppConfig) {
        let mut state = self.edit();
        state.needs_setup = config.environments.is_empty();
        state.health.configured = !config.environments.is_empty();
        state.pending = recovery::pending().map_or(true, |p| p.is_some());
        state.recovery = recovery::root()
            .ok()
            .map(|root| {
                root.join(if cfg!(windows) {
                    "Wukong-Recovery.exe"
                } else {
                    "Wukong-Recovery.AppImage"
                })
            })
            .filter(|path| {
                path.is_file()
                    && path.parent().is_some_and(|root| {
                        root.join("pending.json").is_file() || root.join("previous.json").is_file()
                    })
            });
    }
    pub async fn confirm(&self, service: Arc<Service>) {
        let _serial = self.operation.lock().await;
        let result = tokio::task::spawn_blocking(recovery::confirm).await;
        match result {
            Ok(Ok(true)) => {
                if let Err(error) = service.reload_after_maintenance() {
                    self.phase(
                        "error",
                        UiMessage::new("system-reload-failed").detail(format!("{error:#}")),
                    );
                } else {
                    self.phase(
                        "idle",
                        UiMessage::new(
                            "system-update-completed-client-and-tool-environment-checks-passed",
                        ),
                    );
                    self.edit().pending = false;
                }
            }
            Ok(Ok(false)) => {}
            Ok(Err(error)) => self.phase(
                "error",
                UiMessage::new("system-recovery-required").detail(format!("{error:#}")),
            ),
            Err(error) => self.phase(
                "error",
                UiMessage::new("error-operation").detail(error.to_string()),
            ),
        }
    }
    async fn remember(&self, version: Option<&str>) -> Result<Manifest> {
        let (manifest, bytes, signature) = self.cancellable(manifest::fetch(version)).await?;
        let directory = recovery::cache(&manifest.version)?;
        std::fs::create_dir_all(&directory)?;
        // Cache only authenticated metadata. Interrupted writes fail closed on reload.
        std::fs::write(directory.join("latest.json"), bytes)?;
        std::fs::write(directory.join("latest.json.sig"), signature)?;
        Ok(manifest)
    }
    async fn download(&self, manifest: &Manifest) -> Result<()> {
        let artifact = manifest.artifact()?;
        let path = recovery::payload(manifest)?;
        if path.exists() && manifest::verify_file(&path, artifact).is_ok() {
            return Ok(());
        }
        ensure!(
            fs2::available_space(path.parent().unwrap())? > artifact.size * 3 + 512 * 1024 * 1024,
            UiMessage::new("system-insufficient-space-on-the-client-disk")
        );
        self.phase(
            "downloading",
            UiMessage::new("system-downloading-client").arg("version", manifest.version.clone()),
        );
        let client = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(1800))
            .build()?;
        let mut response = self
            .cancellable(async { Ok(client.get(&artifact.url).send().await?) })
            .await?
            .error_for_status()?;
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        let mut downloaded = 0;
        use std::io::Write;
        while let Some(chunk) = self
            .cancellable(async { Ok(response.chunk().await?) })
            .await?
        {
            downloaded += chunk.len() as u64;
            ensure!(downloaded <= artifact.size, "download exceeds signed size");
            file.write_all(&chunk)?;
            let mut state = self.edit();
            state.downloaded = downloaded;
            state.total = Some(artifact.size);
        }
        file.as_file().sync_all()?;
        manifest::verify_file(file.path(), artifact)?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
    pub async fn action(
        self: &Arc<Self>,
        action: Action,
        epoch: u64,
        service: Arc<Service>,
        app: tauri::AppHandle,
    ) -> Result<Status> {
        if matches!(action, Action::Status) {
            return Ok(self.status());
        }
        let _serial = self.operation.try_lock().context(UiMessage::new(
            "system-an-update-operation-is-already-running",
        ))?;
        let _cache = crate::updates::storage::cache_lock(false)?;
        self.begin(Some(action));
        let result = self.perform(action, epoch, service, app).await;
        {
            let mut state = self.edit();
            state.busy = false;
            state.cancellable = false;
            state.cancelling = false;
        }
        if result
            .as_ref()
            .is_err_and(|e| e.is::<crate::workbench::cancellation::Cancelled>())
        {
            self.phase("cancelled", UiMessage::new("system-preparation-cancelled"));
            return Ok(self.status());
        }
        if let Err(error) = &result {
            let message = UiMessage::from_error_ref(error);
            self.edit().diagnosis = Some(super::update_diagnostics::classify(&message));
            self.phase("error", message);
        }
        result?;
        Ok(self.status())
    }
    async fn perform(
        self: &Arc<Self>,
        action: Action,
        epoch: u64,
        service: Arc<Service>,
        app: tauri::AppHandle,
    ) -> Result<()> {
        if matches!(action, Action::Check) {
            self.phase(
                "checking",
                UiMessage::new("system-checking-official-releases"),
            );
            let manifest = self.remember(None).await?;
            let available = manifest.can_upgrade(env!("CARGO_PKG_VERSION"))?;
            let mut state = self.edit();
            state.available = available.then_some(manifest.version);
            state.notes = manifest.notes;
            state.phase = "idle".into();
            state.message = if available {
                UiMessage::new("system-an-update-is-available")
            } else {
                UiMessage::new("system-already-up-to-date")
            };
            return Ok(());
        }
        if matches!(
            action,
            Action::StartTools | Action::StopTools | Action::SelectTools
        ) {
            self.phase("tools", UiMessage::new("system-managing-tool-environment"));
            let request = match action {
                Action::StartTools => crate::workbench::DesktopRequest::Up,
                Action::StopTools => crate::workbench::DesktopRequest::Down,
                _ => crate::workbench::DesktopRequest::Select { id: None },
            };
            let worker_service = service.clone();
            tokio::task::spawn_blocking(move || worker_service.workbench(epoch, request)).await??;
            let configured = self.status().health.configured;
            self.edit().health = super::update_health::Health {
                configured,
                ..Default::default()
            };
            self.phase("idle", UiMessage::new("system-tool-action-completed"));
            return Ok(());
        }
        let (config, config_path, _operation) = service.maintenance_context(epoch)?;
        if matches!(action, Action::Health | Action::Doctor) {
            self.phase("health", UiMessage::new("system-checking-tool-health"));
            let snapshot = tokio::task::spawn_blocking(move || {
                super::update_health::check(&config, matches!(action, Action::Doctor), epoch)
            })
            .await??;
            self.edit().diagnosis = snapshot
                .error
                .as_ref()
                .map(super::update_diagnostics::classify);
            self.edit().health = snapshot;
            self.phase("idle", UiMessage::new("system-health-check-completed"));
            return Ok(());
        }

        if matches!(action, Action::Integrate) {
            let path = recovery::integrate_linux()?;
            std::process::Command::new(path).spawn()?;
            app.exit(0);
            return Ok(());
        }
        if matches!(action, Action::CleanCache | Action::CleanImages) {
            let _lock = maintenance::lock(&recovery::root()?, true)?;
            ensure!(
                recovery::pending()?.is_none(),
                UiMessage::new("system-finish-or-recover-the-pending-update-first")
            );
            let previous_path = recovery::root()?.join("previous.json");
            let previous: Option<recovery::Journal> = if previous_path.exists() {
                Some(crate::workbench::read_json(&previous_path)?)
            } else {
                None
            };
            let prepared = self.status().prepared;
            let protected = [
                Some(env!("CARGO_PKG_VERSION").to_string()),
                prepared.clone(),
                previous.as_ref().map(|p| p.from.clone()),
            ];
            let protected_images: Vec<String> = protected
                .iter()
                .flatten()
                .map(|v| recovery::cached(v).map(|m| m.image))
                .collect::<Result<_>>()?;
            let docker = crate::workbench::Docker::new(&config.workbench.docker_executable);
            let directory = crate::updates::storage::cache_root()?.join("releases");
            if directory.exists() {
                for entry in std::fs::read_dir(&directory)? {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !entry.file_type()?.is_dir()
                        || semver::Version::parse(&name).is_err()
                        || name == env!("CARGO_PKG_VERSION")
                        || prepared.as_ref() == Some(&name)
                        || previous.as_ref().is_some_and(|p| p.from == name)
                    {
                        continue;
                    }
                    let path = std::fs::canonicalize(entry.path())?;
                    ensure!(
                        path.starts_with(std::fs::canonicalize(&directory)?),
                        "cache path escaped its directory"
                    );
                    let manifest = recovery::cached(&name)?;
                    if matches!(action, Action::CleanImages) {
                        if protected_images.contains(&manifest.image) {
                            continue;
                        }
                        if docker.inspect("image", &manifest.image)?.is_some()
                            && docker
                                .run(&[
                                    "ps",
                                    "-aq",
                                    "--filter",
                                    &format!("ancestor={}", manifest.image),
                                ])?
                                .is_empty()
                        {
                            docker.run(&["image", "rm", &manifest.image])?;
                        }
                    } else {
                        let payload = recovery::payload(&manifest)?;
                        if payload.exists() {
                            ensure!(
                                std::fs::canonicalize(&payload)?.parent() == Some(path.as_path()),
                                "payload escaped cache directory"
                            );
                            std::fs::remove_file(payload)?;
                        }
                    }
                }
            }
            self.phase(
                "idle",
                UiMessage::new(
                    "system-cleanup-completed-current-recovery-and-container-referenced-images",
                ),
            );
            return Ok(());
        }
        if matches!(action, Action::Recover) {
            let helper = recovery::root()?.join(if cfg!(windows) {
                "Wukong-Recovery.exe"
            } else {
                "Wukong-Recovery.AppImage"
            });
            ensure!(
                helper.is_file(),
                UiMessage::new("system-no-recovery-program-is-available-yet")
            );
            std::process::Command::new(helper).spawn()?;
            app.exit(0);
            return Ok(());
        }
        let workbench = Workbench::open(&config)?;
        if let Some(container) = config
            .environments
            .get("workbench")
            .and_then(|environment| environment.runtime.docker_container.as_ref())
        {
            ensure!(
                container == &workbench.name("tools"),
                UiMessage::new("system-switch-to-the-tool-environment-before-installing-or")
            );
        }
        if matches!(action, Action::Install) {
            let version = self.status().prepared.context(UiMessage::new(
                "system-download-and-verify-the-update-first",
            ))?;
            let manifest = recovery::cached(&version)?;
            ensure!(
                manifest.can_upgrade(env!("CARGO_PKG_VERSION"))?,
                UiMessage::new("system-this-is-not-a-newer-version")
            );
            let _lock = maintenance::lock(&recovery::root()?, true)?;
            ensure!(
                recovery::pending()?.is_none(),
                UiMessage::new("system-an-update-already-needs-recovery")
            );
            let _workbench_lock = workbench.lock()?;
            workbench.require_idle()?;
            let previous = workbench.environment()?;
            let previous_manifest = recovery::cached(env!("CARGO_PKG_VERSION"))?;
            ensure!(
                previous.image == previous_manifest.image,
                UiMessage::new("system-the-environment-is-not-a-verified-version-repair")
            );
            let journal = recovery::Journal {
                from: env!("CARGO_PKG_VERSION").into(),
                to: manifest.version.clone(),
                config: std::path::absolute(config_path)?,
                destination: recovery::executable()?,
                previous_image: previous_manifest.image,
            };
            let raw = serde_json::from_slice(&std::fs::read(
                recovery::cache(&version)?.join("latest.json"),
            )?)?;
            let update = app
                .updater_builder()
                .pubkey(manifest::PUBLIC_KEY.trim())
                .build()?
                .restore_update(raw)?
                .context(UiMessage::new("system-the-update-is-unavailable"))?;
            let bytes = std::fs::read(recovery::payload(&manifest)?)?;
            update.verify(&bytes)?;
            let helper = recovery::stage(&journal)?;
            {
                let mut state = self.edit();
                state.pending = true;
                state.recovery = Some(helper);
            }
            self.phase(
                "installing",
                UiMessage::new("system-installing-the-client-will-restart-shortly"),
            );
            update.install(bytes)?;
            #[cfg(not(windows))]
            app.restart();
            return Ok(());
        }
        ensure!(
            recovery::pending()?.is_none(),
            UiMessage::new("system-recover-or-confirm-the-pending-update-first")
        );
        let setup = matches!(action, Action::Setup);
        {
            let mut status = self.edit();
            status.environment_step = None;
            status.image_progress = None;
            status.image_version = None;
        }
        self.phase(
            "checking",
            if setup {
                UiMessage::new("system-reading-the-tool-environment-for-the-current-version")
            } else {
                UiMessage::new("system-preparing-update")
            },
        );
        let manifest = self
            .remember(if setup {
                Some(env!("CARGO_PKG_VERSION"))
            } else {
                None
            })
            .await?;
        if !setup {
            ensure!(
                manifest.can_upgrade(env!("CARGO_PKG_VERSION"))?,
                UiMessage::new("system-no-newer-version-is-available-to-install")
            );
            self.download(&manifest).await?;
            let previous = self.remember(Some(env!("CARGO_PKG_VERSION"))).await?;
            self.download(&previous).await?;
        }
        self.edit().image_version = Some(manifest.version.clone());
        let controller = self.clone();
        let image = manifest.image.clone();
        let probe_config = config.clone();
        let cancel = self.status().cancel;
        tokio::task::spawn_blocking(move || {
            environment::prepare(
                &probe_config,
                &image,
                cancel,
                |log| {
                    let mut state = controller.edit();
                    state.diagnostic_log = Some(log.to_owned());
                    state.log_available = true;
                },
                |message| {
                    let phase = match message.code.as_str() {
                        "system-checking-docker-host" => "docker",
                        "system-pulling-the-tool-image-docker-will-reuse-downloaded" => "pulling",
                        "system-checking-the-candidate-tool-environment" => "validating",
                        _ => "environment",
                    };
                    controller.phase(phase, message)
                },
                |value| {
                    controller.edit().image_progress = Some(value);
                },
            )
        })
        .await??;
        self.seal_preparation()?;
        if setup {
            self.phase(
                "activating",
                UiMessage::new("system-activating-tool-environment"),
            );
            let image = manifest.image;
            tokio::task::spawn_blocking(move || {
                let _lock = maintenance::lock(&recovery::root()?, true)?;
                environment::activate(&config_path, &image)
            })
            .await??;
            service.reload_after_maintenance()?;
            {
                let mut state = self.edit();
                state.needs_setup = false;
                // activate() has just checked the live container, tools, and saved binding.
                state.health = super::update_health::Health::verified_now(service.status().epoch);
            }
            self.phase(
                "idle",
                UiMessage::new("system-tool-environment-prepared-validation-passed"),
            );
        } else {
            self.edit().prepared = Some(manifest.version);
            self.phase(
                "ready",
                UiMessage::new("system-download-and-environment-checks-completed-ready-to-install"),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_is_bound_to_operation_and_sealed_before_activation() {
        let controller = Controller::default();
        controller.begin(Some(Action::Setup));
        let old = controller.status().operation_id.unwrap();
        controller.begin(Some(Action::Setup));
        assert!(controller.cancel(&old).is_err());
        let current = controller.status().operation_id.unwrap();
        controller.cancel(&current).unwrap();
        assert!(controller.seal_preparation().is_err());
        controller.begin(Some(Action::Setup));
        controller.seal_preparation().unwrap();
        assert!(
            controller
                .cancel(&controller.status().operation_id.unwrap())
                .is_err()
        );
        assert!(!controller.status().cancel.is_cancelled());
    }
    #[test]
    fn snapshots_are_read_only_and_new_operations_keep_monotonic_revisions() {
        let controller = Controller::default();
        controller.begin(Some(Action::Setup));
        let first = controller.status();
        assert_eq!(controller.status().revision, first.revision);
        controller.phase("pulling", UiMessage::default());
        assert!(controller.status().revision > first.revision);
        controller.begin(Some(Action::Check));
        let second = controller.status();
        assert_ne!(first.operation_id, second.operation_id);
        assert!(second.revision > first.revision);
    }
}
