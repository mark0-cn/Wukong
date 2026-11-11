use super::*;
use std::fs;
fn fixture() -> Result<(tempfile::TempDir, Cleanup)> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("WukongData");
    fs::create_dir_all(&home)?;
    fs::write(
        home.join("wukong.toml"),
        toml::to_string(&crate::config::AppConfig::default())?,
    )?;
    fs::write(home.join("preferences.json"), b"{}")?;
    let cleanup = Cleanup::at(home)?;
    Ok((temp, cleanup))
}
#[cfg(windows)]
#[test]
fn canonical_verbatim_drive_paths_are_supported() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = fs::canonicalize(temp.path())?.join("owned");
    create_owned_directory(&root)?;
    assert!(locations::owns_directory(&root));
    assert_eq!(
        files::absolute(&root)?,
        files::absolute(&crate::workbench::portable_path(&root))?
    );
    Ok(())
}
#[test]
fn deferred_uninstall_blocks_new_work_and_requires_the_matching_review_to_finish() -> Result<()> {
    let (_temp, cleanup) = fixture()?;
    let plan = cleanup.plan(Selection {
        data: true,
        ..Default::default()
    })?;
    assert!(cleanup.execute_deferred(&plan, true)?.complete);
    assert!(crate::updates::maintenance::lock(&cleanup.home.join("updates"), false).is_err());
    assert!(crate::updates::maintenance::lock(&cleanup.home.join("updates"), true).is_err());
    assert!(crate::updates::storage::cache_lock_at(&cleanup.home.join("updates"), false).is_err());
    assert!(cleanup.finish("unrelated-review").is_err());
    cleanup.finish(&plan.fingerprint)?;
    assert!(crate::updates::maintenance::lock(&cleanup.home.join("updates"), false).is_ok());
    Ok(())
}
#[test]
fn an_active_cache_download_blocks_cleanup_before_removal() -> Result<()> {
    let (_temp, cleanup) = fixture()?;
    let plan = cleanup.plan(Selection {
        data: true,
        ..Default::default()
    })?;
    let cache = crate::updates::storage::cache_lock_at(&cleanup.home.join("updates"), false)?;
    assert!(cleanup.execute(&plan).is_err());
    assert!(cleanup.home.join("wukong.toml").is_file());
    drop(cache);
    assert!(cleanup.execute(&plan)?.complete);
    Ok(())
}
#[test]
fn pending_updates_and_active_workbench_operations_preserve_settings() -> Result<()> {
    let (_temp, cleanup) = fixture()?;
    crate::workbench::write_json(
        &cleanup.home.join("updates/pending.json"),
        &serde_json::json!({}),
    )?;
    let plan = cleanup.plan(Selection {
        data: true,
        ..Default::default()
    })?;
    assert!(cleanup.execute(&plan).is_err());
    assert!(cleanup.home.join("preferences.json").is_file());
    fs::remove_file(cleanup.home.join("updates/pending.json"))?;
    let root = cleanup.home.join(".wukong/workbench");
    crate::workbench::write_json(
        &root.join("workspace.json"),
        &serde_json::json!({"schema_version":1,"id":"test-workspace"}),
    )?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("operation.lock"))?;
    file.lock()?;
    let plan = cleanup.plan(Selection {
        data: true,
        ..Default::default()
    })?;
    assert!(cleanup.execute(&plan).is_err());
    drop(file);
    let report = cleanup.execute(&plan)?;
    assert!(report.complete);
    assert!(
        root.join("workspace.json").is_file(),
        "lost ownership of retained Docker resources"
    );
    Ok(())
}
#[cfg(windows)]
#[test]
fn locked_owned_files_report_partial_failure_and_keep_configuration() -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let (_temp, cleanup) = fixture()?;
    let path = cleanup.home.join(".wukong/artifacts/locked");
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, b"locked")?;
    let _handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)?;
    let report = cleanup.execute(&cleanup.plan(Selection {
        data: true,
        ..Default::default()
    })?)?;
    assert!(!report.complete);
    assert!(!report.errors.is_empty());
    assert!(cleanup.home.join("wukong.toml").is_file());
    assert!(
        !cleanup
            .home
            .join("updates/cleanup-in-progress.json")
            .exists()
    );
    Ok(())
}
#[test]
fn plans_are_read_only_and_unknown_files_survive_data_removal() -> Result<()> {
    let (temp, cleanup) = fixture()?;
    let state = cleanup.home.join(".wukong");
    let store = crate::store::Store::open(&state)?;
    fs::write(state.join("artifacts/owned"), b"recorded bytes")?;
    fs::write(state.join("personal.txt"), b"must remain")?;
    drop(store);
    let outside = temp.path().join("original.elf");
    fs::write(&outside, b"original")?;
    let before = fs::read(&outside)?;
    let plan = cleanup.plan(Selection {
        data: true,
        ..Default::default()
    })?;
    assert!(
        !cleanup.home.join("updates").exists(),
        "preview created maintenance state"
    );
    assert!(
        plan.files
            .iter()
            .any(|entry| entry.path.ends_with("artifacts/owned"))
    );
    assert!(
        !plan
            .files
            .iter()
            .any(|entry| entry.path.ends_with("personal.txt"))
    );
    let report = cleanup.execute(&plan)?;
    assert!(report.complete, "{:?}", report.errors);
    assert!(!state.join("state.sqlite3").exists());
    assert_eq!(fs::read(&outside)?, before);
    assert!(state.join("personal.txt").exists());
    assert!(!cleanup.home.join("wukong.toml").exists());
    Ok(())
}
#[test]
fn changed_inventory_and_active_maintenance_are_rejected_before_deleting() -> Result<()> {
    let (_temp, cleanup) = fixture()?;
    let plan = cleanup.plan(Selection {
        data: true,
        ..Default::default()
    })?;
    fs::write(cleanup.home.join("preferences.json"), b"{\"changed\":true}")?;
    assert!(cleanup.execute(&plan).is_err());
    assert!(cleanup.home.join("wukong.toml").exists());
    let plan = cleanup.plan(Selection {
        data: true,
        ..Default::default()
    })?;
    let _task = crate::updates::maintenance::lock(&cleanup.home.join("updates"), false)?;
    assert!(cleanup.execute(&plan).is_err());
    assert!(cleanup.home.join("preferences.json").exists());
    Ok(())
}
#[test]
fn relocated_cache_requires_ownership_and_keeps_unrelated_siblings() -> Result<()> {
    let (temp, cleanup) = fixture()?;
    let custom = temp.path().join("shared");
    fs::create_dir_all(custom.join("releases"))?;
    fs::write(custom.join("releases/unrelated"), b"foreign")?;
    let mut registry = Registry::default();
    registry.locations.insert(Location {
        kind: LocationKind::Cache,
        path: custom.clone(),
    });
    crate::workbench::write_json(&cleanup.home.join("managed-locations.json"), &registry)?;
    let plan = cleanup.plan(Selection {
        data: true,
        ..Default::default()
    })?;
    assert!(
        !plan
            .files
            .iter()
            .any(|entry| entry.path.starts_with(&custom))
    );
    let owned = temp.path().join("owned-cache");
    create_owned_directory(&owned)?;
    fs::create_dir_all(owned.join("releases"))?;
    fs::write(owned.join("releases/installer"), b"cache")?;
    fs::write(owned.join("personal.txt"), b"foreign")?;
    registry.locations.insert(Location {
        kind: LocationKind::Cache,
        path: owned.clone(),
    });
    crate::workbench::write_json(&cleanup.home.join("managed-locations.json"), &registry)?;
    let report = cleanup.execute(&cleanup.plan(Selection {
        data: true,
        ..Default::default()
    })?)?;
    assert!(report.complete);
    assert!(custom.join("releases/unrelated").exists());
    assert!(!owned.join("releases/installer").exists());
    assert!(owned.join("personal.txt").exists());
    Ok(())
}
#[cfg(unix)]
#[test]
fn links_cannot_escape_owned_directories() -> Result<()> {
    let (temp, cleanup) = fixture()?;
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside)?;
    fs::write(outside.join("keep"), b"outside")?;
    let state = cleanup.home.join(".wukong");
    fs::create_dir_all(state.join("artifacts"))?;
    std::os::unix::fs::symlink(&outside, state.join("artifacts/escape"))?;
    let plan = cleanup.plan(Selection {
        data: true,
        ..Default::default()
    })?;
    assert!(
        !plan
            .files
            .iter()
            .any(|entry| entry.path.starts_with(&outside))
    );
    assert!(cleanup.execute(&plan)?.complete);
    assert!(outside.join("keep").exists());
    Ok(())
}

#[test]
fn configuration_paths_are_not_permission_to_delete_shared_directory_contents() -> Result<()> {
    let (temp, cleanup) = fixture()?;
    let shared = temp.path().join("project");
    fs::create_dir_all(shared.join("projects"))?;
    fs::write(shared.join("projects/source.c"), b"user code")?;
    let mut registry = Registry::default();
    registry.locations.insert(Location {
        kind: LocationKind::Ghidra,
        path: shared.clone(),
    });
    registry.locations.insert(Location {
        kind: LocationKind::Config,
        path: shared.join("Cargo.toml"),
    });
    fs::write(shared.join("Cargo.toml"), b"[package]\nname='keep'")?;
    crate::workbench::write_json(&cleanup.home.join("managed-locations.json"), &registry)?;
    let plan = cleanup.plan(Selection {
        data: true,
        ..Default::default()
    })?;
    assert!(
        !plan
            .files
            .iter()
            .any(|entry| entry.path.starts_with(&shared))
    );
    assert!(
        plan.retained
            .iter()
            .any(|entry| entry.path == shared.display().to_string())
    );
    assert!(cleanup.execute(&plan)?.complete);
    assert!(shared.join("projects/source.c").exists());
    assert!(shared.join("Cargo.toml").exists());
    Ok(())
}
