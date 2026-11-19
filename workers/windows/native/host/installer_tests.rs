use super::{installation_receipt_path, installed_receipt, installed_receipts, refreshed_receipt};
use anyhow::Result;
use base64::Engine;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
use wukong_worker_package::{ConnectionReceipt, RECEIPT_VERSION, SshPublicKey};

fn fixture_receipt(installation_id: &str) -> Value {
    let mut blob = vec![0, 0, 0, 11];
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&[0, 0, 0, 32]);
    blob.extend_from_slice(&[7; 32]);
    let host_key = SshPublicKey {
        algorithm: "ssh-ed25519".into(),
        key: base64::engine::general_purpose::STANDARD.encode(blob),
    };
    let fingerprint = host_key.fingerprint().expect("fixture key");
    json!({
        "schema": 1,
        "receipt_version": RECEIPT_VERSION,
        "installed": true,
        "installation_id": installation_id,
        "environment_id": "fixture_environment",
        "generation": 1,
        "manifest_sha256": "a".repeat(64),
        "ssh_user": format!("wkgm{}", &installation_id[..16]),
        "ssh_port": 2222,
        "host_key": host_key,
        "host_fingerprint": fingerprint,
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
    assert_eq!(
        receipts[0].host_fingerprint,
        fixture_receipt(&valid)["host_fingerprint"]
    );
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
        installed_receipt(&valid, &installation_id)?.installation_id,
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
fn refresh_updates_only_the_address_candidates() -> Result<()> {
    let installation_id = "a".repeat(32);
    let original = fixture_receipt(&installation_id);
    let parsed: ConnectionReceipt = serde_json::from_value(original.clone())?;
    let refreshed = serde_json::to_value(refreshed_receipt(
        &parsed,
        vec!["10.1.2.3".into(), "192.168.1.10".into()],
    )?)?;
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
fn installed_receipt_requires_the_current_version() -> Result<()> {
    let installation_id = "a".repeat(32);
    let mut previous = fixture_receipt(&installation_id);
    previous["receipt_version"] = json!(1);
    assert!(installed_receipt(&serde_json::to_vec(&previous)?, &installation_id).is_err());
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
