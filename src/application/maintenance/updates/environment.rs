//! Prepare isolated candidates, then bind a verified environment to user settings.
use crate::i18n::Message as UiMessage;
use crate::{application::maintenance::workbench::Workbench, config::AppConfig};
use anyhow::{Context, Result, ensure};
use std::path::Path;

/// Require the Docker image named by a local-release package to exist on this
/// host and report its size.
///
/// `cargo package-local` embeds a local tag instead of a registry digest; pulling
/// it would target a repository that does not exist. The operator builds the tag
/// from this checkout, so a missing image is an actionable local setup error.
pub fn require_local_image(
    docker: &crate::application::maintenance::workbench::Docker,
    image: &str,
) -> Result<u64> {
    let inspection = docker.inspect("image", image)?.with_context(|| {
        format!(
            "the local-release Docker image `{image}` is not available on this Docker host; build it from this checkout with `docker buildx bake --load --set workbench.tags={image}` (or set WUKONG_LOCAL_IMAGE before `cargo package-local`) and retry"
        )
    })?;
    ensure!(
        inspection["Os"] == "linux" && inspection["Architecture"] == "amd64",
        "the local-release Docker image `{image}` must be a linux/amd64 image"
    );
    let size = inspection["Size"]
        .as_u64()
        .context("the local-release Docker image records no size")?;
    ensure!(
        size > 0,
        "the local-release Docker image records an empty size"
    );
    Ok(size)
}

/// `local_build` selects the local-release pipeline: the image must already
/// exist as a local tag and is never pulled from a registry.
pub fn prepare(
    config: &AppConfig,
    image: &str,
    local_build: bool,
    cancel: tokio_util::sync::CancellationToken,
    diagnostics: impl Fn(&Path),
    progress: impl Fn(UiMessage),
    pull: impl Fn(crate::application::maintenance::workbench::pull_progress::PullProgress),
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
    if local_build {
        workbench.docker.preflight_host()?;
        require_local_image(&workbench.docker, image)?;
    } else {
        workbench.docker.preflight(image)?;
    }
    let result = (|| {
        if local_build {
            // Re-inspect after the host check so a concurrently removed image
            // still produces the actionable local-build diagnosis.
            require_local_image(&workbench.docker, image)?;
            pull(
                crate::application::maintenance::workbench::pull_progress::PullProgress {
                    cached: true,
                    finished: true,
                    ..Default::default()
                },
            );
        } else {
            progress(UiMessage::new(
                "system-pulling-the-tool-image-docker-will-reuse-downloaded",
            ));
            if workbench.docker.inspect("image", image)?.is_none() {
                workbench
                    .docker
                    .pull_observed(image, &root.join("logs/pull.log"), &pull)?;
            } else {
                pull(
                    crate::application::maintenance::workbench::pull_progress::PullProgress {
                        cached: true,
                        finished: true,
                        ..Default::default()
                    },
                );
            }
        }
        if local_build {
            workbench.ensure_up(
                Some(image),
                crate::application::maintenance::workbench::environment::ImageAcquisition::LocalOnly,
            )?;
        } else {
            workbench.up(Some(image))?;
        }
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
            if error.is::<crate::application::maintenance::workbench::cancellation::Cancelled>() {
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
    use crate::interfaces::desktop::configuration::{ConfigDocument, ConfigEdit};
    use serde_json::json;
    let environment = workbench.environment()?;
    let mut document = ConfigDocument::open(path.to_owned())?;
    let fields = [
        (vec!["default_environment"], json!("workbench")),
        (
            vec!["environments", "workbench", "name"],
            json!("Linux workbench"),
        ),
        (
            vec!["environments", "workbench", "platform"],
            json!("linux"),
        ),
        (
            vec!["environments", "workbench", "runtime", "backend"],
            json!("docker"),
        ),
        (
            vec!["environments", "workbench", "runtime", "docker_container"],
            json!(environment.container),
        ),
        (
            vec!["environments", "workbench", "runtime", "docker_executable"],
            json!(workbench.config.workbench.docker_executable),
        ),
        (
            vec!["environments", "workbench", "runtime", "remote_state_dir"],
            json!("/state"),
        ),
        (
            vec!["environments", "workbench", "runtime", "remote_bridge_path"],
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
