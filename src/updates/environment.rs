//! Prepare isolated candidates, then bind a verified environment to user settings.
use crate::{config::AppConfig, workbench::Workbench};
use anyhow::{Context, Result, ensure};
use std::path::Path;

pub fn prepare(config: &AppConfig, image: &str, progress: impl Fn(&str)) -> Result<()> {
    let root = crate::desktop::paths::home()?
        .join("updates/probes")
        .join(uuid::Uuid::new_v4().to_string());
    let mut probe = config.clone();
    probe.workbench.root = root.clone();
    let workbench = Workbench::open(&probe)?;
    let result = (|| {
        progress("正在拉取工具镜像；Docker 会复用已经下载的层");
        workbench.up(Some(image))?;
        progress("正在检查候选工具环境");
        ensure!(
            workbench.doctor()?["passed"] == true,
            "candidate environment self-check failed"
        );
        Ok(())
    })();
    // Only resources bearing this newly created probe's workspace identity.
    for (kind, name) in [
        ("container", workbench.name("tools")),
        ("volume", workbench.name("state")),
        ("network", workbench.name("offline")),
    ] {
        if let Some(value) = workbench.docker.inspect(kind, &name)? {
            workbench.owned(&value, &name)?;
            let args = if kind == "container" {
                vec![kind, "rm", "-f", &name]
            } else {
                vec![kind, "rm", &name]
            };
            workbench.docker.run(&args)?;
        }
    }
    result
}

pub fn bind(path: &Path, workbench: &Workbench) -> Result<()> {
    use crate::desktop::configuration::{ConfigDocument, ConfigEdit};
    use serde_json::json;
    let environment = workbench.environment()?;
    let mut document = ConfigDocument::open(path.to_owned())?;
    let fields = [
        (vec!["runtime", "backend"], json!("docker")),
        (
            vec!["runtime", "docker_container"],
            json!(environment.container),
        ),
        (vec!["runtime", "remote_state_dir"], json!("/state")),
        (
            vec!["runtime", "remote_bridge_path"],
            json!("/opt/wukong/runtime/bridge.py"),
        ),
        (vec!["workbench", "image"], json!(environment.image)),
        (vec!["workbench", "root"], json!(workbench.root)),
    ];
    document.patch(
        &fields
            .into_iter()
            .map(|(path, value)| ConfigEdit {
                path: path.into_iter().map(String::from).collect(),
                value,
            })
            .collect::<Vec<_>>(),
    )?;
    document.save(None)?;
    Ok(())
}

pub fn activate(config_path: &Path, image: &str) -> Result<()> {
    let config = AppConfig::load(Some(config_path))?;
    let workbench = Workbench::open(&config)?;
    let _lock = workbench.lock()?;
    let old = if workbench.root.join("environment.json").exists() {
        Some(workbench.environment()?)
    } else {
        None
    };
    let result = (|| {
        if old.as_ref().is_some_and(|old| old.image != image) {
            workbench.update(image)?;
        } else {
            workbench.up(Some(image))?;
        }
        ensure!(
            workbench.doctor()?["passed"] == true,
            "environment self-check failed"
        );
        bind(config_path, &workbench)
    })();
    if result.is_err()
        && let Some(old) = old
    {
        workbench
            .update(&old.image_id)
            .context("failed to restore previous environment")?;
        bind(config_path, &workbench)?;
    }
    result
}
