//! Prepare isolated candidates, then bind a verified environment to user settings.
use crate::i18n::Message as UiMessage;
use crate::{config::AppConfig, workbench::Workbench};
use anyhow::{Context, Result, ensure};
use std::path::Path;

pub fn prepare(
    config: &AppConfig,
    image: &str,
    cancel: tokio_util::sync::CancellationToken,
    diagnostics: impl Fn(&Path),
    progress: impl Fn(UiMessage),
    pull: impl Fn(crate::workbench::pull_progress::PullProgress),
) -> Result<()> {
    let root = super::storage::cache_root()?
        .join("probes")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(root.join("logs"))?;
    diagnostics(&root.join("logs/pull.log"));
    let mut probe = config.clone();
    probe.workbench.root = root.clone();
    let mut workbench = Workbench::open(&probe)?;
    workbench.docker.cancellation = Some(cancel.clone());
    progress(UiMessage::new("system-checking-docker-host"));
    workbench.docker.preflight(image)?;
    let result = (|| {
        progress(UiMessage::new(
            "system-pulling-the-tool-image-docker-will-reuse-downloaded",
        ));
        if workbench.docker.inspect("image", image)?.is_none() {
            workbench
                .docker
                .pull_observed(image, &root.join("logs/pull.log"), &pull)?;
        } else {
            pull(crate::workbench::pull_progress::PullProgress {
                cached: true,
                finished: true,
                ..Default::default()
            });
        }
        workbench.up(Some(image))?;
        progress(UiMessage::new(
            "system-checking-the-candidate-tool-environment",
        ));
        ensure!(
            workbench.doctor()?["passed"] == true,
            "candidate environment self-check failed"
        );
        Ok(())
    })();
    workbench.docker.cancellation = None;
    // Only resources bearing this newly created probe's workspace identity.
    let cleanup = (|| -> Result<()> {
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
        Ok(())
    })();
    // A cleanup failure must not replace the original pull/doctor diagnosis.
    match result {
        Err(error) => {
            if error.is::<crate::workbench::cancellation::Cancelled>() {
                cleanup
                    .context("cancelled preparation could not clean up its candidate resources")?;
            } else if let Err(cleanup_error) = cleanup {
                tracing::warn!(error = %cleanup_error, "candidate cleanup failed; preserving original diagnosis");
            }
            Err(error)
        }
        Ok(()) => cleanup,
    }
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
        ensure!(
            workbench.doctor()?["passed"] == true,
            "restored environment failed self-check"
        );
        bind(config_path, &workbench)?;
    }
    result
}
