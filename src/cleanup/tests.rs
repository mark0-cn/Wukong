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
