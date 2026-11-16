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
