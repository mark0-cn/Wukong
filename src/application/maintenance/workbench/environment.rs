use super::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct Environment {
    pub image: String,
    pub image_id: String,
    pub container: String,
    pub volume: String,
    pub network: String,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageAcquisition {
    AllowPull,
    LocalOnly,
}

fn validate_health(health: &Value) -> Result<()> {
    ensure!(
        health["healthy"] == true,
        "worker startup health probe failed: {health}"
    );
    let protocol = health["identity"]["protocol"]
        .as_u64()
        .context("worker health protocol missing")?;
    let expected = wukong_worker_protocol::PROTOCOL as u64;
    ensure!(
        protocol == expected,
        "worker protocol {protocol} is incompatible with controller protocol {expected}; select a matching tool image with env update --image IMAGE"
    );
    Ok(())
}

impl Workbench {
    pub fn environment(&self) -> Result<Environment> {
        read_json(&self.root.join("environment.json"))
    }
    pub fn status(&self) -> Result<Value> {
        let version: Value =
            serde_json::from_str(&self.docker.run(&["version", "--format", "{{json .}}"])?)?;
        let environment = self.environment().ok();
        let container = environment
            .as_ref()
            .map(|e| self.docker.inspect("container", &e.container))
            .transpose()?
            .flatten();
        Ok(
            json!({"workspace":self.root,"id":self.id,"docker":version,"environment":environment,"container":container}),
        )
    }
    pub fn ensure_network(&self, name: &str) -> Result<()> {
        if let Some(network) = self.docker.inspect("network", name)? {
            self.owned(&network, name)?;
        } else {
            self.docker.run(&[
                "network",
                "create",
                "--internal",
                "--label",
                &format!("wukong.workspace={}", self.id),
                name,
            ])?;
        }
        Ok(())
    }
    pub fn up(&self, image: Option<&str>) -> Result<Value> {
        self.ensure_up_with_volume(image, ImageAcquisition::AllowPull, None)
    }
    pub(crate) fn ensure_up(
        &self,
        image: Option<&str>,
        acquisition: ImageAcquisition,
    ) -> Result<Value> {
        self.ensure_up_with_volume(image, acquisition, None)
    }
    fn ensure_up_with_volume(
        &self,
        image: Option<&str>,
        acquisition: ImageAcquisition,
        volume: Option<String>,
    ) -> Result<Value> {
        let pending = self.root.join("environment.pending.json");
        let selected = image
            .map(str::to_owned)
            .or_else(|| self.environment().ok().map(|e| e.image_id))
            .or_else(|| read_json::<Environment>(&pending).ok().map(|e| e.image_id))
            .unwrap_or_else(|| self.config.workbench.image.clone());
        let image = selected.as_str();
        if self.docker.inspect("image", image)?.is_none() {
            ensure!(
                acquisition == ImageAcquisition::AllowPull,
                crate::i18n::Message::new("system-tool-image-needs-repair")
            );
            self.docker
                .logged(&["pull", image], &self.root.join("logs/pull.log"))?;
        }
        let inspection = self
            .docker
            .inspect("image", image)?
            .context("tool image missing")?;
        ensure!(
            inspection["Os"] == "linux" && inspection["Architecture"] == "amd64",
            "first release requires a linux/amd64 tool image"
        );
        let image_id = inspection["Id"]
            .as_str()
            .context("image ID missing")?
            .to_owned();
        let env = Environment {
            image: image.into(),
            image_id,
            container: self.name("tools"),
            volume: volume.unwrap_or_else(|| self.name("state")),
            network: self.name("offline"),
        };
        // Persist the selected immutable image before creating any resources.
        // Retry reconciles owned names/labels and reuses an interrupted start.
        write_json(&pending, &env)?;
        self.start_worker(&env)?;
        let health: Value = serde_json::from_str(&self.docker.run(&[
            "exec",
            &env.container,
            "python3",
            "/opt/wukong/runtime/wukong_worker/workbench/control.py",
            "health",
            "/state",
        ])?)?;
        validate_health(&health)?;
        write_json(&self.root.join("environment.json"), &env)?;
        self.write_worker_config(&env, &self.root.join("wukong.toml"))?;
        fs::remove_file(pending)?;
        Ok(
            json!({"environment":env,"health":health,"config":self.root.join("wukong.toml"),"transport":"host docker exec; no Docker socket mounted"}),
        )
    }
    pub fn start_worker(&self, env: &Environment) -> Result<()> {
        if let Some(old) = self.docker.inspect("container", &env.container)? {
            self.owned(&old, &env.container)?;
            ensure!(
                old["HostConfig"]["Init"] == true,
                "worker container lacks a child-process reaper; close active work and recreate it: env update for the main worker, or challenge down/up for a challenge worker"
            );
            ensure!(
                old["Image"] == env.image_id,
                "worker image changed; use env update after closing active work"
            );
            if old["State"]["Running"] != true {
                self.docker.run(&["start", &env.container])?;
            }
        } else {
            self.ensure_network(&env.network)?;
            if let Some(volume) = self.docker.inspect("volume", &env.volume)? {
                self.owned(&volume, &env.volume)?;
            } else {
                self.docker.run(&[
                    "volume",
                    "create",
                    "--label",
                    &format!("wukong.workspace={}", self.id),
                    &env.volume,
                ])?;
            }
            self.docker.run(&[
                "run",
                "--detach",
                "--init",
                "--name",
                &env.container,
                "--label",
                &format!("wukong.workspace={}", self.id),
                "--label",
                "wukong.kind=tools",
                "--network",
                &env.network,
                "--cap-drop",
                "ALL",
                "--cap-add",
                "CHOWN",
                "--cap-add",
                "SETUID",
                "--cap-add",
                "SETGID",
                "--cap-add",
                "KILL",
                "--cap-add",
                "SYS_PTRACE",
                "--cap-add",
                "DAC_OVERRIDE",
                "--security-opt",
                "no-new-privileges",
                "--security-opt",
                "seccomp=unconfined",
                "--read-only",
                "--pids-limit",
                "256",
                "--memory",
                &self.config.workbench.memory,
                "--cpus",
                &self.config.workbench.cpus.to_string(),
                "--tmpfs",
                "/tmp:rw,size=1g,exec",
                "--env",
                "HOME=/state/home",
                "--env",
                "XDG_CACHE_HOME=/state/cache",
                "--mount",
                &format!("type=volume,source={},target=/state", env.volume),
                &env.image_id,
            ])?;
        }
        self.docker
            .run(&["exec", &env.container, "chown", "0:0", "/state"])?;
        Ok(())
    }
    pub fn write_worker_config(&self, env: &Environment, path: &Path) -> Result<()> {
        let mut config = self.config.clone();
        config.resolve_paths(&std::env::current_dir()?);
        config.environments.insert(
            "workbench".into(),
            crate::adapters::environments::EnvironmentConfig {
                name: "Linux workbench".into(),
                platform: crate::adapters::environments::Platform::Linux,
                installation_id: None,
                runtime: crate::adapters::worker::RuntimeConfig {
                    transport: crate::adapters::worker::TransportConfig::Docker(
                        crate::adapters::worker::DockerConfig {
                            docker_executable: config.workbench.docker_executable.clone(),
                            remote_state_dir: "/state".into(),
                            remote_bridge_path: "/opt/wukong/runtime/bridge.py".into(),
                            ..crate::adapters::worker::DockerConfig::new(env.container.clone())
                        },
                    ),
                    ..Default::default()
                },
            },
        );
        config.default_environment = Some("workbench".into());
        config.workbench.root = self.root.clone();
        config.workbench.image = env.image_id.clone();
        fs::write(path, toml::to_string_pretty(&config)?)?;
        Ok(())
    }
    pub fn doctor(&self) -> Result<Value> {
        let result = self.worker_json(&[
            "/opt/wukong/runtime/wukong_worker/workbench/control.py",
            "doctor",
            "/state",
        ])?;
        write_json(&self.root.join("doctor.json"), &result)?;
        Ok(result)
    }
    pub fn worker_json(&self, args: &[&str]) -> Result<Value> {
        self.worker_json_for(&self.environment()?, args)
    }
    fn worker_json_for(&self, env: &Environment, args: &[&str]) -> Result<Value> {
        let mut command = vec!["exec", &env.container, "python3"];
        command.extend_from_slice(args);
        serde_json::from_str(&self.docker.run(&command)?).context("invalid worker observation")
    }
    pub fn require_idle(&self) -> Result<()> {
        self.require_idle_for(&self.environment()?)
    }
    fn require_idle_for(&self, env: &Environment) -> Result<()> {
        if let Some(container) = self.docker.inspect("container", &env.container)? {
            self.owned(&container, &env.container)?;
            if container["State"]["Running"] == true {
                let result = self.worker_json_for(
                    env,
                    &[
                        "/opt/wukong/runtime/wukong_worker/workbench/control.py",
                        "idle",
                        "/state",
                    ],
                )?;
                ensure!(
                    result["idle"] == true,
                    "worker has active jobs or sessions: {result}"
                );
            }
        }
        Ok(())
    }
    fn update_base(&self) -> Result<(Option<Environment>, Environment)> {
        let active = if self.root.join("environment.json").exists() {
            Some(self.environment()?)
        } else {
            None
        };
        let pending = if active.is_none() {
            Some(
                read_json(&self.root.join("environment.pending.json")).context(
                    "no active or pending environment; start the workbench with env up first",
                )?,
            )
        } else {
            None
        };
        let old = active.clone().or(pending).expect("pending checked above");
        Ok((active, old))
    }
    pub fn down(&self) -> Result<Value> {
        self.require_idle()?;
        let env = self.environment()?;
        if self.docker.inspect("container", &env.container)?.is_some() {
            self.docker.run(&["stop", &env.container])?;
        }
        Ok(json!({"stopped":env.container,"data_preserved":true}))
    }
    pub fn update(&self, image: &str) -> Result<Value> {
        let (active, old) = self.update_base()?;
        self.require_idle_for(&old)?;
        if self.docker.inspect("image", image)?.is_none() {
            self.docker
                .logged(&["pull", image], &self.root.join("logs/pull.log"))?;
        }
        if let Some(old) = &active {
            write_json(&self.root.join("previous-environment.json"), old)?;
        }
        if self.docker.inspect("container", &old.container)?.is_some() {
            self.docker.run(&["rm", "-f", "-v", &old.container])?;
        }
        let Some(old) = active else {
            let volume = format!("{}-{}", self.name("state"), uuid::Uuid::new_v4().simple());
            return self
                .ensure_up_with_volume(Some(image), ImageAcquisition::AllowPull, Some(volume))
                .context(
                    "upgrade from interrupted startup failed; no previous active environment existed to restore",
                );
        };
        match self.up(Some(image)) {
            Ok(result) => Ok(result),
            Err(error) => {
                if let Some(failed) = self.docker.inspect("container", &old.container)? {
                    self.owned(&failed, &old.container)?;
                    self.docker.run(&["rm", "-f", "-v", &old.container])?;
                }
                self.up(Some(&old.image_id))
                    .context("upgrade AND previous image restoration failed")?;
                Err(error.context("upgrade failed; attempted previous image restoration"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(image: &str) -> Environment {
        Environment {
            image: image.into(),
            image_id: format!("sha256:{}", "a".repeat(64)),
            container: "wukong-test-tools".into(),
            volume: "wukong-test-state".into(),
            network: "wukong-test-network".into(),
        }
    }

    #[test]
    fn protocol_mismatch_has_an_actionable_upgrade_error() {
        let error = validate_health(&json!({
            "healthy": true,
            "identity": {"protocol": wukong_worker_protocol::PROTOCOL - 1}
        }))
        .unwrap_err()
        .to_string();
        assert!(error.contains("worker protocol 3"), "{error}");
        assert!(error.contains("controller protocol 4"), "{error}");
        assert!(error.contains("env update --image IMAGE"), "{error}");
    }

    #[test]
    fn interrupted_first_start_is_a_valid_update_base() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let pending = environment("pending");
        write_json(&temp.path().join("environment.pending.json"), &pending)?;
        let workbench = Workbench {
            root: temp.path().to_owned(),
            id: "test".into(),
            docker: Docker::new("missing-docker"),
            config: AppConfig::default(),
        };
        let (active, old) = workbench.update_base()?;
        assert!(active.is_none());
        assert_eq!(old.image, "pending");
        write_json(
            &temp.path().join("environment.json"),
            &environment("active"),
        )?;
        let (active, old) = workbench.update_base()?;
        assert_eq!(active.expect("active environment").image, "active");
        assert_eq!(old.image, "active");
        Ok(())
    }
}
