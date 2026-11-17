//! Build immutable hash-bound launch layouts without executing targets.
use super::*;

impl ExecutionService {
    pub(super) async fn prepare_launch(
        &self,
        agent: &AgentMetadata,
        call: &str,
        input: args::LaunchPrepareArgs,
    ) -> Result<Value> {
        validate_options(&input)?;
        let run: RunRecord = self.store.load(&agent.run_id)?;
        let mut members: Vec<(String, String)> = Vec::new();
        let mut loader = None;
        // The 32 MiB limit is a transfer budget: account for it before uploading so a
        // huge target is rejected instead of being sent and only then refused.
        let mut transfer_bytes = 0u64;
        let mut account = |size: u64| -> Result<()> {
            transfer_bytes = transfer_bytes
                .checked_add(size)
                .context("launch size overflow")?;
            ensure!(transfer_bytes <= 32 * 1024 * 1024, "launch exceeds 32 MiB");
            Ok(())
        };
        let entry = match input.source {
            args::LaunchSource::Primary => {
                account(fs::metadata(&run.target_path)?.len())?;
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
            args::LaunchSource::Bundle {
                entry,
                members: selected,
            } => {
                let bundle = bundle::select_members(
                    run.bundle.context("target is not a directory bundle")?,
                    entry,
                    selected,
                )?;
                let policy = hex::encode(Sha256::digest(serde_json::to_vec(
                    &json!({"bundle":bundle.bundle_sha256,"entry":bundle.entry}),
                )?));
                validate_cwd(
                    input.cwd.as_deref().unwrap_or("."),
                    bundle.members.iter().map(|m| m.path.as_str()),
                )?;
                for member in bundle.members {
                    account(fs::metadata(bundle.root.join(&member.path))?.len())?;
                    let relative = format!("bundles/{policy}/{}", member.path);
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
            validate_member_path(name)?;
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
        validate_cwd(&cwd, members.iter().map(|(name, _)| name.as_str()))?;
        let mut launch = json!({"schema_version":1,"target":entry,"files":files,"args":input.args,"cwd":cwd,"env":input.env,"input_mode":input.input_mode,"timeout_secs":input.timeout_secs});
        if let Some(loader) = loader {
            launch["loader"] = json!(loader);
            launch["library_path"] = json!("lib");
        }
        let digest = hex::encode(Sha256::digest(serde_json::to_vec(&launch)?));
        let base = format!("launches/{digest}");
        launch["base"] = json!(format!("files/{base}"));
        ensure!(
            serde_json::to_vec(&launch)?.len() <= 65536,
            "LaunchSpec exceeds 64 KiB; reduce arguments or selected members"
        );
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

pub(super) fn validate_member_path(name: &str) -> Result<()> {
    wukong_worker_package::validate_path(name)?;
    ensure!(
        name.len() <= 1024 && name.split('/').count() <= 5,
        "launch member path is too deep or long"
    );
    ensure!(
        name != "launch.json" && !name.starts_with("launch.json/"),
        "launch.json is reserved for the manifest"
    );
    Ok(())
}
fn validate_cwd<'a>(cwd: &str, members: impl Iterator<Item = &'a str>) -> Result<()> {
    if cwd != "." {
        validate_member_path(cwd)?;
        ensure!(
            members
                .into_iter()
                .any(|n| n.starts_with(&format!("{cwd}/"))),
            "cwd is not in the launch layout"
        );
    }
    Ok(())
}
fn validate_options(input: &args::LaunchPrepareArgs) -> Result<()> {
    ensure!(
        input.args.len() <= 64
            && input
                .args
                .iter()
                .all(|a| a.len() <= 32768 && !a.contains('\0')),
        "invalid launch arguments"
    );
    ensure!(
        (1..=3600).contains(&input.timeout_secs),
        "invalid launch timeout"
    );
    ensure!(
        input.env.len() <= 64
            && input.env.iter().all(|(k, v)| !k.is_empty()
                && k.len() <= 256
                && !k.contains(['=', '\0'])
                && v.len() <= 32768
                && !v.contains('\0')),
        "invalid launch environment"
    );
    ensure!(
        serde_json::to_vec(input)?.len() <= 60 * 1024,
        "launch options exceed 60 KiB"
    );
    if let Some(cwd) = &input.cwd
        && cwd != "."
    {
        validate_member_path(cwd)?;
    }
    ensure!(
        !matches!(input.input_mode, args::LaunchInputMode::ArgvFile)
            || input.args.iter().any(|a| a == "@@"),
        "argv_file requires an @@ argument"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launch_options_and_layout_are_validated_before_upload() -> Result<()> {
        let good = json!({"source":{"kind":"bundle","entry":"two.exe","members":["two.exe"]},"args":[],"input_mode":"stdin_pipe","timeout_secs":30});
        validate_options(&serde_json::from_value(good.clone())?)?;
        for (key, value) in [
            ("cwd", json!("../outside")),
            ("args", json!(["x".repeat(32769)])),
            ("env", json!({"BAD=KEY":"value"})),
            ("timeout_secs", json!(0)),
            ("input_mode", json!("argv_file")),
        ] {
            let mut input = good.clone();
            input[key] = value;
            assert!(
                validate_options(&serde_json::from_value(input)?).is_err(),
                "{key}"
            );
        }
        assert!(validate_member_path("launch.json").is_err());
        assert!(validate_cwd("work", ["two.exe"].into_iter()).is_err());
        assert!(validate_cwd("work", ["work/data"].into_iter()).is_ok());
        Ok(())
    }
}
