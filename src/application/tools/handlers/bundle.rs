//! Bundle discovery and staging tools.
use super::*;
use crate::application::tools::context::ToolContext;
use crate::domain::RunRecord;
use sha2::{Digest, Sha256};
use std::fs;

fn run_bundle(
    tools: &ToolContext,
    agent: &AgentMetadata,
) -> Result<crate::domain::bundle::BundleManifest> {
    let run: RunRecord = tools.analysis.store.load(&agent.run_id)?;
    run.bundle
        .context("target is not a directory bundle; pass a directory to analyze")
}

pub(in crate::application::tools) fn handle_bundle_manifest<'a>(
    tools: &'a ToolContext,
    agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { Ok(json!({"bundle": run_bundle(tools, agent)?})) })
}

pub(in crate::application::tools) fn handle_bundle_evidence<'a>(
    tools: &'a ToolContext,
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
        let evidence = tools.analysis.store.artifact(
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

pub(in crate::application::tools) fn handle_bundle_stage<'a>(
    tools: &'a ToolContext,
    agent: &'a AgentMetadata,
    call: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let bundle = run_bundle(tools, agent)?;
        ensure!(
            bundle.members.len() <= 64,
            "native bundle launch supports at most 64 members"
        );
        let base_rel = format!("bundles/{}", bundle.bundle_sha256);
        let base = format!("files/{base_rel}");
        let mut files = serde_json::Map::new();
        let mut staged = Vec::new();
        for member in &bundle.members {
            ensure!(
                member.path.split('/').count() <= 8,
                "bundle member path is too deep"
            );
            ensure!(
                !member
                    .path
                    .split('/')
                    .any(|part| part == ".." || part == "."),
                "bundle member path is unsafe"
            );
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
            files.insert(member.path.clone(), json!(member.sha256));
            staged.push(json!({
                "path": member.path,
                "kind": member.kind,
                "sha256": member.sha256,
                "worker_path": raw["path"],
            }));
        }
        let launch = json!({
            "schema_version": 1,
            "base": base,
            "target": bundle.entry,
            "files": files,
            "args": [],
            "cwd": ".",
            "input_mode": "stdin_pipe",
        });
        let uploaded = tools
            .execution
            .files
            .file_put(
                agent,
                &format!("{call}:launch"),
                &json!({"text": serde_json::to_string(&launch)?, "path": format!("{base_rel}/launch.json")}),
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
