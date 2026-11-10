//! Desktop update orchestration. Network preparation never switches the live worker.
use super::service::Service;
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
    pub version: String,
    pub phase: String,
    pub message: String,
    pub available: Option<String>,
    pub notes: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub prepared: Option<String>,
    pub recovery: Option<PathBuf>,
    pub needs_setup: bool,
    pub pending: bool,
}
pub struct Controller {
    status: Mutex<Status>,
    operation: tokio::sync::Mutex<()>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Status,
    Check,
    Prepare,
    Setup,
    Install,
    Recover,
    Integrate,
    CleanCache,
    CleanImages,
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
    fn phase(&self, phase: &str, message: &str) {
        let mut state = self.status.lock().unwrap();
        state.phase = phase.into();
        state.message = message.into();
    }
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
    pub fn initialization(&self, config: &AppConfig) {
        let mut state = self.status.lock().unwrap();
        state.needs_setup = !config.workbench.root.join("environment.json").is_file()
            || config.runtime.backend == crate::runtime::RuntimeBackend::Disabled;
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
                    self.phase("error", &format!("更新已安装，但配置重载失败：{error:#}"));
                } else {
                    self.phase("idle", "更新已完成，客户端与工具环境自检通过");
                    self.status.lock().unwrap().pending = false;
                }
            }
            Ok(Ok(false)) => {}
            Ok(Err(error)) => self.phase("error", &format!("更新需要恢复：{error:#}")),
            Err(error) => self.phase("error", &error.to_string()),
        }
    }
    async fn remember(&self, version: Option<&str>) -> Result<Manifest> {
        let (manifest, bytes, signature) = manifest::fetch(version).await?;
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
            "客户端所在磁盘空间不足"
        );
        self.phase(
            "downloading",
            &format!("正在下载客户端 {}", manifest.version),
        );
        let client = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(1800))
            .build()?;
        let mut response = client.get(&artifact.url).send().await?.error_for_status()?;
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        let mut downloaded = 0;
        use std::io::Write;
        while let Some(chunk) = response.chunk().await? {
            downloaded += chunk.len() as u64;
            ensure!(downloaded <= artifact.size, "download exceeds signed size");
            file.write_all(&chunk)?;
            let mut state = self.status.lock().unwrap();
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
        let _serial = self.operation.try_lock().context("更新操作正在进行")?;
        let result = self.perform(action, epoch, service, app).await;
        if let Err(error) = &result {
            self.phase("error", &format!("{error:#}"));
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
            self.phase("checking", "正在检查正式版本");
            let manifest = self.remember(None).await?;
            let available = manifest.can_upgrade(env!("CARGO_PKG_VERSION"))?;
            let mut state = self.status.lock().unwrap();
            state.available = available.then_some(manifest.version);
            state.notes = manifest.notes;
            state.phase = "idle".into();
            state.message = if available {
                "有可用更新"
            } else {
                "当前已是最新版本"
            }
            .into();
            return Ok(());
        }
        let (config, config_path, _operation) = service.maintenance_context(epoch)?;
        if matches!(action, Action::Integrate) {
            let path = recovery::integrate_linux()?;
            std::process::Command::new(path).spawn()?;
            app.exit(0);
            return Ok(());
        }
        if matches!(action, Action::CleanCache | Action::CleanImages) {
            let _lock = maintenance::lock(&recovery::root()?, true)?;
            ensure!(recovery::pending()?.is_none(), "先完成或恢复待处理更新");
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
            let docker = crate::workbench::Docker::new(&config.runtime.docker_executable);
            let directory = recovery::root()?.join("releases");
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
                "清理已完成，当前版本、恢复版本和容器引用的镜像均保留",
            );
            return Ok(());
        }
        if matches!(action, Action::Recover) {
            let helper = recovery::root()?.join(if cfg!(windows) {
                "Wukong-Recovery.exe"
            } else {
                "Wukong-Recovery.AppImage"
            });
            ensure!(helper.is_file(), "尚无恢复入口");
            std::process::Command::new(helper).spawn()?;
            app.exit(0);
            return Ok(());
        }
        let workbench = Workbench::open(&config)?;
        if let Some(container) = &config.runtime.docker_container {
            ensure!(
                container == &workbench.name("tools"),
                "请先切换到工具环境，再安装或更新"
            );
        }
        if matches!(action, Action::Install) {
            let version = self.status().prepared.context("请先下载并验证更新")?;
            let manifest = recovery::cached(&version)?;
            ensure!(
                manifest.can_upgrade(env!("CARGO_PKG_VERSION"))?,
                "不是更新版本"
            );
            let _lock = maintenance::lock(&recovery::root()?, true)?;
            ensure!(recovery::pending()?.is_none(), "已有待恢复更新");
            let _workbench_lock = workbench.lock()?;
            workbench.require_idle()?;
            let previous = workbench.environment()?;
            let previous_manifest = recovery::cached(env!("CARGO_PKG_VERSION"))?;
            ensure!(
                previous.image == previous_manifest.image,
                "当前环境不属于已验证版本，请先修复环境"
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
                .context("更新不可用")?;
            let bytes = std::fs::read(recovery::payload(&manifest)?)?;
            update.verify(&bytes)?;
            let helper = recovery::stage(&journal)?;
            {
                let mut state = self.status.lock().unwrap();
                state.pending = true;
                state.recovery = Some(helper);
            }
            self.phase("installing", "正在安装，客户端即将重启");
            update.install(bytes)?;
            #[cfg(not(windows))]
            app.restart();
            return Ok(());
        }
        ensure!(recovery::pending()?.is_none(), "先恢复或确认待处理更新");
        let setup = matches!(action, Action::Setup);
        self.phase(
            "checking",
            if setup {
                "正在读取当前版本的配套环境"
            } else {
                "正在准备更新"
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
                "没有可安装的新版本"
            );
            self.download(&manifest).await?;
            let previous = self.remember(Some(env!("CARGO_PKG_VERSION"))).await?;
            self.download(&previous).await?;
        }
        let controller = self.clone();
        let image = manifest.image.clone();
        let probe_config = config.clone();
        tokio::task::spawn_blocking(move || {
            environment::prepare(&probe_config, &image, |message| {
                controller.phase("environment", message)
            })
        })
        .await??;
        if setup {
            let image = manifest.image;
            tokio::task::spawn_blocking(move || {
                let _lock = maintenance::lock(&recovery::root()?, true)?;
                environment::activate(&config_path, &image)
            })
            .await??;
            service.reload_after_maintenance()?;
            self.status.lock().unwrap().needs_setup = false;
            self.phase("idle", "工具环境已准备完成，自检通过");
        } else {
            self.status.lock().unwrap().prepared = Some(manifest.version);
            self.phase("ready", "下载和环境自检已完成，可以安装并重启");
        }
        Ok(())
    }
}
