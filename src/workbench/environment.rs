use super::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct Environment {
    pub image: String,
    pub image_id: String,
    pub container: String,
    pub volume: String,
    pub network: String,
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
        let selected = image
            .map(str::to_owned)
            .or_else(|| self.environment().ok().map(|e| e.image_id))
            .unwrap_or_else(|| self.config.workbench.image.clone());
        let image = selected.as_str();
        if self.docker.inspect("image", image)?.is_none() {
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
            volume: self.name("state"),
            network: self.name("offline"),
        };
        self.start_worker(&env)?;
        write_json(&self.root.join("environment.json"), &env)?;
        self.write_worker_config(&env, &self.root.join("wukong.toml"))?;
        Ok(
            json!({"environment":env,"config":self.root.join("wukong.toml"),"transport":"host docker exec; no Docker socket mounted"}),
        )
    }
    pub fn start_worker(&self, env: &Environment) -> Result<()> {
        if let Some(old) = self.docker.inspect("container", &env.container)? {
            self.owned(&old, &env.container)?;
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
        config.runtime.backend = crate::runtime::RuntimeBackend::Docker;
        config.runtime.docker_container = Some(env.container.clone());
        config.runtime.remote_state_dir = "/state".into();
        config.runtime.remote_bridge_path = "/opt/wukong/runtime/bridge.py".into();
        config.workbench.root = self.root.clone();
        config.workbench.image = env.image_id.clone();
        fs::write(path, toml::to_string_pretty(&config)?)?;
        Ok(())
    }
    pub fn doctor(&self) -> Result<Value> {
        let result = self.worker_json(&[
            "/opt/wukong/runtime/workbench_control.py",
            "doctor",
            "/state",
        ])?;
        write_json(&self.root.join("doctor.json"), &result)?;
        Ok(result)
    }
    pub fn worker_json(&self, args: &[&str]) -> Result<Value> {
        let env = self.environment()?;
        let mut command = vec!["exec", &env.container, "python3"];
        command.extend_from_slice(args);
        serde_json::from_str(&self.docker.run(&command)?).context("invalid worker observation")
    }
    pub fn require_idle(&self) -> Result<()> {
        let env = self.environment()?;
        if let Some(container) = self.docker.inspect("container", &env.container)? {
            self.owned(&container, &env.container)?;
            if container["State"]["Running"] == true {
                let result = self.worker_json(&[
                    "/opt/wukong/runtime/workbench_control.py",
                    "idle",
                    "/state",
                ])?;
                ensure!(
                    result["idle"] == true,
                    "worker has active jobs or sessions: {result}"
                );
            }
        }
        Ok(())
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
        self.require_idle()?;
        if self.docker.inspect("image", image)?.is_none() {
            self.docker
                .logged(&["pull", image], &self.root.join("logs/pull.log"))?;
        }
        let old = self.environment()?;
        write_json(&self.root.join("previous-environment.json"), &old)?;
        if self.docker.inspect("container", &old.container)?.is_some() {
            self.docker.run(&["rm", "-f", "-v", &old.container])?;
        }
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
