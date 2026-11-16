//! Build immutable hash-bound launch layouts without executing targets.
use super::*;

impl ExecutionService {
    pub(super) async fn prepare_launch(
        &self,
        agent: &AgentMetadata,
        call: &str,
        input: args::LaunchPrepareArgs,
    ) -> Result<Value> {
        let run: RunRecord = self.store.load(&agent.run_id)?;
        let mut members: Vec<(String, String)> = Vec::new();
        let mut loader = None;
        let entry = match input.source {
            args::LaunchSource::Primary => {
                let staged = self
                    .files
                    .stage_target(agent, &format!("{call}:primary"))
                    .await?;
                let path = text(&staged, "path")?.to_owned();
                let name = path
                    .rsplit('/')
                    .next()
                    .context("target basename missing")?
                    .to_owned();
                members.push((name.clone(), path));
                name
            }
            args::LaunchSource::Bundle => {
                let bundle = run.bundle.context("target is not a directory bundle")?;
                ensure!(
                    bundle.members.len() <= 64,
                    "launch supports at most 64 members"
                );
                for member in bundle.members {
                    let relative = format!("bundles/{}/{}", bundle.bundle_sha256, member.path);
                    let raw = self
                        .files
                        .upload_path(
                            agent,
                            &format!("{call}:{}", member.path),
                            &bundle.root.join(&member.path),
                            &relative,
                            &member.sha256,
                            member.path == bundle.entry,
                        )
                        .await?;
                    self.files.register_file(agent, &raw, None)?;
                    members.push((member.path, text(&raw, "path")?.to_owned()));
                }
                bundle.entry
            }
            args::LaunchSource::Staged {
                target,
                dependencies,
            } => {
                let name = target
                    .rsplit('/')
                    .next()
                    .context("target basename missing")?
                    .to_owned();
                self.files.known_file(agent, &target, false)?;
                members.push((name.clone(), target));
                members.extend(dependencies.into_iter().map(|m| (m.name, m.path)));
                name
            }
            args::LaunchSource::LinuxRuntime {
                target,
                libc,
                loader: path,
            } => {
                for path in [&target, &libc, &path] {
                    self.files.known_file(agent, path, false)?;
                }
                // Reuse backend ELF identity/architecture validation and preparation.
                let doctor = self.gateway.runtime()?.doctor().await?;
                ensure!(
                    doctor["methods"]["runtime.prepare"]["ready"] == true,
                    "Linux runtime preparation is unavailable"
                );
                self.gateway
                    .worker_call(
                        agent,
                        &format!("{call}:runtime"),
                        Method::RuntimePrepare
                            .decode(json!({"target":target,"libc":libc,"loader":path}))?,
                    )
                    .await?;
                members.extend([
                    ("target.bin".into(), target),
                    ("lib/libc.so.6".into(), libc),
                    ("lib/loader".into(), path),
                ]);
                loader = Some("lib/loader");
                "target.bin".into()
            }
        };
        ensure!(
            !members.is_empty() && members.len() <= 64,
            "launch supports 1 through 64 members"
        );
        let mut files = BTreeMap::new();
        let mut total = 0u64;
        for (name, path) in &members {
            wukong_worker_package::validate_path(name)?;
            ensure!(
                name.split('/').count() <= 5,
                "launch member path is too deep"
            );
            let file = self.files.known_file(agent, path, false)?;
            total = total
                .checked_add(file.size)
                .context("launch size overflow")?;
            ensure!(total <= 32 * 1024 * 1024, "launch exceeds 32 MiB");
            ensure!(
                files.insert(name.clone(), file.sha256).is_none(),
                "duplicate launch member"
            );
        }
        let cwd = input.cwd.unwrap_or_else(|| ".".into());
        if cwd != "." {
            wukong_worker_package::validate_path(&cwd)?;
            ensure!(
                members
                    .iter()
                    .any(|(n, _)| n.starts_with(&format!("{cwd}/"))),
                "cwd is not in the launch layout"
            );
        }
        ensure!(
            !matches!(input.input_mode, args::LaunchInputMode::ArgvFile)
                || input.args.iter().any(|a| a == "@@"),
            "argv_file requires an @@ argument"
        );
        let mut launch = json!({"schema_version":1,"target":entry,"files":files,"args":input.args,"cwd":cwd,"env":input.env,"input_mode":input.input_mode,"timeout_secs":input.timeout_secs});
        if let Some(loader) = loader {
            launch["loader"] = json!(loader);
            launch["library_path"] = json!("lib");
        }
        let digest = hex::encode(Sha256::digest(serde_json::to_vec(&launch)?));
        let base = format!("launches/{digest}");
        launch["base"] = json!(format!("files/{base}"));
        for (name, path) in members {
            let content = self
                .files
                .file_get(
                    agent,
                    &format!("{call}:read:{name}"),
                    &json!({"path":path,"max_bytes":32*1024*1024}),
                )
                .await?;
            self.files.file_put(agent,&format!("{call}:write:{name}"),&json!({"path":format!("{base}/{name}"),"evidence_id":content["content_evidence_id"],"executable":name==entry || Some(name.as_str())==loader})).await?;
        }
        let output=self.files.file_put(agent,&format!("{call}:manifest"),&json!({"path":format!("{base}/launch.json"),"text":serde_json::to_string(&launch)?})).await?;
        Ok(
            json!({"launch_spec":output["file"]["path"],"manifest_evidence_id":output["content_evidence_id"],"manifest":launch,"prepared":true,"executed":false}),
        )
    }
}
