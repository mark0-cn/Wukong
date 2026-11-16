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

#[derive(Clone)]
pub(super) struct BundleService {
    pub(super) store: Store,
}

/// Resolve explicit entry/member selection before uploading any bundle data.
pub(super) fn select_members(
    mut bundle: crate::domain::bundle::BundleManifest,
    entry: Option<String>,
    selected: Option<Vec<String>>,
) -> Result<crate::domain::bundle::BundleManifest> {
    if let Some(entry) = entry {
        bundle.entry = entry;
    }
    if let Some(selected) = selected {
        ensure!(
            !selected.is_empty() && selected.len() <= 64,
            "select 1 through 64 bundle members"
        );
        let unique: std::collections::BTreeSet<_> = selected.iter().collect();
        ensure!(unique.len() == selected.len(), "duplicate selected member");
        for path in &selected {
            ensure!(
                bundle.members.iter().any(|m| &m.path == path),
                "unknown bundle member: {path}"
            );
        }
        bundle.members.retain(|m| selected.contains(&m.path));
    }
    let target = bundle
        .members
        .iter()
        .find(|m| m.path == bundle.entry)
        .context("entry must be a selected bundle member")?;
    bundle.entry_sha256 = target.sha256.clone();
    ensure!(
        !bundle.members.is_empty() && bundle.members.len() <= 64,
        "launch supports 1 through 64 members"
    );
    let mut total = 0u64;
    for member in &bundle.members {
        super::launch::validate_member_path(&member.path)?;
        total = total
            .checked_add(member.size)
            .context("launch size overflow")?;
    }
    ensure!(total <= 32 * 1024 * 1024, "launch exceeds 32 MiB");
    Ok(bundle)
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
    #[test]
    fn entry_and_subset_select_the_executed_identity() -> Result<()> {
        let selected = select_members(
            bundle(),
            Some("two.exe".into()),
            Some(vec!["two.exe".into()]),
        )?;
        assert_eq!(selected.entry, "two.exe");
        assert_eq!(selected.entry_sha256, "c".repeat(64));
        assert_eq!(selected.members.len(), 1);
        Ok(())
    }
    #[test]
    fn invalid_selection_rejected_before_upload() {
        assert!(select_members(bundle(), Some("missing".into()), None).is_err());
        assert!(select_members(bundle(), None, Some(vec!["two.exe".into()])).is_err());
        assert!(select_members(bundle(), None, Some(vec![])).is_err());
        assert!(select_members(bundle(), None, Some(vec!["one.exe".into(); 2])).is_err());
        let mut unsafe_bundle = bundle();
        unsafe_bundle.members[1].path = "../outside".into();
        assert!(select_members(unsafe_bundle, None, None).is_err());
    }
}
