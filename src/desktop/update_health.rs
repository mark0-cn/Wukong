//! Observations of the configured tool environment, never inferred from a config file alone.
use crate::{config::AppConfig, i18n::Message, workbench::Workbench};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Default, Serialize)]
pub struct Health {
    pub configured: bool,
    pub platform: Option<crate::environments::Platform>,
    pub epoch: Option<u64>,
    pub docker: Option<bool>,
    pub running: Option<bool>,
    pub tools: Option<bool>,
    pub checked_at: Option<String>,
    pub tools_checked_at: Option<String>,
    pub image: Option<String>,
    pub checks: BTreeMap<String, bool>,
    pub error: Option<Message>,
}
impl Health {
    pub fn verified_now(epoch: u64) -> Self {
        let timestamp = chrono::Utc::now().to_rfc3339();
        Self {
            configured: true,
            platform: Some(crate::environments::Platform::Linux),
            epoch: Some(epoch),
            docker: Some(true),
            running: Some(true),
            tools: Some(true),
            checked_at: Some(timestamp.clone()),
            tools_checked_at: Some(timestamp),
            ..Default::default()
        }
    }
}
pub fn check(config: &AppConfig, full: bool, epoch: u64) -> Result<Health> {
    let configured = config
        .default_environment
        .as_ref()
        .and_then(|id| config.environments.get(id));
    let Some(environment) = configured else {
        return Ok(Health {
            epoch: Some(epoch),
            checked_at: Some(chrono::Utc::now().to_rfc3339()),
            ..Default::default()
        });
    };
    if config.default_environment.as_deref() != Some("workbench")
        || environment.platform != crate::environments::Platform::Linux
        || environment.runtime.backend() != crate::runtime::RuntimeBackend::Docker
    {
        let mut health = Health {
            configured: true,
            platform: Some(environment.platform),
            epoch: Some(epoch),
            checked_at: Some(chrono::Utc::now().to_rfc3339()),
            ..Default::default()
        };
        if full {
            let result = (|| -> Result<serde_json::Value> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                runtime.block_on(async {
                    let registry =
                        crate::environments::EnvironmentRegistry::new(config.environments.clone())?;
                    let result = registry
                        .doctor(config.default_environment.as_deref().unwrap())
                        .await;
                    registry.close().await;
                    result
                })
            })();
            match result {
                Ok(value) => {
                    health.running = Some(true);
                    health.tools = Some(
                        value["observation"]["capabilities"]["native_target_execution"] == true,
                    );
                    health.tools_checked_at = Some(chrono::Utc::now().to_rfc3339());
                    if health.tools != Some(true) {
                        health.error = Some(Message::new("system-health-tools-failed"));
                    }
                }
                Err(error) => {
                    health.running = Some(false);
                    health.error = Some(Message::from_error(error));
                }
            }
        }
        return Ok(health);
    }
    let workbench = Workbench::open(config)?;
    let _lock = workbench.lock()?;
    let environment = workbench.environment().ok();
    let mut health = Health {
        configured: environment.is_some(),
        platform: Some(crate::environments::Platform::Linux),
        epoch: Some(epoch),
        checked_at: Some(chrono::Utc::now().to_rfc3339()),
        image: environment.as_ref().map(|e| e.image.clone()),
        ..Default::default()
    };
    let result = (|| -> Result<()> {
        let info = workbench.docker.run(&["info", "--format", "{{.OSType}}"]);
        health.docker = Some(info.is_ok());
        ensure!(
            info? == "linux",
            Message::new("system-docker-linux-required")
        );
        if let Some(environment) = environment {
            let container = workbench
                .docker
                .inspect("container", &environment.container)?;
            health.running = Some(
                container
                    .as_ref()
                    .is_some_and(|c| c["State"]["Running"] == true),
            );
            if let Some(container) = container {
                workbench.owned(&container, &environment.container)?;
                ensure!(
                    container["Image"] == environment.image_id,
                    "tool container image differs from its environment record"
                );
            }
            if full {
                ensure!(
                    health.running == Some(true),
                    Message::new("system-tool-container-stopped")
                );
                let result = workbench.doctor()?;
                health.tools = Some(result["passed"] == true);
                health.tools_checked_at = Some(chrono::Utc::now().to_rfc3339());
                if let Some(checks) = result["checks"].as_object() {
                    for (name, result) in checks.iter().take(64) {
                        health
                            .checks
                            .insert(name.chars().take(80).collect(), result["passed"] == true);
                    }
                }
                ensure!(
                    health.tools == Some(true),
                    Message::new("system-health-tools-failed")
                );
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        health.error = Some(Message::from_error(error));
    }
    Ok(health)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_configuration_never_requires_docker_or_opens_a_workbench() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut config = AppConfig::default();
        config.workbench.root = root.path().join("must-not-be-created");
        config.workbench.docker_executable = "must-not-be-invoked".into();
        config.default_environment = Some("workbench".into());
        config.environments.insert(
            "workbench".into(),
            crate::environments::EnvironmentConfig {
                name: "Windows VM".into(),
                platform: crate::environments::Platform::Windows,
                installation_id: None,
                runtime: crate::runtime::RuntimeConfig {
                    transport: crate::runtime::TransportConfig::Ssh(crate::runtime::SshConfig {
                        ..crate::runtime::SshConfig::new("fixture.invalid".into())
                    }),
                    ..Default::default()
                },
            },
        );
        let health = check(&config, false, 1)?;
        assert!(health.configured);
        assert_eq!(health.docker, None);
        assert_eq!(health.tools, None);
        assert!(!config.workbench.root.exists());
        Ok(())
    }
    #[test]
    fn an_existing_record_does_not_claim_a_missing_daemon_is_healthy() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut config = AppConfig::default();
        config.workbench.root = directory.path().join("workbench");
        config.default_environment = Some("workbench".into());
        config.environments.insert(
            "workbench".into(),
            crate::environments::EnvironmentConfig {
                name: "Linux workbench".into(),
                platform: crate::environments::Platform::Linux,
                installation_id: None,
                runtime: crate::runtime::RuntimeConfig {
                    transport: crate::runtime::TransportConfig::Docker(
                        crate::runtime::DockerConfig {
                            ..crate::runtime::DockerConfig::new("fixture".into())
                        },
                    ),
                    ..Default::default()
                },
            },
        );
        config.workbench.docker_executable = directory
            .path()
            .join("missing-docker.exe")
            .to_string_lossy()
            .into_owned();
        let workbench = Workbench::open(&config)?;
        crate::workbench::write_json(
            &workbench.root.join("environment.json"),
            &serde_json::json!({
                "image": "fixture", "image_id": "sha256:fixture", "container": workbench.name("tools"),
                "network": workbench.name("offline"), "volume": workbench.name("state")
            }),
        )?;
        let health = check(&config, true, 1)?;
        assert!(health.configured);
        assert_eq!(health.docker, Some(false));
        assert_eq!(health.tools, None);
        assert!(health.error.is_some());
        assert!(health.checked_at.is_some());
        assert!(health.tools_checked_at.is_none());
        Ok(())
    }
    #[test]
    fn concurrent_workbench_ownership_is_reported_without_waiting() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut config = AppConfig::default();
        config.workbench.root = directory.path().join("workbench");
        let workbench = Workbench::open(&config)?;
        let lock = workbench.lock()?;
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            send.send(Workbench::open(&config).is_err()).unwrap();
        });
        let immediate = receive.recv_timeout(std::time::Duration::from_secs(1));
        drop(lock);
        worker.join().unwrap();
        assert_eq!(immediate, Ok(true));
        Ok(())
    }
}
