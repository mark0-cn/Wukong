//! Bundle discovery and staging tools.
use super::*;
use crate::domain::RunRecord;
use futures::future::BoxFuture;
use sha2::{Digest, Sha256};
use std::fs;

fn run_bundle(
    tools: &BundleService,
    agent: &AgentMetadata,
) -> Result<crate::domain::bundle::BundleManifest> {
    let run: RunRecord = tools.store.load(&agent.run_id)?;
    run.bundle
        .context("target is not a directory bundle; pass a directory to analyze")
}

pub(super) fn handle_bundle_manifest<'a>(
    tools: &'a BundleService,
    agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { Ok(json!({"bundle": run_bundle(tools, agent)?})) })
}

pub(super) fn handle_bundle_evidence<'a>(
    tools: &'a BundleService,
    agent: &'a AgentMetadata,
    call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::BundleMemberArgs = serde_json::from_value(args)?;
        let bundle = run_bundle(tools, agent)?;
        let member = bundle
            .members
            .iter()
            .find(|member| member.path == input.path)
            .with_context(|| format!("bundle member not found: {}", input.path))?;
        let bytes = fs::read(bundle.root.join(&member.path))?;
        ensure!(
            bytes.len() as u64 == member.size
                && hex::encode(Sha256::digest(&bytes)) == member.sha256,
            "bundle member failed verification"
        );
        let evidence = tools.store.artifact(
            &agent.run_id,
            &agent.id,
            call,
            "bundle.member",
            &bytes,
            "application/octet-stream",
            json!({"bundle_sha256":bundle.bundle_sha256,"path":member.path,"sha256":member.sha256,"kind":member.kind}),
        )?;
        Ok(json!({
            "evidence_id": evidence.id,
            "path": member.path,
            "sha256": member.sha256,
            "kind": member.kind,
        }))
    })
}

pub(super) fn handle_bundle_stage<'a>(
    tools: &'a BundleService,
    agent: &'a AgentMetadata,
    call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let input: args::BundleStageArgs = serde_json::from_value(args)?;
        let StagingPlan {
            bundle,
            base_rel,
            launch_text,
            launch_sha256,
        } = StagingPlan::new(run_bundle(tools, agent)?, input)?;
        let mut staged = Vec::new();
        for member in &bundle.members {
            let upload_rel = format!("{base_rel}/{}", member.path);
            let raw = tools
                .execution
                .files
                .upload_path(
                    agent,
                    &format!("{call}:{}", member.path),
                    &bundle.root.join(&member.path),
                    &upload_rel,
                    &member.sha256,
                    member.path == bundle.entry,
                )
                .await?;
            tools.execution.files.register_file(agent, &raw, None)?;
            staged.push(json!({
                "path": member.path,
                "kind": member.kind,
                "sha256": member.sha256,
                "worker_path": raw["path"],
            }));
        }
        let uploaded = tools
            .execution
            .files
            .file_put(
                agent,
                &format!("{call}:launch"),
                &json!({"text": launch_text, "path": format!("{base_rel}/launch-{launch_sha256}.json")}),
            )
            .await?;
        let launch_path = uploaded["file"]["path"]
            .as_str()
            .context("launch spec upload returned no path")?
            .to_owned();
        Ok(json!({
            "bundle_sha256": bundle.bundle_sha256,
            "entry": bundle.entry,
            "launch_spec": launch_path,
            "members": staged,
            "hint": "call job_start with launch_spec and no argv; the worker stages all members into one directory and runs entry there",
        }))
    })
}

#[derive(Clone)]
pub(super) struct BundleService {
    pub(super) store: Store,
    pub(super) execution: ExecutionService,
}

fn safe_member_path(path: &str) -> bool {
    wukong_worker_package::validate_path(path).is_ok()
        && !path.is_empty()
        && path.len() <= 1024
        && path.split('/').count() <= 6
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && !path.contains(['\\', ':', '\0'])
}

struct StagingPlan {
    bundle: crate::domain::bundle::BundleManifest,
    base_rel: String,
    launch_text: String,
    launch_sha256: String,
}
impl StagingPlan {
    fn new(
        mut bundle: crate::domain::bundle::BundleManifest,
        input: args::BundleStageArgs,
    ) -> Result<Self> {
        if let Some(entry) = input.entry {
            bundle.entry = entry;
        }
        if let Some(selected) = input.members {
            ensure!(
                !selected.is_empty() && selected.len() <= 64,
                "select 1 through 64 bundle members"
            );
            for path in &selected {
                ensure!(
                    bundle.members.iter().any(|m| &m.path == path),
                    "unknown bundle member: {path}"
                );
            }
            bundle.members.retain(|m| selected.contains(&m.path));
        }
        ensure!(
            bundle.members.iter().any(|m| m.path == bundle.entry),
            "entry must be a selected bundle member"
        );
        bundle.entry_sha256 = bundle
            .members
            .iter()
            .find(|m| m.path == bundle.entry)
            .unwrap()
            .sha256
            .clone();
        ensure!(
            input.args.len() <= 64
                && input
                    .args
                    .iter()
                    .all(|s| s.len() <= 32768 && !s.contains('\0')),
            "invalid bundle arguments"
        );
        let cwd = input.cwd.unwrap_or_else(|| ".".into());
        ensure!(
            cwd == "." || safe_member_path(&cwd),
            "unsafe bundle working directory"
        );
        let mode = input.input_mode.unwrap_or(args::BundleInputMode::StdinPipe);
        ensure!(
            !matches!(mode, args::BundleInputMode::ArgvFile)
                || input.args.iter().any(|a| a == "@@"),
            "argv_file requires an @@ argument"
        );
        ensure!(
            input.timeout_secs.is_none_or(|t| (1..=3600).contains(&t)),
            "invalid bundle timeout"
        );
        for member in &bundle.members {
            ensure!(safe_member_path(&member.path), "unsafe bundle member path");
        }
        ensure!(
            bundle.members.len() <= 64,
            "native bundle launch supports at most 64 members"
        );
        let policy = hex::encode(Sha256::digest(serde_json::to_vec(
            &json!({"bundle":bundle.bundle_sha256,"entry":bundle.entry}),
        )?));
        let base_rel = format!("bundles/{policy}");
        let base = format!("files/{base_rel}");
        let files: serde_json::Map<String, Value> = bundle
            .members
            .iter()
            .map(|m| (m.path.clone(), json!(m.sha256)))
            .collect();
        let launch = json!({"schema_version":1,"base":base,"target":bundle.entry,"files":files,"args":input.args,"cwd":cwd,"input_mode":mode,"timeout_secs":input.timeout_secs.unwrap_or(300)});
        let launch_text = serde_json::to_string(&launch)?;
        ensure!(
            launch_text.len() <= 65536,
            "LaunchSpec exceeds 64 KiB; reduce arguments or selected members"
        );
        let launch_sha256 = hex::encode(Sha256::digest(launch_text.as_bytes()));
        Ok(Self {
            bundle,
            base_rel,
            launch_text,
            launch_sha256,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bundle() -> crate::domain::bundle::BundleManifest {
        serde_json::from_value(json!({"schema":1,"root":"fixtures","bundle_sha256":"a".repeat(64),"entry":"one.exe","entry_sha256":"b".repeat(64),"edges":[],"members":[
            {"path":"one.exe","sha256":"b".repeat(64),"size":1,"format":"pe","kind":"executable"},
            {"path":"two.exe","sha256":"c".repeat(64),"size":1,"format":"pe","kind":"executable"}
        ]})).unwrap()
    }
    fn plan(input: Value) -> Result<StagingPlan> {
        StagingPlan::new(bundle(), serde_json::from_value(input)?)
    }
    #[test]
    fn launch_changes_get_new_paths_and_entry_changes_get_new_execution_policies() -> Result<()> {
        let original = plan(json!({}))?;
        let timeout = plan(json!({"timeout_secs":15}))?;
        assert_eq!(original.base_rel, timeout.base_rel);
        assert_ne!(original.launch_sha256, timeout.launch_sha256);
        let entry = plan(
            json!({"entry":"two.exe","members":["two.exe"],"args":["--input","@@"],"input_mode":"argv_file","cwd":"work"}),
        )?;
        assert_ne!(original.base_rel, entry.base_rel);
        let launch: Value = serde_json::from_str(&entry.launch_text)?;
        assert_eq!(launch["target"], "two.exe");
        assert_eq!(launch["cwd"], "work");
        assert_eq!(launch["files"].as_object().unwrap().len(), 1);
        Ok(())
    }
    #[test]
    fn invalid_launches_fail_before_any_upload_is_planned() {
        for input in [
            json!({"entry":"missing"}),
            json!({"members":["two.exe"]}),
            json!({"cwd":"../outside"}),
            json!({"input_mode":"argv_file"}),
            json!({"typo":true}),
            json!({"args":["x".repeat(32768),"x".repeat(32768)]}),
        ] {
            assert!(plan(input).is_err());
        }
    }
}
