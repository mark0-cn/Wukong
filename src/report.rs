//! Auditable reports and an integrity-checked, self-contained evidence bundle.
use crate::{domain::*, store::Store};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSummary {
    pub output_dir: PathBuf,
    pub report_json: PathBuf,
    pub report_markdown: PathBuf,
    pub report_html: PathBuf,
    pub manifest_path: PathBuf,
    pub files: usize,
    pub evidence: usize,
    pub findings: usize,
    pub target_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub target_path: String,
    pub target_sha256: String,
    pub files: Vec<ManifestEntry>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleVerification {
    pub files: usize,
    pub bytes: u64,
    pub target_sha256: String,
}

#[derive(Serialize)]
struct Snapshot {
    run: RunRecord,
    agents: Vec<AgentRecord>,
    findings: Vec<Finding>,
    verifications: Vec<Verification>,
    coverage: Vec<Coverage>,
    evidence: Vec<Evidence>,
    calls: Vec<ToolCallRecord>,
    documents: Vec<SharedDocument>,
    jobs: Vec<Value>,
    worker_files: Vec<Value>,
    debug_sessions: Vec<Value>,
    events: Vec<Event>,
    messages: Vec<Message>,
    task_results: Vec<Value>,
}

fn snapshot(store: &Store, run_id: &str) -> Result<Snapshot> {
    store.transaction(|tx| {
        let run = Store::tx_get(tx, "run", run_id)?.context("run does not exist")?;
        let mut query = tx.prepare(
            "SELECT seq,agent_id,kind,data,created_at FROM events WHERE run_id=?1 ORDER BY seq",
        )?;
        let rows = query
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let events = rows
            .into_iter()
            .map(|(seq, agent_id, kind, data, at)| {
                Ok(Event {
                    seq,
                    agent_id,
                    kind,
                    data: serde_json::from_str(&data)?,
                    created_at: at.parse()?,
                    run_id: run_id.into(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let messages = Store::tx_messages(tx, run_id)?;
        let mut documents = Vec::new();
        for kind in ["document", "threat_model", "note", "todo"] {
            documents.extend(Store::tx_list::<SharedDocument>(tx, kind, run_id)?);
        }
        Ok(Snapshot {
            run,
            agents: Store::tx_list(tx, "agent", run_id)?,
            findings: Store::tx_list(tx, "finding", run_id)?,
            verifications: Store::tx_list(tx, "verification", run_id)?,
            coverage: Store::tx_list(tx, "coverage", run_id)?,
            evidence: Store::tx_list(tx, "evidence", run_id)?,
            calls: Store::tx_list(tx, "call", run_id)?,
            documents,
            jobs: Store::tx_list(tx, "job", run_id)?,
            worker_files: Store::tx_list(tx, "worker_file", run_id)?,
            debug_sessions: Store::tx_list(tx, "debug_session", run_id)?,
            events,
            messages,
            task_results: Store::tx_list(tx, "task_result", run_id)?,
        })
    })
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn hash_file(path: &Path) -> Result<(u64, String)> {
    let mut input = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size = 0;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hex::encode(hash.finalize())))
}

fn secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace('-', "_");
    if key.ends_with("_env") {
        return false;
    }
    matches!(
        key.as_str(),
        "api_key"
            | "apikey"
            | "authorization"
            | "password"
            | "passwd"
            | "secret"
            | "token"
            | "credential"
            | "private_key"
    ) || [
        "_api_key",
        "_secret",
        "_token",
        "_password",
        "_credential",
        "_private_key",
    ]
    .iter()
    .any(|suffix| key.ends_with(suffix))
}

/// Configuration/metadata only. Original evidence bytes remain original and
/// are explicitly indexed by their original digests.
pub fn redact(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if secret_key(key) {
                    *value = Value::String("[REDACTED]".into());
                } else {
                    redact(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact(value);
            }
        }
        _ => {}
    }
}

fn escaped(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn markdown_text(value: &str) -> String {
    escaped(value)
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('|', "\\|")
}

fn stage(stage: FindingStage) -> &'static str {
    match stage {
        FindingStage::Suspected => "suspected",
        FindingStage::Reproduced => "reproduced",
        FindingStage::RootCauseConfirmed => "root_cause_confirmed",
        FindingStage::ImpactVerified => "impact_verified",
        FindingStage::Inconclusive => "inconclusive",
        FindingStage::Rejected => "rejected",
    }
}

fn validated_verification<'a>(
    snapshot: &'a Snapshot,
    finding: &Finding,
) -> Option<&'a Verification> {
    let id = finding.verification_id.as_ref()?;
    snapshot.verifications.iter().find(|verification| {
        verification.id == *id
            && verification.run_id == finding.run_id
            && verification.finding_id == finding.id
            && verification.passed
            && verification.attempts > 0
            && verification.successes == verification.attempts
            && !verification.evidence_ids.is_empty()
            && verification.verifier_agent_id != finding.agent_id
            && snapshot.agents.iter().any(|agent| {
                agent.id == verification.verifier_agent_id && agent.role == Role::Verifier
            })
    })
}

fn readable_report(snapshot: &Snapshot) -> (String, String) {
    let run = &snapshot.run;
    let mut md = format!(
        "# Binary analysis report\n\nRun: `{}`\n\nTarget SHA256: `{}`\n\nStatus: {}\n\nTask: {}\n\n{}\n\n",
        markdown_text(&run.id),
        run.target_sha256,
        run.status,
        markdown_text(&run.task),
        markdown_text(
            run.summary
                .as_deref()
                .unwrap_or("No final summary recorded.")
        )
    );
    let mut html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src 'none'; base-uri 'none'; form-action 'none'\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Binary analysis report</title><style>body{{max-width:1050px;margin:2rem auto;padding:0 1rem;font:16px/1.6 system-ui;color:#202a36}}code,pre{{background:#f1f4f7;overflow-wrap:anywhere}}pre{{padding:1rem;white-space:pre-wrap}}article{{border-top:1px solid #cbd2da;margin-top:2rem}}table{{border-collapse:collapse;width:100%}}td,th{{border:1px solid #cbd2da;padding:.5rem;text-align:left}}.unconfirmed{{color:#874e00}}</style></head><body><h1>Binary analysis report</h1><p>Run: <code>{}</code></p><p>Target SHA256: <code>{}</code></p><p>Status: {}</p><h2>Task</h2><pre>{}</pre><h2>Summary</h2><pre>{}</pre>",
        escaped(&run.id),
        run.target_sha256,
        run.status,
        escaped(&run.task),
        escaped(
            run.summary
                .as_deref()
                .unwrap_or("No final summary recorded.")
        )
    );
    md.push_str("## Findings\n\n");
    html.push_str("<h2>Findings</h2>");
    if snapshot.findings.is_empty() {
        md.push_str(
            "No findings recorded. This does not prove the target is free of vulnerabilities.\n\n",
        );
        html.push_str("<p>No findings recorded. This does not prove the target is free of vulnerabilities.</p>");
    }
    for finding in &snapshot.findings {
        let verification = validated_verification(snapshot, finding);
        let label = if finding.stage == FindingStage::ImpactVerified && verification.is_none() {
            "impact_verified (recorded claim; independent verification is missing or incomplete)"
        } else {
            stage(finding.stage)
        };
        md.push_str(&format!(
            "### {}\n\nID: `{}`\n\nRecorded stage: {}\n\n{}\n\n",
            markdown_text(&finding.title),
            markdown_text(&finding.id),
            label,
            markdown_text(&finding.description)
        ));
        html.push_str(&format!("<article><h3>{}</h3><p>ID: <code>{}</code></p><p class=\"unconfirmed\">Recorded stage: {}</p><pre>{}</pre>",escaped(&finding.title),escaped(&finding.id),escaped(label),escaped(&finding.description)));
        for (name, value) in [
            ("Root cause", finding.root_cause.as_deref()),
            ("Reported impact", finding.impact.as_deref()),
            (
                "Severity",
                finding.severity.map(|severity| match severity {
                    Severity::Critical => "critical",
                    Severity::High => "high",
                    Severity::Medium => "medium",
                    Severity::Low => "low",
                    Severity::Info => "info",
                }),
            ),
            ("Severity rationale", finding.severity_rationale.as_deref()),
        ] {
            if let Some(value) = value {
                md.push_str(&format!("**{name}:** {}\n\n", markdown_text(value)));
                html.push_str(&format!("<h4>{name}</h4><pre>{}</pre>", escaped(value)));
            }
        }
        if let Some(verification) = verification {
            md.push_str(&format!(
                "Independent replay: {} / {} successful attempts. Verified impact: {}.\n\n",
                verification.successes,
                verification.attempts,
                markdown_text(&verification.impact)
            ));
            html.push_str(&format!("<p>Independent replay: {} / {} successful attempts.</p><h4>Verified impact</h4><pre>{}</pre>",verification.successes,verification.attempts,escaped(&verification.impact)));
        }
        md.push_str("Evidence IDs: ");
        md.push_str(
            &finding
                .evidence_ids
                .iter()
                .map(|id| format!("`{}`", markdown_text(id)))
                .collect::<Vec<_>>()
                .join(", "),
        );
        md.push_str("\n\n");
        html.push_str(&format!(
            "<p>Evidence IDs: {}</p></article>",
            escaped(&finding.evidence_ids.join(", "))
        ));
    }
    md.push_str("## Coverage\n\n| Scope | Kind | Outcome | Notes |\n|---|---|---|---|\n");
    html.push_str("<h2>Coverage</h2><table><thead><tr><th>Scope</th><th>Kind</th><th>Outcome</th><th>Notes</th></tr></thead><tbody>");
    for coverage in &snapshot.coverage {
        md.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            markdown_text(&coverage.scope).replace('\n', " "),
            markdown_text(&coverage.kind),
            markdown_text(&coverage.outcome),
            markdown_text(&coverage.notes).replace('\n', " ")
        ));
        html.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            escaped(&coverage.scope),
            escaped(&coverage.kind),
            escaped(&coverage.outcome),
            escaped(&coverage.notes)
        ));
    }
    md.push_str(&format!("\n## Accounting\n\n{} agents; {} tool calls; {} evidence records; {} tokens; USD {:.6}.\n\nSee `report.json` and `replay.json` for environment, conditions, exact tool arguments, and evidence references.\n",snapshot.agents.len(),snapshot.calls.len(),snapshot.evidence.len(),run.total_tokens,run.cost_usd));
    html.push_str(&format!("</tbody></table><h2>Accounting</h2><p>{} agents; {} tool calls; {} evidence records; {} tokens; USD {:.6}.</p><p>See report.json and replay.json for environment, conditions, exact tool arguments, and evidence references.</p></body></html>",snapshot.agents.len(),snapshot.calls.len(),snapshot.evidence.len(),run.total_tokens,run.cost_usd));
    (md, html)
}

fn write_file(
    root: &Path,
    relative: &str,
    bytes: &[u8],
    files: &mut BTreeMap<String, ManifestEntry>,
) -> Result<()> {
    safe_relative(relative)?;
    let path = root.join(relative);
    fs::create_dir_all(path.parent().context("export file parent missing")?)?;
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    files.insert(
        relative.into(),
        ManifestEntry {
            path: relative.into(),
            size: bytes.len() as u64,
            sha256: digest(bytes),
        },
    );
    Ok(())
}

fn write_json(
    root: &Path,
    relative: &str,
    value: &impl Serialize,
    files: &mut BTreeMap<String, ManifestEntry>,
) -> Result<()> {
    write_file(root, relative, &serde_json::to_vec_pretty(value)?, files)
}

fn safe_relative(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && !path.contains(['\\', ':', '\n', '\r', '\0']),
        "invalid bundle path"
    );
    let path = Path::new(path);
    ensure!(
        !path.is_absolute()
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "bundle path escapes its root"
    );
    Ok(())
}

fn validate_evidence_refs(snapshot: &Snapshot) -> Result<()> {
    let ids: BTreeSet<_> = snapshot
        .evidence
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    for (kind, id, refs) in snapshot
        .findings
        .iter()
        .map(|v| ("finding", v.id.as_str(), &v.evidence_ids))
        .chain(
            snapshot
                .verifications
                .iter()
                .map(|v| ("verification", v.id.as_str(), &v.evidence_ids)),
        )
        .chain(
            snapshot
                .coverage
                .iter()
                .map(|v| ("coverage", v.id.as_str(), &v.evidence_ids)),
        )
    {
        for evidence in refs {
            ensure!(
                ids.contains(evidence.as_str()),
                "{kind} {id} references missing evidence {evidence}"
            );
        }
    }
    Ok(())
}

/// Export without executing the sample. Existing output paths are refused;
/// complete bundles are published by renaming a verified staging directory.
pub fn export(store: &Store, run_id: &str, output: &Path) -> Result<ExportSummary> {
    ensure!(
        !output.exists(),
        "export destination already exists: {}",
        output.display()
    );
    let snapshot = snapshot(store, run_id)?;
    validate_evidence_refs(&snapshot)?;
    let (target_size, target_hash) =
        hash_file(&snapshot.run.target_path).context("read original target for export")?;
    ensure!(
        target_hash == snapshot.run.target_sha256,
        "target SHA256 changed since run creation"
    );
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let parent = fs::canonicalize(parent)?;
    let name = output
        .file_name()
        .context("export destination must name a new directory")?;
    let final_path = parent.join(name);
    ensure!(!final_path.exists(), "export destination already exists");
    let staging = parent.join(format!(".{}.tmp", id("export")));
    fs::create_dir(&staging)?;
    let result = (|| -> Result<ExportSummary> {
        let mut files = BTreeMap::new();
        let target_relative = "target/sample.bin";
        fs::create_dir(staging.join("target"))?;
        fs::copy(&snapshot.run.target_path, staging.join(target_relative))?;
        let (copied_size, copied_hash) = hash_file(&staging.join(target_relative))?;
        ensure!(
            copied_size == target_size && copied_hash == target_hash,
            "target changed during export"
        );
        files.insert(
            target_relative.into(),
            ManifestEntry {
                path: target_relative.into(),
                size: copied_size,
                sha256: copied_hash,
            },
        );
        let mut evidence_index = Vec::new();
        for evidence in &snapshot.evidence {
            let bytes = store
                .read_evidence(run_id, &evidence.id)
                .with_context(|| format!("verify evidence {}", evidence.id))?;
            ensure!(
                bytes.len() as u64 == evidence.size,
                "evidence {} size does not match metadata",
                evidence.id
            );
            let relative = format!("evidence/{}/{}", &evidence.sha256[..2], evidence.sha256);
            if !files.contains_key(&relative) {
                write_file(&staging, &relative, &bytes, &mut files)?;
            }
            evidence_index.push(json!({"record":evidence,"bundle_path":relative}));
        }
        let assessments:Vec<_>=snapshot.findings.iter().map(|finding|json!({"finding_id":finding.id,"recorded_stage":stage(finding.stage),"independent_verification_valid":validated_verification(&snapshot,finding).is_some()})).collect();
        let mut report = json!({"schema_version":1,"exported_at":now(),"snapshot":snapshot,"finding_assessments":assessments,"evidence_index":evidence_index});
        redact(&mut report);
        write_json(&staging, "report.json", &report, &mut files)?;
        let mut config = snapshot.run.config.clone();
        redact(&mut config);
        write_json(&staging, "config.redacted.json", &config, &mut files)?;
        let mut replay = json!({"schema_version":1,"target":{"original_path":snapshot.run.target_path,"bundle_path":target_relative,"sha256":target_hash},"config":"config.redacted.json","tool_calls":snapshot.calls,"verifications":snapshot.verifications,"jobs":snapshot.jobs,"evidence":evidence_index,"instructions":"Verify manifest before replay. Recreate the recorded runtime and dependencies, map the original target path to target/sample.bin, import required evidence by bundle_path, then replay the recorded tool arguments. Environment credentials must be supplied separately. Export itself executes no target code."});
        redact(&mut replay);
        write_json(&staging, "replay.json", &replay, &mut files)?;
        let (md, html) = readable_report(&snapshot);
        write_file(&staging, "report.md", md.as_bytes(), &mut files)?;
        write_file(&staging, "report.html", html.as_bytes(), &mut files)?;
        write_file(&staging,"README.txt",b"re-agent evidence bundle\n\nreport.md / report.html: human-readable recorded findings and verification status.\nreport.json: transactional run snapshot and original evidence index.\nreplay.json: exact recorded tool calls, verification environment and conditions.\nconfig.redacted.json: settings with credential fields redacted.\ntarget/sample.bin: original target bytes (not executed during export).\nevidence/: deduplicated original evidence bytes, addressed by SHA256.\nmanifest.json / SHA256SUMS: all payload digests. Use re-agent bundle-verify.\n\nA checksum verifies integrity, not publisher identity. Original evidence is not redacted.\nRedacted configuration and event metadata are not a credential backup.\n",&mut files)?;
        let manifest = BundleManifest {
            schema_version: 1,
            run_id: run_id.into(),
            created_at: now(),
            target_path: target_relative.into(),
            target_sha256: target_hash.clone(),
            files: files.into_values().collect(),
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        fs::write(staging.join("manifest.json"), &manifest_bytes)?;
        let sums = checksum_text(&manifest, &manifest_bytes);
        fs::write(staging.join("SHA256SUMS"), sums)?;
        verify_bundle(&staging)?;
        fs::rename(&staging, &final_path).context("publish verified export bundle")?;
        Ok(ExportSummary {
            output_dir: final_path.clone(),
            report_json: final_path.join("report.json"),
            report_markdown: final_path.join("report.md"),
            report_html: final_path.join("report.html"),
            manifest_path: final_path.join("manifest.json"),
            files: manifest.files.len() + 2,
            evidence: snapshot.evidence.len(),
            findings: snapshot.findings.len(),
            target_sha256: target_hash,
        })
    })();
    if result.is_err() && staging.is_dir() {
        // Only this freshly created, canonical-parent child can be removed.
        let checked = fs::canonicalize(&staging)?;
        ensure!(
            checked.parent() == Some(parent.as_path())
                && checked
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(".export_")),
            "refuse cleanup outside export staging directory"
        );
        fs::remove_dir_all(checked)?;
    }
    result
}

fn checksum_text(manifest: &BundleManifest, manifest_bytes: &[u8]) -> String {
    let mut sums = String::new();
    for file in &manifest.files {
        sums.push_str(&format!("{}  {}\n", file.sha256, file.path));
    }
    sums.push_str(&format!("{}  manifest.json\n", digest(manifest_bytes)));
    sums
}

fn collect_files(root: &Path, current: &Path, files: &mut BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "bundle contains a symlink"
        );
        let canonical = fs::canonicalize(entry.path())?;
        ensure!(canonical.starts_with(root), "bundle file escapes its root");
        if metadata.is_dir() {
            collect_files(root, &canonical, files)?;
        } else {
            ensure!(metadata.is_file(), "bundle contains a non-file entry");
            files.insert(
                canonical
                    .strip_prefix(root)?
                    .to_str()
                    .context("non-UTF8 bundle path")?
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

/// Validate every payload, the checksum index, relative paths, and absence of
/// extra files or symlink escapes. This never starts a runtime or target.
pub fn verify_bundle(output: &Path) -> Result<BundleVerification> {
    let root = fs::canonicalize(output)?;
    ensure!(root.is_dir(), "bundle is not a directory");
    let manifest_bytes = fs::read(root.join("manifest.json"))?;
    let manifest: BundleManifest =
        serde_json::from_slice(&manifest_bytes).context("decode bundle manifest")?;
    ensure!(manifest.schema_version == 1, "unsupported bundle schema");
    let mut expected = BTreeSet::from(["manifest.json".into(), "SHA256SUMS".into()]);
    let mut bytes = 0;
    for entry in &manifest.files {
        safe_relative(&entry.path)?;
        ensure!(
            entry.sha256.len() == 64 && entry.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid manifest digest"
        );
        ensure!(
            expected.insert(entry.path.clone()),
            "duplicate or reserved manifest file"
        );
        let path = fs::canonicalize(root.join(&entry.path))?;
        ensure!(path.starts_with(&root), "bundle path escapes its root");
        let (size, hash) = hash_file(&path)?;
        ensure!(
            size == entry.size && hash == entry.sha256,
            "bundle digest or size mismatch: {}",
            entry.path
        );
        bytes += size;
    }
    safe_relative(&manifest.target_path)?;
    let target = manifest
        .files
        .iter()
        .find(|entry| entry.path == manifest.target_path)
        .context("manifest target is not indexed")?;
    ensure!(
        target.sha256 == manifest.target_sha256,
        "target digest disagrees with manifest"
    );
    ensure!(
        fs::read_to_string(root.join("SHA256SUMS"))? == checksum_text(&manifest, &manifest_bytes),
        "SHA256SUMS disagrees with manifest"
    );
    let mut actual = BTreeSet::new();
    collect_files(&root, &root, &mut actual)?;
    ensure!(
        actual == expected,
        "bundle contains missing or unindexed files"
    );
    Ok(BundleVerification {
        files: expected.len(),
        bytes,
        target_sha256: manifest.target_sha256,
    })
}
