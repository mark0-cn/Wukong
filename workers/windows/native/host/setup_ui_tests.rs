use super::{
    ExportResult, SaveDialog, export_name, fallback_export_path, save_export_with,
    serialize_receipt, stored_receipt_bytes,
};
use anyhow::Result;
use serde_json::json;
use std::{fs, path::PathBuf};

struct Accepts(PathBuf);

impl SaveDialog for Accepts {
    fn choose(&self, _suggested_name: &str) -> Result<Option<PathBuf>> {
        Ok(Some(self.0.clone()))
    }
}

struct Cancels;

impl SaveDialog for Cancels {
    fn choose(&self, _suggested_name: &str) -> Result<Option<PathBuf>> {
        Ok(None)
    }
}

struct Unavailable;

impl SaveDialog for Unavailable {
    fn choose(&self, _suggested_name: &str) -> Result<Option<PathBuf>> {
        anyhow::bail!("the save dialog is unavailable")
    }
}

fn saved(outcome: ExportResult) -> (PathBuf, bool) {
    match outcome {
        ExportResult::Saved { path, fallback } => (path, fallback),
        other => panic!("expected a saved export, got {other:?}"),
    }
}

#[test]
fn export_writes_to_the_selected_path_without_touching_the_fallback() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let chosen = temporary.path().join("chosen.json");
    let fallback = temporary.path().join("Desktop").join("fallback.json");
    let outcome = save_export_with(
        &Accepts(chosen.clone()),
        b"{\"schema\":1}\n",
        "wukong-worker-fixture.json",
        Some(fallback.clone()),
    );
    let (path, used_fallback) = saved(outcome);
    assert_eq!(path, chosen);
    assert!(!used_fallback);
    assert_eq!(fs::read(&chosen)?, b"{\"schema\":1}\n");
    assert!(!fallback.exists());
    Ok(())
}

#[test]
fn export_falls_back_when_the_dialog_is_unavailable() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let fallback = temporary.path().join("Desktop").join("fallback.json");
    fs::create_dir_all(fallback.parent().expect("fallback parent"))?;
    let outcome = save_export_with(
        &Unavailable,
        b"{\"installed\":true}\n",
        "wukong-worker-fixture.json",
        Some(fallback.clone()),
    );
    let (path, used_fallback) = saved(outcome);
    assert_eq!(path, fallback);
    assert!(used_fallback);
    assert_eq!(fs::read(&path)?, b"{\"installed\":true}\n");
    Ok(())
}

#[test]
fn export_falls_back_when_the_chosen_path_cannot_be_written() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    // The selected parent directory does not exist, so writing the chosen path fails.
    let chosen = temporary.path().join("missing").join("chosen.json");
    let fallback = temporary.path().join("Desktop").join("fallback.json");
    fs::create_dir_all(fallback.parent().expect("fallback parent"))?;
    let outcome = save_export_with(
        &Accepts(chosen),
        b"{\"candidate_addresses\":[]}\n",
        "wukong-worker-fixture.json",
        Some(fallback.clone()),
    );
    let (path, used_fallback) = saved(outcome);
    assert_eq!(path, fallback);
    assert!(used_fallback);
    assert_eq!(fs::read(&path)?, b"{\"candidate_addresses\":[]}\n");
    Ok(())
}

#[test]
fn cancelled_export_writes_nothing() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let fallback = temporary.path().join("Desktop").join("fallback.json");
    let outcome = save_export_with(
        &Cancels,
        b"{}\n",
        "wukong-worker-fixture.json",
        Some(fallback.clone()),
    );
    assert!(matches!(outcome, ExportResult::Cancelled));
    assert!(!fallback.exists());
    Ok(())
}

#[test]
fn failed_export_reports_when_no_fallback_location_is_available() -> Result<()> {
    let outcome = save_export_with(&Unavailable, b"{}\n", "wukong-worker-fixture.json", None);
    assert!(matches!(outcome, ExportResult::Failed(_)));
    Ok(())
}

#[test]
fn fallback_destination_prefers_the_user_desktop_then_public() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let profile = temporary.path().join("user");
    let public = temporary.path().join("public");
    fs::create_dir_all(&profile)?;
    fs::create_dir_all(&public)?;
    let name = "wukong-worker-fixture.json";
    // No Desktop anywhere: the Public profile directory is used directly.
    assert_eq!(
        fallback_export_path(Some(&profile), Some(&public), name),
        Some(public.join(name))
    );
    fs::create_dir_all(public.join("Desktop"))?;
    assert_eq!(
        fallback_export_path(Some(&profile), Some(&public), name),
        Some(public.join("Desktop").join(name))
    );
    fs::create_dir_all(profile.join("Desktop"))?;
    assert_eq!(
        fallback_export_path(Some(&profile), Some(&public), name),
        Some(profile.join("Desktop").join(name))
    );
    assert_eq!(fallback_export_path(None, None, name), None);
    Ok(())
}

#[test]
fn export_name_requires_a_valid_installation_id() -> Result<()> {
    let installation_id = "a".repeat(32);
    assert_eq!(
        export_name(&installation_id)?,
        format!("wukong-worker-{installation_id}.json")
    );
    assert!(export_name("not-an-installation").is_err());
    assert!(export_name(&"A".repeat(32)).is_err());
    Ok(())
}

#[test]
fn stored_receipt_bytes_are_reused_verbatim() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let base = temporary.path().join("WukongWorkers");
    let installation_id = "a".repeat(32);
    let directory = base.join(&installation_id);
    fs::create_dir_all(&directory)?;
    let stored = b"{\"schema\":1,\"installed\":true}\n";
    fs::write(directory.join("connection.json"), stored)?;
    assert_eq!(
        stored_receipt_bytes(&base, &installation_id)?,
        Some(stored.to_vec())
    );
    assert_eq!(stored_receipt_bytes(&base, &"b".repeat(32))?, None);
    assert!(stored_receipt_bytes(&base, "not-an-installation").is_err());
    Ok(())
}

#[test]
fn serialized_receipt_is_valid_json_with_a_trailing_newline() -> Result<()> {
    let receipt = json!({"schema": 1, "candidate_addresses": ["10.0.0.2"]});
    let bytes = serialize_receipt(&receipt)?;
    assert_eq!(bytes.last(), Some(&b'\n'));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes)?,
        receipt
    );
    Ok(())
}
