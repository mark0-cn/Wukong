//! Prepare isolated candidates, then bind a verified environment to user settings.
#[cfg(feature = "local-release")]
use super::local_release::{self, LocalRelease};
#[cfg(feature = "local-release")]
use crate::application::maintenance::workbench::{Docker, pull_progress::PullProgress};
use crate::i18n::Message as UiMessage;
use crate::{application::maintenance::workbench::Workbench, config::AppConfig};
use anyhow::{Context, Result, ensure};
use std::path::Path;
#[cfg(feature = "local-release")]
use std::path::PathBuf;

/// Require the Docker image named by a local-release package to exist on this
/// host and report its size.
///
/// `cargo package-local` embeds a local tag instead of a registry digest. The
/// preparation pipeline runs after onboarding has acquired that tag (from a
/// local build or by pulling the recorded CI image), so a missing image here is
/// an actionable local setup error.
pub fn require_local_image(
    docker: &crate::application::maintenance::workbench::Docker,
    image: &str,
) -> Result<u64> {
    let inspection = docker.inspect("image", image)?.with_context(|| {
        format!(
            "the local-release Docker image `{image}` is not available on this Docker host; build it from this checkout with `docker buildx bake --load --set workbench.tags={image}` (or set WUKONG_LOCAL_IMAGE before `cargo package-local`) and retry"
        )
    })?;
    image_identity(&inspection, image)
}

/// Validate the platform and size Docker reports for one local-release image.
fn image_identity(inspection: &serde_json::Value, image: &str) -> Result<u64> {
    ensure!(
        inspection["Os"] == "linux" && inspection["Architecture"] == "amd64",
        "the local-release Docker image `{image}` must be a linux/amd64 image"
    );
    let size = inspection["Size"]
        .as_u64()
        .with_context(|| format!("the local-release Docker image `{image}` records no size"))?;
    ensure!(
        size > 0,
        "the local-release Docker image `{image}` records an empty size"
    );
    Ok(size)
}

/// How first-run onboarding obtains the Docker image of a local-release package.
#[cfg(feature = "local-release")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LocalImagePlan {
    /// The descriptor image already exists on this Docker host.
    Present,
    /// Pull the recorded CI-built image, then tag it as the descriptor image.
    Pull { reference: String },
}

/// Decide whether onboarding may reuse a local image or must pull the recorded
/// CI-built workbench image.
///
/// The answer depends only on the inspection result and the descriptor fields,
/// so the priority is testable without a Docker daemon. A package without a
/// recorded CI image falls back to the actionable local-build instructions; the
/// official signed release image repository is never pulled.
#[cfg(feature = "local-release")]
pub(crate) fn plan_local_image(
    image: &str,
    image_registry: Option<&str>,
    version: &str,
    present: bool,
) -> Result<LocalImagePlan> {
    if present {
        return Ok(LocalImagePlan::Present);
    }
    match image_registry
        .map(str::trim)
        .filter(|reference| !reference.is_empty())
    {
        Some(reference) => {
            let suggested = format!("{}:local-{version}", local_release::DEFAULT_REGISTRY);
            ensure!(
                !local_release::official_release_image(reference),
                "local-release.json records the official release image repository `{reference}`; local onboarding only pulls the separate CI workbench image `{suggested}`"
            );
            Ok(LocalImagePlan::Pull {
                reference: reference.to_owned(),
            })
        }
        None => Err(anyhow::anyhow!(
            "the local-release Docker image `{image}` is not available on this Docker host and this package records no CI-built workbench image to pull; {}",
            local_image_help(image, None, version)
        )),
    }
}

/// Actionable acquisition instructions used when onboarding must obtain the image.
///
/// Both the registry pull/tag commands and the local Bake command are listed so a
/// missing image, an unreachable registry or a missing login can be recovered
/// without rebuilding the installer.
#[cfg(feature = "local-release")]
fn local_image_help(image: &str, reference: Option<&str>, version: &str) -> String {
    let reference = reference.map_or_else(
        || format!("{}:local-{version}", local_release::DEFAULT_REGISTRY),
        ToOwned::to_owned,
    );
    format!(
        "pull it with `docker pull --platform linux/amd64 {reference}`, tag it with `docker tag {reference} {image}` (run `docker login` first when the registry repository is private), or build it from this checkout with `docker buildx bake --load --set workbench.tags={image}`, then retry"
    )
}

/// Docker operations onboarding needs while acquiring the local-release image.
///
/// A small trait keeps the resolution testable: production uses [`Docker`], tests
/// use an in-memory host and never start Docker.
#[cfg(feature = "local-release")]
trait LocalImageHost {
    fn inspect_image(&self, image: &str) -> Result<Option<serde_json::Value>>;
    fn pull_image(&self, reference: &str, report: &mut dyn FnMut(PullProgress)) -> Result<()>;
    fn tag_image(&self, reference: &str, image: &str) -> Result<()>;
}

#[cfg(feature = "local-release")]
struct DockerImageHost<'a> {
    docker: &'a Docker,
    log: PathBuf,
}

#[cfg(feature = "local-release")]
impl LocalImageHost for DockerImageHost<'_> {
    fn inspect_image(&self, image: &str) -> Result<Option<serde_json::Value>> {
        self.docker.inspect("image", image)
    }

    fn pull_image(&self, reference: &str, report: &mut dyn FnMut(PullProgress)) -> Result<()> {
        self.docker
            .pull_observed_platform(reference, "linux/amd64", &self.log, |progress| {
                report(progress)
            })
    }

    fn tag_image(&self, reference: &str, image: &str) -> Result<()> {
        self.docker.run(&["tag", reference, image])?;
        Ok(())
    }
}

/// Resolve the local-release image, pulling and tagging the recorded CI image on
/// first use so a packaged installer works on another machine.
#[cfg(feature = "local-release")]
pub(crate) fn require_or_pull_local_image_observed(
    docker: &Docker,
    descriptor: &LocalRelease,
    mut report: impl FnMut(PullProgress),
) -> Result<u64> {
    let host = DockerImageHost {
        docker,
        log: pull_log()?,
    };
    resolve_local_image(&host, descriptor, &mut report)
}

/// Require or pull the local-release image without reporting layer progress.
///
/// Onboarding uses the observed variant to drive the download indicator; this
/// entry point keeps the same resolution contract for callers that only log.
#[cfg(feature = "local-release")]
pub fn require_or_pull_local_image(docker: &Docker, descriptor: &LocalRelease) -> Result<u64> {
    require_or_pull_local_image_observed(docker, descriptor, |_| {})
}

#[cfg(feature = "local-release")]
fn pull_log() -> Result<PathBuf> {
    let directory = super::storage::cache_root()?.join("probes");
    std::fs::create_dir_all(&directory)?;
    Ok(directory.join("local-release-image.log"))
}

/// Inspect, pull and tag the image so later preparation stages keep using the
/// descriptor's local `image` tag.
#[cfg(feature = "local-release")]
fn resolve_local_image(
    host: &impl LocalImageHost,
    descriptor: &LocalRelease,
    report: &mut dyn FnMut(PullProgress),
) -> Result<u64> {
    let local = host.inspect_image(&descriptor.image)?;
    match plan_local_image(
        &descriptor.image,
        descriptor.image_registry.as_deref(),
        &descriptor.version,
        local.is_some(),
    )? {
        LocalImagePlan::Present => image_identity(
            local
                .as_ref()
                .context("the local-release image inspection was lost between checks")?,
            &descriptor.image,
        ),
        LocalImagePlan::Pull { reference } => {
            let help = local_image_help(&descriptor.image, Some(&reference), &descriptor.version);
            host.pull_image(&reference, report).with_context(|| {
                format!("cannot pull the CI-built workbench image `{reference}`; {help}")
            })?;
            let pulled = host.inspect_image(&reference)?.with_context(|| {
                format!("the pulled workbench image `{reference}` is unavailable; {help}")
            })?;
            let size = image_identity(&pulled, &reference)?;
            host.tag_image(&reference, &descriptor.image)
                .with_context(|| {
                    format!("cannot tag `{reference}` as `{}`; {help}", descriptor.image)
                })?;
            ensure!(
                host.inspect_image(&descriptor.image)?.is_some(),
                "tagging `{reference}` as `{}` produced no local image; {help}",
                descriptor.image
            );
            Ok(size)
        }
    }
}

/// `local_build` selects the local-release pipeline: onboarding has already
/// acquired the descriptor image tag (built locally or pulled from the CI
/// registry); preparation itself never pulls it from a registry.
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

#[cfg(all(test, feature = "local-release"))]
mod local_image_tests {
    use super::*;
    use serde_json::{Value, json};
    use std::{cell::RefCell, collections::BTreeMap};

    /// In-memory stand-in for Docker: no daemon, no child processes, and every
    /// operation records the argv-equivalent call it received.
    #[derive(Default)]
    struct FakeHost {
        images: RefCell<BTreeMap<String, Value>>,
        calls: RefCell<Vec<String>>,
        reports: RefCell<usize>,
        pull: Option<std::result::Result<Value, &'static str>>,
    }

    impl LocalImageHost for FakeHost {
        fn inspect_image(&self, image: &str) -> Result<Option<Value>> {
            self.calls.borrow_mut().push(format!("inspect {image}"));
            Ok(self.images.borrow().get(image).cloned())
        }

        fn pull_image(&self, reference: &str, report: &mut dyn FnMut(PullProgress)) -> Result<()> {
            self.calls.borrow_mut().push(format!("pull {reference}"));
            report(PullProgress {
                finished: true,
                ..Default::default()
            });
            *self.reports.borrow_mut() += 1;
            match &self.pull {
                Some(Ok(value)) => {
                    self.images
                        .borrow_mut()
                        .insert(reference.to_owned(), value.clone());
                    Ok(())
                }
                Some(Err(message)) => anyhow::bail!("{message}"),
                None => anyhow::bail!("no fake pull result configured"),
            }
        }

        fn tag_image(&self, reference: &str, image: &str) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("tag {reference} {image}"));
            let value = self.images.borrow().get(reference).cloned();
            match value {
                Some(value) => {
                    self.images.borrow_mut().insert(image.to_owned(), value);
                    Ok(())
                }
                None => anyhow::bail!("cannot tag a missing image"),
            }
        }
    }

    fn image(size: u64) -> Value {
        json!({"Os": "linux", "Architecture": "amd64", "Size": size})
    }

    fn descriptor(registry: Option<&str>, image_size: u64) -> LocalRelease {
        LocalRelease {
            schema: local_release::SCHEMA,
            version: env!("CARGO_PKG_VERSION").to_string(),
            runtime_protocol: crate::adapters::worker::protocol::PROTOCOL,
            data_schema: crate::application::maintenance::updates::manifest::DATA_SCHEMA,
            image: local_release::DEFAULT_IMAGE.to_string(),
            image_size,
            image_registry: registry.map(ToOwned::to_owned),
            template_path: None,
            built_at: "2026-11-18T06:00:00Z".into(),
            source_revision: "0123456789abcdef0123456789abcdef01234567".into(),
        }
    }

    const REFERENCE: &str = "hlmark0/wukong-workbench:local-1.0.0";

    fn resolved(host: &FakeHost, registry: Option<&str>) -> Result<u64> {
        let mut report = |_: PullProgress| {};
        resolve_local_image(host, &descriptor(registry, 0), &mut report)
    }

    #[test]
    fn a_present_local_image_is_used_without_pulling() {
        let host = FakeHost {
            images: RefCell::new(BTreeMap::from([("wukong:local".into(), image(2048))])),
            ..Default::default()
        };
        assert_eq!(resolved(&host, Some(REFERENCE)).unwrap(), 2048);
        assert_eq!(*host.calls.borrow(), vec!["inspect wukong:local"]);
        assert_eq!(*host.reports.borrow(), 0);
    }

    #[test]
    fn a_missing_local_image_pulls_verifies_and_tags_the_ci_image() {
        let host = FakeHost {
            pull: Some(Ok(image(4096))),
            ..Default::default()
        };
        assert_eq!(resolved(&host, Some(REFERENCE)).unwrap(), 4096);
        assert_eq!(
            *host.calls.borrow(),
            vec![
                "inspect wukong:local".to_string(),
                format!("pull {REFERENCE}"),
                format!("inspect {REFERENCE}"),
                format!("tag {REFERENCE} wukong:local"),
                "inspect wukong:local".to_string(),
            ]
        );
        assert_eq!(*host.reports.borrow(), 1);
    }

    #[test]
    fn a_pull_failure_reports_both_the_registry_and_local_build_commands() {
        let host = FakeHost {
            pull: Some(Err("manifest unknown")),
            ..Default::default()
        };
        let error = resolved(&host, Some(REFERENCE)).unwrap_err();
        let text = format!("{error:#}");
        let pull = format!("docker pull --platform linux/amd64 {REFERENCE}");
        let tag = format!("docker tag {REFERENCE} wukong:local");
        for expected in [
            "cannot pull the CI-built workbench image",
            "manifest unknown",
            pull.as_str(),
            tag.as_str(),
            "docker buildx bake --load --set workbench.tags=wukong:local",
            "docker login",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        assert!(
            !host
                .calls
                .borrow()
                .iter()
                .any(|call| call.starts_with("tag "))
        );
    }

    #[test]
    fn a_missing_image_without_a_registry_reports_the_suggested_reference() {
        let host = FakeHost::default();
        let error = resolved(&host, None).unwrap_err();
        let text = format!("{error:#}");
        for expected in [
            "records no CI-built workbench image",
            "docker pull --platform linux/amd64 hlmark0/wukong-workbench:local-",
            "docker tag",
            "docker buildx bake --load --set workbench.tags=wukong:local",
            "docker login",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        assert_eq!(*host.calls.borrow(), vec!["inspect wukong:local"]);
    }

    #[test]
    fn a_pulled_image_must_be_a_sized_linux_amd64_image() {
        for invalid in [
            json!({"Os": "windows", "Architecture": "amd64", "Size": 4096}),
            json!({"Os": "linux", "Architecture": "arm64", "Size": 4096}),
            json!({"Os": "linux", "Architecture": "amd64", "Size": 0}),
        ] {
            let host = FakeHost {
                pull: Some(Ok(invalid.clone())),
                ..Default::default()
            };
            let error = resolved(&host, Some(REFERENCE)).unwrap_err();
            assert!(
                format!("{error:#}").contains("linux/amd64")
                    || format!("{error:#}").contains("empty size"),
                "unexpected error for {invalid}: {error:#}"
            );
            assert!(
                !host
                    .calls
                    .borrow()
                    .iter()
                    .any(|call| call.starts_with("tag ")),
                "an invalid pulled image must not be tagged"
            );
        }
    }

    #[test]
    fn the_official_release_repository_is_never_pulled() {
        let host = FakeHost::default();
        let error = resolved(&host, Some("hlmark0/wukong:local-1.0.0")).unwrap_err();
        assert!(format!("{error:#}").contains("official release image repository"));
        assert_eq!(*host.calls.borrow(), vec!["inspect wukong:local"]);
    }

    #[test]
    fn the_plan_prefers_the_local_image_over_the_recorded_registry() {
        assert_eq!(
            plan_local_image("wukong:local", Some(REFERENCE), "1.0.0", true).unwrap(),
            LocalImagePlan::Present
        );
        assert_eq!(
            plan_local_image("wukong:local", None, "1.0.0", true).unwrap(),
            LocalImagePlan::Present
        );
        assert_eq!(
            plan_local_image("wukong:local", Some(REFERENCE), "1.0.0", false).unwrap(),
            LocalImagePlan::Pull {
                reference: REFERENCE.to_owned()
            }
        );
        assert!(plan_local_image("wukong:local", None, "1.0.0", false).is_err());
    }
}
