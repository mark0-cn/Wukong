use re_agent::{
    domain::*,
    report::{self, BundleManifest},
    store::Store,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

fn fixture(root: &Path) -> (Store, String, String) {
    let store = Store::open(root.join("state")).unwrap();
    let target = root.join("fixture.bin");
    fs::write(&target, b"trusted-report-fixture").unwrap();
    let run_id = "run_fixture".to_owned();
    let run = RunRecord {
        id: run_id.clone(),
        target_path: target,
        target_sha256: hex::encode(Sha256::digest(b"trusted-report-fixture")),
        task: "inspect <script>alert(1)</script>".into(),
        status: Status::Completed,
        root_id: "agent_root".into(),
        config: json!({"model":{"provider":"openai","api_key_env":"OPENAI_API_KEY","api_key":"config-secret-1"},"mcp":{"headers":{"Authorization":"config-secret-2"},"env":{"MY_ACCESS_TOKEN":"config-secret-3"}},"budget":{"max_total_tokens":1000}}),
        budget: Budget::default(),
        created_at: now(),
        updated_at: now(),
        total_tokens: 42,
        cost_usd: 0.012,
        summary: Some("Investigated input parsing".into()),
    };
    store.put("run", &run_id, &run_id, &run).unwrap();
    let agent = AgentRecord {
        id: "agent_root".into(),
        run_id: run_id.clone(),
        parent_id: None,
        name: "Root".into(),
        role: Role::Root,
        task: "inspect".into(),
        scope: "fixture".into(),
        skills: vec![],
        active_skills: vec![],
        contract: Default::default(),
        task_result: None,
        status: Status::Completed,
        depth: 0,
        generation: 0,
        inherit_context: false,
        inherited_context: None,
        checkpoint: None,
        wait_for: vec![],
        wait_until: None,
        wait_policy: DependencyPolicy::AllTerminal,
        dependency_policy: DependencyPolicy::AllSuccess,
        predecessors_released: true,
        wait_for_predecessors: vec![],
        turns: 1,
        total_tokens: 42,
        cost_usd: 0.012,
        token_cap: None,
        summary: Some("done".into()),
        error: None,
        created_at: now(),
        updated_at: now(),
    };
    store.put("agent", &agent.id, &run_id, &agent).unwrap();
    let evidence = store
        .artifact(
            &run_id,
            &agent.id,
            "call_fixture",
            "test",
            b"original crash log",
            "text/plain",
            json!({"tool_version":"test-1"}),
        )
        .unwrap();
    let finding = Finding {
        id: "finding_fixture".into(),
        run_id: run_id.clone(),
        agent_id: agent.id.clone(),
        title: "Potential <img src=x onerror=alert(1)>".into(),
        description: "Unconfirmed parser issue".into(),
        root_cause: None,
        impact: Some("control flow claim".into()),
        severity: Some(Severity::High),
        severity_rationale: Some("recorded claim only; verification missing".into()),
        stage: FindingStage::ImpactVerified,
        location: Some(json!({"module_offset":4096})),
        evidence_ids: vec![evidence.id.clone()],
        verification_id: None,
        created_at: now(),
        updated_at: now(),
    };
    store
        .put("finding", &finding.id, &run_id, &finding)
        .unwrap();
    let coverage = Coverage {
        id: "coverage_fixture".into(),
        run_id: run_id.clone(),
        agent_id: agent.id.clone(),
        scope: "parse_input".into(),
        kind: "function".into(),
        outcome: "inspected".into(),
        notes: "Read <parser>".into(),
        evidence_ids: vec![evidence.id.clone()],
        created_at: now(),
        updated_at: now(),
    };
    store
        .put("coverage", &coverage.id, &run_id, &coverage)
        .unwrap();
    store
        .put(
            "job",
            "job_fixture",
            &run_id,
            &json!({"id":"job_fixture","status":"completed","argv":["/target/fixture"]}),
        )
        .unwrap();
    store
        .event(
            &run_id,
            Some(&agent.id),
            "test.completed",
            json!({"ok":true}),
        )
        .unwrap();
    (store, run_id, evidence.id)
}

#[test]
fn exports_complete_original_bundle_redacts_config_and_does_not_assert_unverified_impact() {
    let temp = tempfile::tempdir().unwrap();
    let (store, run_id, _) = fixture(temp.path());
    let output = temp.path().join("bundle");
    let summary = report::export(&store, &run_id, &output).unwrap();
    assert_eq!(summary.evidence, 1);
    assert_eq!(summary.findings, 1);
    assert_eq!(report::verify_bundle(&output).unwrap().files, summary.files);
    assert_eq!(
        fs::read(output.join("target/sample.bin")).unwrap(),
        b"trusted-report-fixture"
    );
    let report = fs::read_to_string(&summary.report_json).unwrap();
    for secret in ["config-secret-1", "config-secret-2", "config-secret-3"] {
        assert!(!report.contains(secret));
    }
    assert!(report.contains("OPENAI_API_KEY"));
    assert!(report.contains("max_total_tokens"));
    let json: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        json["finding_assessments"][0]["independent_verification_valid"],
        false
    );
    assert_eq!(json["snapshot"]["jobs"][0]["id"], "job_fixture");
    assert_eq!(json["snapshot"]["events"][0]["kind"], "test.completed");
    let html = fs::read_to_string(&summary.report_html).unwrap();
    assert!(!html.contains("<script>alert(1)</script>"));
    assert!(!html.contains("<img src=x"));
    assert!(html.contains("&#60;script&#62;"));
    assert!(html.contains("independent verification is missing"));
    let replay: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("replay.json")).unwrap()).unwrap();
    let artifact = replay["evidence"][0]["bundle_path"].as_str().unwrap();
    assert_eq!(
        fs::read(output.join(artifact)).unwrap(),
        b"original crash log"
    );
    assert!(report::export(&store, &run_id, &output).is_err());
}

#[test]
fn tampered_payload_extra_file_and_traversal_manifest_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let (store, run_id, _) = fixture(temp.path());
    let output = temp.path().join("bundle");
    report::export(&store, &run_id, &output).unwrap();
    fs::write(output.join("unindexed.bin"), b"extra").unwrap();
    assert!(report::verify_bundle(&output).is_err());
    fs::remove_file(output.join("unindexed.bin")).unwrap();
    fs::write(output.join("target/sample.bin"), b"changed").unwrap();
    assert!(report::verify_bundle(&output).is_err());
    fs::write(output.join("target/sample.bin"), b"trusted-report-fixture").unwrap();
    let path = output.join("manifest.json");
    let mut manifest: BundleManifest = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest.files[0].path = "../outside".into();
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(report::verify_bundle(&output).is_err());
}

#[test]
fn target_or_evidence_changes_abort_export_without_publishing_a_partial_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let (store, run_id, evidence_id) = fixture(temp.path());
    let record: Evidence = store.require("evidence", &evidence_id).unwrap();
    fs::write(
        store.root().join(&record.relative_path),
        b"corrupted evidence",
    )
    .unwrap();
    let output = temp.path().join("bundle");
    assert!(report::export(&store, &run_id, &output).is_err());
    assert!(!output.exists());
    assert!(!fs::read_dir(temp.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".export_")
    }));
    let run: RunRecord = store.require("run", &run_id).unwrap();
    fs::write(&run.target_path, b"changed target").unwrap();
    assert!(report::export(&store, &run_id, &output).is_err());
}

#[test]
fn missing_evidence_reference_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let (store, run_id, _) = fixture(temp.path());
    let mut finding: Finding = store.require("finding", "finding_fixture").unwrap();
    finding.evidence_ids.push("ev_missing".into());
    store
        .put("finding", &finding.id, &run_id, &finding)
        .unwrap();
    assert!(report::export(&store, &run_id, &temp.path().join("bundle")).is_err());
}

#[test]
fn template_escapes_verified_details_and_preserves_independent_replay_status() {
    let directory = tempfile::tempdir().unwrap();
    let (store, run_id, evidence_id) = fixture(directory.path());
    let mut verifier: AgentRecord = store.require("agent", "agent_root").unwrap();
    verifier.id = "independent_verifier".into();
    verifier.role = Role::Verifier;
    store
        .put("agent", &verifier.id, &run_id, &verifier)
        .unwrap();
    let mut finding: Finding = store.require("finding", "finding_fixture").unwrap();
    finding.root_cause = Some("<svg onload=alert(1)> & \"quoted\"".into());
    finding.verification_id = Some("verification_fixture".into());
    store
        .put("finding", &finding.id, &run_id, &finding)
        .unwrap();
    let verification = Verification {
        id: "verification_fixture".into(),
        run_id: run_id.clone(),
        finding_id: finding.id,
        verifier_agent_id: verifier.id,
        passed: true,
        attempts: 1,
        successes: 1,
        impact: "Observed <img src=x onerror=alert(1)>".into(),
        evidence_ids: vec![evidence_id],
        environment: json!({}),
        conditions: json!({}),
        result: json!({}),
        created_at: now(),
    };
    store
        .put("verification", &verification.id, &run_id, &verification)
        .unwrap();
    let report = report::export(&store, &run_id, &directory.path().join("bundle")).unwrap();
    let html = fs::read_to_string(report.report_html).unwrap();
    assert!(html.contains("1 / 1 successful attempts"));
    assert!(!html.contains("independent verification is missing"));
    assert!(!html.contains("<svg"));
    assert!(!html.contains("<img"));
    assert!(html.contains("&#60;svg onload=alert(1)&#62; &#38; &#34;quoted&#34;"));
    assert!(html.contains("Verified impact"));
}
