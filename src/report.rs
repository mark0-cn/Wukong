//! Auditable reports and an integrity-checked, self-contained evidence bundle.
use crate::content::{file_identity as hash_file, sha256_bytes as digest};
use crate::i18n::{Locale, Message as UiMessage};
use crate::{domain::*, store::Store};
use anyhow::{Context, Result, ensure};
use askama::Template;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
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

fn markdown_text(value: &str) -> String {
    // Escape raw HTML inside Markdown separately from Markdown punctuation.
    let escaped = match askama::filters::escape(value, askama::filters::Html) {
        Ok(value) => value.to_string(),
        Err(never) => match never {},
    };
    escaped
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
    snapshot: &'a crate::queries::ReportSnapshot,
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

struct HtmlFinding<'a> {
    finding: &'a Finding,
    label: String,
    fields: Vec<(String, &'a str)>,
    verification: Option<&'a Verification>,
    evidence: String,
}

#[derive(Template)]
#[template(path = "report.html")]
struct HtmlReport<'a> {
    snapshot: &'a crate::queries::ReportSnapshot,
    findings: Vec<HtmlFinding<'a>>,
    summary: String,
    locale: Locale,
    status: String,
    accounting: String,
}

impl HtmlReport<'_> {
    fn label(&self, key: &str) -> String {
        self.locale.text(key)
    }
}
fn readable_report(
    snapshot: &crate::queries::ReportSnapshot,
    locale: Locale,
) -> Result<(String, String)> {
    let run = &snapshot.run;
    let tr = |key: &str| locale.text(key);
    let summary = run
        .summary
        .clone()
        .unwrap_or_else(|| tr("report-no-summary"));
    let status = tr(&format!("report-status-{}", run.status));
    let mut md = format!(
        "# {}\n\n{}: `{}`\n\n{}: `{}`\n\n{}: {}\n\n{}: {}\n\n{}\n\n",
        tr("report-title"),
        tr("report-run"),
        markdown_text(&run.id),
        tr("report-target"),
        run.target_sha256,
        tr("report-status"),
        status,
        tr("report-task"),
        markdown_text(&run.task),
        markdown_text(&summary)
    );
    let mut findings = Vec::new();
    md.push_str(&format!("## {}\n\n", tr("report-findings")));
    if snapshot.findings.is_empty() {
        md.push_str(&format!("{}\n\n", tr("report-no-findings")));
    }
    for finding in &snapshot.findings {
        let verification = validated_verification(snapshot, finding);
        let label = if finding.stage == FindingStage::ImpactVerified && verification.is_none() {
            tr("report-unconfirmed")
        } else {
            tr(&format!("report-stage-{}", stage(finding.stage)))
        };
        md.push_str(&format!(
            "### {}\n\nID: `{}`\n\n{}: {}\n\n{}\n\n",
            markdown_text(&finding.title),
            markdown_text(&finding.id),
            tr("report-stage"),
            label,
            markdown_text(&finding.description)
        ));
        let mut fields = Vec::new();
        for (key, value) in [
            ("report-root-cause", finding.root_cause.as_deref()),
            ("report-impact", finding.impact.as_deref()),
            (
                "report-severity",
                finding.severity.map(|severity| match severity {
                    Severity::Critical => "critical",
                    Severity::High => "high",
                    Severity::Medium => "medium",
                    Severity::Low => "low",
                    Severity::Info => "info",
                }),
            ),
            (
                "report-severity-rationale",
                finding.severity_rationale.as_deref(),
            ),
        ] {
            if let Some(value) = value {
                let name = tr(key);
                md.push_str(&format!("**{name}:** {}\n\n", markdown_text(value)));
                fields.push((name, value));
            }
        }
        if let Some(verification) = verification {
            md.push_str(&format!(
                "{}: {} / {} {}. {}: {}.\n\n",
                tr("report-replay"),
                verification.successes,
                verification.attempts,
                tr("report-attempts"),
                tr("report-verified-impact"),
                markdown_text(&verification.impact)
            ));
        }
        md.push_str(&format!(
            "{}: {}\n\n",
            tr("report-evidence"),
            finding
                .evidence_ids
                .iter()
                .map(|id| format!("`{}`", markdown_text(id)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        findings.push(HtmlFinding {
            finding,
            label,
            fields,
            verification,
            evidence: finding.evidence_ids.join(", "),
        });
    }
    md.push_str(&format!(
        "## {}\n\n| {} | {} | {} | {} |\n|---|---|---|---|\n",
        tr("report-coverage"),
        tr("report-scope"),
        tr("report-kind"),
        tr("report-outcome"),
        tr("report-notes")
    ));
    for coverage in &snapshot.coverage {
        md.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            markdown_text(&coverage.scope).replace('\n', " "),
            markdown_text(&coverage.kind),
            markdown_text(&coverage.outcome),
            markdown_text(&coverage.notes).replace('\n', " ")
        ));
    }
    let accounting = UiMessage::new("report-accounting-body")
        .arg("agents", snapshot.agents.len())
        .arg("calls", snapshot.calls.len())
        .arg("evidence", snapshot.evidence.len())
        .arg("tokens", run.total_tokens)
        .arg("cost", format!("{:.6}", run.cost_usd))
        .render(locale);
    md.push_str(&format!(
        "\n## {}\n\n{}\n\n{}\n",
        tr("report-accounting"),
        accounting,
        tr("report-raw-guide")
    ));
    let html = HtmlReport {
        snapshot,
        findings,
        summary,
        locale,
        status,
        accounting,
    }
    .render()
    .context("render HTML report")?;
    Ok((md, html))
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

fn validate_evidence_refs(snapshot: &crate::queries::ReportSnapshot) -> Result<()> {
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
    export_with_locale(store, run_id, output, Locale::EnUs)
}
pub fn export_with_locale(
    store: &Store,
    run_id: &str,
    output: &Path,
    locale: Locale,
) -> Result<ExportSummary> {
    ensure!(
        !output.exists(),
        "export destination already exists: {}",
        output.display()
    );
    let snapshot = crate::queries::ReportQueries::new(store).snapshot(run_id)?;
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
    let staging_directory = tempfile::Builder::new()
        .prefix(".export_")
        .tempdir_in(&parent)?;
    let staging = staging_directory.path();
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
                write_file(staging, &relative, &bytes, &mut files)?;
            }
            evidence_index.push(json!({"record":evidence,"bundle_path":relative}));
        }
        let assessments:Vec<_>=snapshot.findings.iter().map(|finding|json!({"finding_id":finding.id,"recorded_stage":stage(finding.stage),"independent_verification_valid":validated_verification(&snapshot,finding).is_some()})).collect();
        let mut report = json!({"schema_version":1,"exported_at":now(),"snapshot":snapshot,"finding_assessments":assessments,"evidence_index":evidence_index});
        redact(&mut report);
        write_json(staging, "report.json", &report, &mut files)?;
        let mut config = snapshot.run.config.clone();
        redact(&mut config);
        write_json(staging, "config.redacted.json", &config, &mut files)?;
        let mut replay = json!({"schema_version":1,"target":{"original_path":snapshot.run.target_path,"bundle_path":target_relative,"sha256":target_hash},"config":"config.redacted.json","tool_calls":snapshot.calls,"verifications":snapshot.verifications,"jobs":snapshot.jobs,"evidence":evidence_index,"instructions":"Verify manifest before replay. Recreate the recorded runtime and dependencies, map the original target path to target/sample.bin, import required evidence by bundle_path, then replay the recorded tool arguments. Environment credentials must be supplied separately. Export itself executes no target code."});
        redact(&mut replay);
        write_json(staging, "replay.json", &replay, &mut files)?;
        let (md, html) = readable_report(&snapshot, locale)?;
        write_file(staging, "report.md", md.as_bytes(), &mut files)?;
        write_file(staging, "report.html", html.as_bytes(), &mut files)?;
        write_file(
            staging,
            "README.txt",
            locale.text("report-bundle-readme").as_bytes(),
            &mut files,
        )?;
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
        verify_bundle(staging)?;
        fs::rename(staging, &final_path).context("publish verified export bundle")?;
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
    if result.is_ok() {
        // Publication moved the directory; disarm cleanup of its former path.
        let _ = staging_directory.keep();
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
