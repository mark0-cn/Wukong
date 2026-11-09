//! Per-challenge sibling containers and explicit network modes.
use super::challenge::{Challenge, relative_path};
use super::environment::Environment;
use super::*;

impl Workbench {
    fn challenge_environment(&self, id: &str) -> Result<Environment> {
        let base = self.environment()?;
        Ok(Environment {
            image: base.image,
            image_id: base.image_id,
            container: self.name(&format!("{id}-tools")),
            volume: self.name(&format!("{id}-state")),
            network: self.name(&format!("{id}-net")),
        })
    }
    fn challenge_idle(&self, id: &str) -> Result<()> {
        let env = self.challenge_environment(id)?;
        if let Some(container) = self.docker.inspect("container", &env.container)? {
            self.owned(&container, &env.container)?;
            if container["State"]["Running"] == true {
                let value: Value = serde_json::from_str(&self.docker.run(&[
                    "exec",
                    &env.container,
                    "python3",
                    "/opt/re-agent/runtime/workbench_control.py",
                    "idle",
                    "/state",
                ])?)?;
                ensure!(
                    value["idle"] == true,
                    "challenge has active jobs/sessions: {value}"
                );
            }
        }
        Ok(())
    }
    pub fn prepare_challenge(&self, id: &str) -> Result<Value> {
        self.verify_original(id)?;
        let challenge = self.challenge(id)?;
        self.challenge_idle(id)?;
        let root = self.challenge_dir(id)?;
        let env = self.challenge_environment(id)?;
        self.start_worker(&env)?;
        self.docker.run(&[
            "exec",
            &env.container,
            "mkdir",
            "-p",
            "/state/files/challenge",
        ])?;
        // Only challenge attachments and prepared files cross the container boundary.
        for directory in ["original", "prepared", "scripts"] {
            self.docker.copy_to(
                &root.join(directory),
                &env.container,
                "/state/files/challenge/",
            )?;
        }
        let request = root.join("launch-request.json");
        write_json(&request, &challenge)?;
        self.docker.copy_to(
            &request,
            &env.container,
            "/state/files/challenge/launch-request.json",
        )?;
        let observation: Value = serde_json::from_str(&self.docker.run(&[
            "exec",
            &env.container,
            "python3",
            "/opt/re-agent/runtime/workbench_prepare.py",
            "/state/files/challenge",
        ])?)?;
        self.docker.copy_from(
            &env.container,
            "/state/files/challenge/launch.json",
            &root.join("launch.json"),
        )?;
        self.docker.copy_from(
            &env.container,
            "/state/files/challenge/prepared/.",
            &root.join("prepared"),
        )?;
        let mut lock: Value = read_json(&root.join("environment.lock.json"))?;
        lock["tool_image_id"] = json!(env.image_id);
        lock["manifest_sha256"] = json!(crate::binary::sha256_file(root.join("challenge.toml"))?);
        lock["launch"] = observation;
        lock["status"] = json!("prepared");
        write_json(&root.join("environment.lock.json"), &lock)?;
        self.write_worker_config(&env, &root.join("re-agent.toml"))?;
        Ok(
            json!({"id":id,"environment":env,"launch":lock["launch"],"config":root.join("re-agent.toml")}),
        )
    }
    pub fn build_challenge(&self, id: &str) -> Result<Value> {
        self.verify_original(id)?;
        self.challenge_idle(id)?;
        let challenge = self.challenge(id)?;
        let root = self.challenge_dir(id)?;
        let image = self.name(&format!("challenge-{id}:local"));
        let reference = if let Some(dockerfile) = &challenge.runtime.dockerfile {
            let file = relative_path(&root, dockerfile)?;
            ensure!(
                file.starts_with(root.join("original")),
                "build context must be original attachments"
            );
            let mut args = vec![
                "build".to_owned(),
                "--platform".into(),
                "linux/amd64".into(),
                "--label".into(),
                format!("re-agent.workspace={}", self.id),
                "-t".into(),
                image.clone(),
                "-f".into(),
                portable_path(&file).to_string_lossy().into_owned(),
            ];
            for (key, value) in &challenge.runtime.build_args {
                ensure!(
                    !key.contains('=') && !key.starts_with('-') && !key.contains('\0'),
                    "invalid build argument"
                );
                args.extend(["--build-arg".into(), format!("{key}={value}")]);
            }
            args.push(
                portable_path(&root.join("original"))
                    .to_string_lossy()
                    .into_owned(),
            );
            self.docker.logged(
                &args.iter().map(String::as_str).collect::<Vec<_>>(),
                &root.join("results/build.log"),
            )?;
            image
        } else {
            let image = challenge
                .runtime
                .image
                .context("runtime needs dockerfile or image")?;
            if self.docker.inspect("image", &image)?.is_none() {
                self.docker
                    .logged(&["pull", &image], &root.join("results/pull.log"))?;
            }
            image
        };
        let inspect = self
            .docker
            .inspect("image", &reference)?
            .context("challenge image missing")?;
        ensure!(
            inspect["Os"] == "linux" && inspect["Architecture"] == "amd64",
            "challenge image must be linux/amd64 in this release"
        );
        let mut lock: Value = read_json(&root.join("environment.lock.json"))?;
        lock["challenge_image_id"] = inspect["Id"].clone();
        lock["build_manifest_sha256"] =
            json!(crate::binary::sha256_file(root.join("challenge.toml"))?);
        write_json(&root.join("environment.lock.json"), &lock)?;
        Ok(json!({"id":id,"image_id":inspect["Id"],"reference":reference}))
    }
    fn checked_image(&self, id: &str) -> Result<String> {
        let root = self.challenge_dir(id)?;
        let lock: Value = read_json(&root.join("environment.lock.json"))?;
        ensure!(
            lock["build_manifest_sha256"]
                == crate::binary::sha256_file(root.join("challenge.toml"))?,
            "manifest changed since build; run challenge build again"
        );
        Ok(lock["challenge_image_id"]
            .as_str()
            .context("build the challenge first")?
            .to_owned())
    }
    pub fn up_challenge(&self, id: &str, allow_privileged: bool) -> Result<Value> {
        self.verify_original(id)?;
        let challenge = self.challenge(id)?;
        self.challenge_idle(id)?;
        let env = self.challenge_environment(id)?;
        self.start_worker(&env)?;
        let service = self.name(&format!("{id}-service"));
        if challenge.network.mode == "local_service" {
            let image = self.checked_image(id)?;
            ensure!(
                !challenge.runtime.privileged || allow_privileged,
                "this jail requests privileged mode; explicitly pass --allow-privileged after reviewing its Dockerfile"
            );
            if let Some(existing) = self.docker.inspect("container", &service)? {
                self.owned(&existing, &service)?;
                ensure!(
                    existing["Image"] == image,
                    "service uses a different image; run challenge down first"
                );
                self.docker.run(&["start", &service])?;
            } else {
                let mut args = vec!["run", "-d", "--name", &service, "--label"];
                let label = format!("re-agent.workspace={}", self.id);
                args.extend([
                    &label,
                    "--network",
                    &env.network,
                    "--network-alias",
                    "challenge",
                    "--pids-limit",
                    "256",
                    "--memory",
                    "1g",
                ]);
                if challenge.runtime.privileged {
                    args.push("--privileged");
                } else {
                    args.extend(["--cap-drop", "ALL", "--security-opt", "no-new-privileges"]);
                }
                args.push(&image);
                self.docker.run(&args)?;
            }
        } else if challenge.network.mode == "remote" {
            self.start_relay(&challenge, &env, &service)?;
        }
        self.write_worker_config(&env, &self.challenge_dir(id)?.join("re-agent.toml"))?;
        self.challenge_status(id)
    }
    fn start_relay(&self, challenge: &Challenge, env: &Environment, service: &str) -> Result<()> {
        let egress = self.name(&format!("{}-egress", challenge.id));
        if let Some(existing) = self.docker.inspect("network", &egress)? {
            self.owned(&existing, &egress)?;
        } else {
            self.docker.run(&[
                "network",
                "create",
                "--label",
                &format!("re-agent.workspace={}", self.id),
                &egress,
            ])?;
        }
        if let Some(existing) = self.docker.inspect("container", service)? {
            self.owned(&existing, service)?;
            self.docker.run(&["rm", "-f", "-v", service])?;
        }
        self.docker.run(&[
            "create",
            "--name",
            service,
            "--label",
            &format!("re-agent.workspace={}", self.id),
            "--network",
            &egress,
            "--read-only",
            "--user",
            "65534:65534",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--pids-limit",
            "32",
            "--memory",
            "128m",
            "--entrypoint",
            "python3",
            &env.image_id,
            "/opt/re-agent/runtime/workbench_relay.py",
            challenge.network.host.as_deref().unwrap(),
            &challenge.network.port.to_string(),
        ])?;
        self.docker.run(&[
            "network",
            "connect",
            "--alias",
            "challenge",
            &env.network,
            service,
        ])?;
        self.docker.run(&["start", service])?;
        Ok(())
    }
    pub fn challenge_status(&self, id: &str) -> Result<Value> {
        let challenge = self.challenge(id)?;
        let env = self.challenge_environment(id)?;
        let service = self.name(&format!("{id}-service"));
        Ok(
            json!({"id":id,"mode":challenge.network.mode,"endpoint":if challenge.network.mode=="offline" {Value::Null}else{json!({"host":"challenge","port":challenge.network.port})},"tool_container":self.docker.inspect("container",&env.container)?,"service_container":self.docker.inspect("container",&service)?,"network":env.network,"config":self.challenge_dir(id)?.join("re-agent.toml")}),
        )
    }
    pub fn down_challenge(&self, id: &str) -> Result<Value> {
        self.challenge(id)?;
        self.challenge_idle(id)?;
        let env = self.challenge_environment(id)?;
        for name in [self.name(&format!("{id}-service")), env.container.clone()] {
            if let Some(existing) = self.docker.inspect("container", &name)? {
                self.owned(&existing, &name)?;
                self.docker.run(&["rm", "-f", "-v", &name])?;
            }
        }
        for name in [env.network, self.name(&format!("{id}-egress"))] {
            if let Some(existing) = self.docker.inspect("network", &name)? {
                self.owned(&existing, &name)?;
                self.docker.run(&["network", "rm", &name])?;
            }
        }
        Ok(json!({"id":id,"stopped":true,"data_preserved":true}))
    }
    /// Export the original image filesystem without running its entrypoint.
    pub fn export_rootfs(&self, id: &str) -> Result<Value> {
        self.verify_original(id)?;
        let image = self.checked_image(id)?;
        let name = self.name(&format!(
            "{id}-extract-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        let root = self.challenge_dir(id)?;
        let output = root.join("prepared/rootfs.tar");
        ensure!(
            !output.exists(),
            "prepared/rootfs.tar exists; choose a new challenge or remove this generated archive first"
        );
        self.docker.run(&[
            "create",
            "--name",
            &name,
            "--label",
            &format!("re-agent.workspace={}", self.id),
            "--network",
            "none",
            &image,
        ])?;
        let result = self.docker.logged(
            &[
                "export",
                "--output",
                &portable_path(&output).to_string_lossy(),
                &name,
            ],
            &root.join("results/export-rootfs.log"),
        );
        let cleanup = self.docker.run(&["rm", "-v", &name]);
        result?;
        cleanup?;
        let digest = crate::binary::sha256_file(&output)?;
        write_json(
            &root.join("prepared/rootfs.json"),
            &json!({"image_id":image,"sha256":digest,"rootfs_prefix":self.challenge(id)?.runtime.rootfs,"runtime_confirmed":false}),
        )?;
        Ok(json!({"archive":output,"sha256":digest,"runtime_confirmed":false}))
    }
}

impl Workbench {
    pub fn extract_file(&self, id: &str, path: &str, role: &str) -> Result<Value> {
        ensure!(
            ["binary", "libc", "loader"].contains(&role),
            "role must be binary, libc or loader"
        );
        ensure!(
            path.starts_with('/') && !path.contains('\0') && !path.split('/').any(|p| p == ".."),
            "container path must be absolute without traversal"
        );
        self.verify_original(id)?;
        self.challenge_idle(id)?;
        let image = self.checked_image(id)?;
        let root = self.challenge_dir(id)?;
        let relative = format!("prepared/extracted-{role}");
        let output = root.join(&relative);
        ensure!(!output.exists(), "extracted file already exists");
        let name = self.name(&format!(
            "{id}-extract-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        self.docker.run(&[
            "create",
            "--name",
            &name,
            "--label",
            &format!("re-agent.workspace={}", self.id),
            "--network",
            "none",
            &image,
        ])?;
        let result = self.docker.copy_from(&name, path, &output);
        let cleanup = self.docker.run(&["rm", "-v", &name]);
        result?;
        cleanup?;
        ensure!(
            fs::symlink_metadata(&output)?.is_file(),
            "extracted path must be a regular file; specify the resolved in-image path"
        );
        let profile = crate::binary::inspect(&output)?;
        ensure!(
            profile.format == "Elf",
            "extracted runtime file must be ELF"
        );
        let mut challenge = self.challenge(id)?;
        match role {
            "binary" => {
                challenge.target.binary = Some(relative.clone());
                challenge.architecture = profile.architecture.clone();
            }
            "libc" => challenge.runtime.libc = Some(relative.clone()),
            _ => challenge.runtime.loader = Some(relative.clone()),
        };
        fs::write(
            root.join("challenge.toml"),
            toml::to_string_pretty(&challenge)?,
        )?;
        let mut lock: Value = read_json(&root.join("environment.lock.json"))?;
        // Extraction changes only declared file paths; the built image remains the source.
        lock["build_manifest_sha256"] =
            json!(crate::binary::sha256_file(root.join("challenge.toml"))?);
        lock["extracted"][role] = json!({"image_id":image,"container_path":path,"file":relative,"sha256":crate::binary::sha256_file(&output)?});
        write_json(&root.join("environment.lock.json"), &lock)?;
        Ok(json!({"file":output,"profile":profile,"runtime_confirmed":false}))
    }
    pub fn pwninit(&self, id: &str) -> Result<Value> {
        self.verify_original(id)?;
        self.challenge_idle(id)?;
        let challenge = self.challenge(id)?;
        let root = self.challenge_dir(id)?;
        let env = self.environment()?;
        let binary = relative_path(
            &root,
            challenge
                .target
                .binary
                .as_deref()
                .context("pwninit requires binary")?,
        )?;
        let libc = relative_path(
            &root,
            challenge
                .runtime
                .libc
                .as_deref()
                .context("pwninit requires supplied libc")?,
        )?;
        let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
        let name = self.name(&format!("{id}-pwninit-{suffix}"));
        let output = root.join(format!("prepared/pwninit-{suffix}"));
        self.docker.run(&[
            "run",
            "-d",
            "--name",
            &name,
            "--label",
            &format!("re-agent.workspace={}", self.id),
            "--network",
            "bridge",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--memory",
            "1g",
            "--pids-limit",
            "64",
            &env.image_id,
        ])?;
        let result = (|| -> Result<Value> {
            self.docker.run(&["exec", &name, "mkdir", "-p", "/work"])?;
            self.docker.copy_to(&binary, &name, "/work/target")?;
            self.docker.copy_to(&libc, &name, "/work/libc.so.6")?;
            let mut args = vec![
                "exec",
                "--user",
                "65534:65534",
                "--workdir",
                "/work",
                &name,
                "timeout",
                "--kill-after=5s",
                "180",
                "pwninit",
                "--bin",
                "target",
                "--libc",
                "libc.so.6",
                "--no-template",
                "--no-patch-bin",
            ];
            if let Some(loader) = &challenge.runtime.loader {
                self.docker
                    .copy_to(&relative_path(&root, loader)?, &name, "/work/ld.so")?;
                args.extend(["--ld", "ld.so"]);
            }
            // Root changes only disposable copies. Downloads/parsing run unprivileged.
            self.docker
                .run(&["exec", &name, "chmod", "-R", "a+rwX", "/work"])?;
            // Match the immutable image's symbol cache by Build-ID before downloading.
            let cached_symbols = r#"import sys,re,json,subprocess
from pathlib import Path
sys.path.insert(0,'/opt/re-agent/runtime')
from worker_profile import file_identity
identity=file_identity('libc.so.6'); build=identity.get('build_id') or ''
source='pwninit_download'
if re.fullmatch('[0-9a-fA-F]{4,128}',build):
    symbols=Path('/usr/lib/debug/.build-id')/build[:2]/(build[2:]+'.debug')
    if symbols.is_file():
        subprocess.run(['eu-unstrip','-o','libc.symbols.so','libc.so.6',str(symbols)],check=True,timeout=30)
        Path('libc.symbols.so').replace('libc.so.6')
        source='tool_image_build_id_cache'
Path('symbols-source.json').write_text(json.dumps({'source':source,'build_id':build,'source_sha256':identity['sha256']}))
"#;
            self.docker.run(&[
                "exec",
                "--user",
                "65534:65534",
                "--workdir",
                "/work",
                &name,
                "python3",
                "-c",
                cached_symbols,
            ])?;
            let execution = self.docker.logged(&args, &root.join("results/pwninit.log"));
            fs::create_dir_all(&output)?;
            self.docker.copy_from(&name, "/work/.", &output)?;
            let files = super::challenge::inventory(&output)?;
            write_json(
                &output.join("pwninit-result.json"),
                &json!({"files":files,"source_libc_sha256":crate::binary::sha256_file(&libc)?,"runtime_confirmed":false,"success":execution.is_ok()}),
            )?;
            execution?;
            Ok(
                json!({"directory":output,"files":files,"note":"downloaded symbols may change libc bytes; originals and selected runtime remain unchanged"}),
            )
        })();
        let cleanup = self.docker.run(&["rm", "-f", "-v", &name]);
        result.and_then(|value| {
            cleanup?;
            Ok(value)
        })
    }
}

impl Workbench {
    /// An operator debugger shares only the challenge PID namespace, never the host's.
    pub fn debug_challenge(&self, id: &str) -> Result<Value> {
        self.verify_original(id)?;
        let challenge = self.challenge(id)?;
        ensure!(
            challenge.network.mode == "local_service",
            "debug companion requires a local challenge service"
        );
        let service = self.name(&format!("{id}-service"));
        let target = self
            .docker
            .inspect("container", &service)?
            .context("start the original challenge service first")?;
        self.owned(&target, &service)?;
        ensure!(
            target["State"]["Running"] == true,
            "challenge service is not running"
        );
        let image = self.checked_image(id)?;
        ensure!(
            target["Image"] == image,
            "running service does not match locked image"
        );
        let env = self.challenge_environment(id)?;
        let name = self.name(&format!("{id}-debug"));
        if let Some(existing) = self.docker.inspect("container", &name)? {
            self.owned(&existing, &name)?;
            ensure!(
                existing["HostConfig"]["PidMode"]
                    == format!("container:{}", target["Id"].as_str().unwrap()),
                "debug companion belongs to a previous service instance; run challenge down first"
            );
            self.docker.run(&["start", &name])?;
        } else {
            self.docker.run(&[
                "run",
                "-d",
                "--name",
                &name,
                "--label",
                &format!("re-agent.workspace={}", self.id),
                "--label",
                "re-agent.kind=debug-companion",
                "--pid",
                &format!("container:{service}"),
                "--network",
                &env.network,
                "--read-only",
                "--cap-drop",
                "ALL",
                "--cap-add",
                "SYS_PTRACE",
                "--cap-add",
                "DAC_OVERRIDE",
                "--security-opt",
                "no-new-privileges",
                "--security-opt",
                "seccomp=unconfined",
                "--tmpfs",
                "/tmp:rw,size=1g,exec",
                "--memory",
                "2g",
                "--pids-limit",
                "128",
                &env.image_id,
            ])?;
        }
        let processes=self.docker.run(&["exec",&name,"python3","-c","import pathlib,json; print(json.dumps([{'pid':int(p.name),'command':(p/'cmdline').read_bytes().replace(bytes([0]),b' ').decode(errors='replace')[:512]} for p in pathlib.Path('/proc').iterdir() if p.name.isdigit() and (p/'cmdline').is_file()]))"])?;
        Ok(
            json!({"container":name,"service_image_id":image,"processes":serde_json::from_str::<Value>(&processes)?,"instruction":"docker exec -it <debug-container> gdb -ex 'set sysroot /proc/<PID>/root' -p <PID>","final_verification":"restart the original service after debugger experiments and verify the protocol there"}),
        )
    }
}
