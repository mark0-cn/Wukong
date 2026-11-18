use super::{
    RECEIPT_VERSION, installation_receipt_path, installed_receipt, installed_receipts,
    refreshed_receipt,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn fixture_receipt(installation_id: &str) -> Value {
    json!({
        "schema": 1,
        "receipt_version": RECEIPT_VERSION,
        "installed": true,
        "installation_id": installation_id,
        "environment_id": "fixture_environment",
        "generation": 1,
        "manifest_sha256": "a".repeat(64),
        "ssh_user": "fixture_worker",
        "ssh_port": 2222,
        "host_key": {"algorithm": "ssh-ed25519", "key": "AAAAC3NzaC1lZDI1NTE5AAAAIFixture"},
        "host_fingerprint": "SHA256:fixture",
        "candidate_addresses": ["192.168.1.10"],
        "production_isolation_verified": false
    })
}

fn write_receipt(base: &Path, installation_id: &str, receipt: &Value) -> Result<()> {
    let directory = base.join(installation_id);
    fs::create_dir_all(&directory)?;
    fs::write(
        directory.join("connection.json"),
        serde_json::to_vec(receipt)?,
    )?;
    Ok(())
}

#[test]
fn installed_receipts_accept_only_complete_matching_receipts() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let base = temporary.path().join("WukongWorkers");
    let valid = "a".repeat(32);
    let foreign = "b".repeat(32);
    let malformed = "c".repeat(32);
    let missing = "d".repeat(32);
    let invalid_name = "z".repeat(32);
    write_receipt(&base, &valid, &fixture_receipt(&valid))?;
    // A receipt that claims another installation directory must never enable re-export.
    write_receipt(&base, &foreign, &fixture_receipt(&valid))?;
    fs::create_dir_all(base.join(&malformed))?;
    fs::write(base.join(&malformed).join("connection.json"), b"{not json")?;
    fs::create_dir_all(base.join(&missing))?;
    fs::write(base.join(&invalid_name), b"not an installation directory")?;

    let receipts = installed_receipts(&base)?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].installation_id, valid);
    assert_eq!(receipts[0].receipt["host_fingerprint"], "SHA256:fixture");
    Ok(())
}

#[test]
fn installed_receipts_are_ordered_and_an_absent_base_is_empty() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let base = temporary.path().join("WukongWorkers");
    assert!(installed_receipts(&base)?.is_empty());
    let later = "b".repeat(32);
    let earlier = "a".repeat(32);
    write_receipt(&base, &later, &fixture_receipt(&later))?;
    write_receipt(&base, &earlier, &fixture_receipt(&earlier))?;
    let ids = installed_receipts(&base)?
        .into_iter()
        .map(|found| found.installation_id)
        .collect::<Vec<_>>();
    assert_eq!(ids, [earlier, later]);
    Ok(())
}

#[test]
fn installed_receipt_rejects_schema_identity_and_shape_changes() -> Result<()> {
    let installation_id = "a".repeat(32);
    let valid = serde_json::to_vec(&fixture_receipt(&installation_id))?;
    assert_eq!(
        installed_receipt(&valid, &installation_id)?["installation_id"],
        installation_id
    );
    assert!(installed_receipt(b"[]", &installation_id).is_err());
    let mut wrong_schema = fixture_receipt(&installation_id);
    wrong_schema["schema"] = json!(2);
    assert!(installed_receipt(&serde_json::to_vec(&wrong_schema)?, &installation_id).is_err());
    let mut uninstalled = fixture_receipt(&installation_id);
    uninstalled["installed"] = json!(false);
    assert!(installed_receipt(&serde_json::to_vec(&uninstalled)?, &installation_id).is_err());
    let mut missing_identity = fixture_receipt(&installation_id);
    missing_identity
        .as_object_mut()
        .expect("fixture object")
        .remove("host_fingerprint");
    assert!(installed_receipt(&serde_json::to_vec(&missing_identity)?, &installation_id).is_err());
    assert!(installed_receipt(&valid, &"b".repeat(32)).is_err());
    Ok(())
}

#[test]
fn refresh_updates_only_the_receipt_version_and_address_candidates() -> Result<()> {
    let installation_id = "a".repeat(32);
    let original = fixture_receipt(&installation_id);
    let refreshed = refreshed_receipt(&original, vec!["10.1.2.3".into(), "192.168.1.10".into()])?;
    assert_eq!(refreshed["receipt_version"], RECEIPT_VERSION);
    assert_eq!(
        refreshed["candidate_addresses"],
        json!(["10.1.2.3", "192.168.1.10"])
    );
    let mut expected = original.as_object().expect("fixture object").clone();
    expected.remove("candidate_addresses");
    let mut changed = refreshed.as_object().expect("refreshed object").clone();
    changed.remove("candidate_addresses");
    assert_eq!(Value::Object(changed), Value::Object(expected));
    assert_eq!(refreshed["installation_id"], installation_id);
    assert_eq!(refreshed["host_key"], original["host_key"]);
    assert_eq!(refreshed["host_fingerprint"], original["host_fingerprint"]);
    // A saved refreshed receipt must stay byte-valid JSON with the same shape.
    let bytes = serde_json::to_vec(&refreshed)?;
    assert_eq!(serde_json::from_slice::<Value>(&bytes)?, refreshed);
    Ok(())
}

#[test]
fn refresh_upgrades_a_version_one_receipt() -> Result<()> {
    let installation_id = "a".repeat(32);
    let mut original = fixture_receipt(&installation_id);
    let fields = original.as_object_mut().expect("fixture object");
    fields.remove("receipt_version");
    fields.remove("candidate_addresses");
    let refreshed = refreshed_receipt(&original, vec!["192.168.1.10".into()])?;
    assert_eq!(refreshed["receipt_version"], RECEIPT_VERSION);
    assert_eq!(
        refreshed["candidate_addresses"],
        json!(["192.168.1.10"]),
        "refreshed export must replace older candidates"
    );
    Ok(())
}

#[test]
fn refresh_rejects_incomplete_receipts() -> Result<()> {
    assert!(refreshed_receipt(&json!({"schema": 1}), Vec::new()).is_err());
    let mut invalid_identity = fixture_receipt(&"a".repeat(32));
    invalid_identity["installation_id"] = json!("not-an-installation");
    assert!(refreshed_receipt(&invalid_identity, Vec::new()).is_err());
    Ok(())
}

#[test]
fn receipt_paths_revalidate_the_installation_identifier() -> Result<()> {
    let base = PathBuf::from(r"C:\fixture\WukongWorkers");
    let installation_id = "a".repeat(32);
    assert_eq!(
        installation_receipt_path(&base, &installation_id)?,
        base.join(&installation_id).join("connection.json")
    );
    assert!(installation_receipt_path(&base, "not-an-installation").is_err());
    assert!(installation_receipt_path(&base, &"A".repeat(32)).is_err());
    Ok(())
}
