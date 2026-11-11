//! Exact file inventories; never recursively delete a configured root or follow reparse points.
use super::*;
use std::{collections::BTreeSet, fs, time::UNIX_EPOCH};
pub(super) fn absolute(path: &Path) -> Result<PathBuf> {
    let path = std::path::absolute(path)?;
    #[cfg(windows)]
    let path = crate::workbench::portable_path(&path);
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                ensure!(clean.pop(), "path escaped its root");
            }
            std::path::Component::CurDir => {}
            other => clean.push(other),
        }
    }
    ensure!(clean.parent().is_some(), "refusing a filesystem root");
    // Resolve existing, link-free ancestors so Win32 drive/verbatim/8.3/case
    // aliases cannot inventory (or lock) the same physical directory twice.
    // Keep reparse paths lexical: cleanup must retain them rather than adopt
    // their targets. Normal application startup can still use configured links.
    if safe(&clean).is_ok() {
        let mut ancestor = clean.as_path();
        let mut suffix = vec![];
        while !ancestor.try_exists()? {
            suffix.push(
                ancestor
                    .file_name()
                    .context("path has no existing ancestor")?
                    .to_owned(),
            );
            ancestor = ancestor.parent().context("path has no parent")?;
        }
        let mut resolved = crate::workbench::portable_path(&fs::canonicalize(ancestor)?);
        for part in suffix.into_iter().rev() {
            resolved.push(part);
        }
        clean = resolved;
    }
    Ok(clean)
}
fn reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
pub(super) fn safe(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        // A verbatim drive prefix alone (\\?\C:) is not a queryable Win32 path.
        if matches!(component, std::path::Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => ensure!(
                !reparse(&metadata),
                "refusing link/reparse path {}",
                current.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn identity(path: &Path, record: bool) -> Result<FileEntry> {
    safe(path)?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(metadata.is_file(), "not a regular file");
    Ok(FileEntry {
        path: crate::workbench::portable_path(&fs::canonicalize(path)?),
        bytes: metadata.len(),
        modified: metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|time| time.as_nanos()),
        record,
    })
}
fn add(path: &Path, record: bool, plan: &mut Plan) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    if let Err(error) = safe(path) {
        plan.retained.push(Retained {
            path: path.display().to_string(),
            reason: error.to_string(),
        });
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_file() {
        plan.files.push(identity(path, record)?);
    } else if metadata.is_dir() {
        plan.directories.push(path.to_owned());
        let mut children = fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        children.sort();
        for child in children {
            ensure!(
                plan.files.len() < 200_000,
                "cleanup inventory exceeds 200000 files"
            );
            add(&child, record, plan)?;
        }
    } else {
        plan.retained.push(Retained {
            path: path.display().to_string(),
            reason: "not a regular file or directory".into(),
        });
    }
    Ok(())
}
pub(super) fn inventory(home: &Path, plan: &mut Plan) -> Result<()> {
    safe(home)?;
    let registry = locations::inventory(home)?;
    let keep_environments = !plan.selection.tools
        || !plan.selection.challenges
        || plan.retained.iter().any(|item| {
            matches!(
                item.reason.as_str(),
                "referenced by a retained container"
                    | "unverified or shared image ownership"
                    | "Docker scope not selected"
                    | "unrecognized workspace resource name; retained"
            )
        });
    let mut recognized: BTreeSet<_> = registry
        .locations
        .iter()
        .map(|location| location.path.clone())
        .collect();
    for location in registry.locations {
        let path = location.path;
        if !path.try_exists()? {
            continue;
        }
        if let Err(error) = safe(&path) {
            plan.retained.push(Retained {
                path: path.display().to_string(),
                reason: error.to_string(),
            });
            continue;
        }
        if location.kind == LocationKind::Config {
            // Explicit configuration files are settings, not ownership of their parent.
            let owned = path.starts_with(home)
                || fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
                    .is_some_and(|value| {
                        value.get("state_dir").is_some()
                            && value.get("model").is_some()
                            && value.get("runtime").is_some()
                    });
            if !owned {
                plan.retained.push(Retained {
                    path: path.display().to_string(),
                    reason: "unverified configuration ownership; retained".into(),
                });
                continue;
            }
            add(&path, true, plan)?;
            recognized.insert(path);
            continue;
        }
        let names: &[&str] = match location.kind {
            LocationKind::State => &[
                "state.sqlite3",
                "state.sqlite3-wal",
                "state.sqlite3-shm",
                "artifacts",
                "rig",
            ],
            LocationKind::Worker => &[
                "jobs",
                "debug",
                "interact",
                "files",
                "cache",
                "home",
                "logs",
                "requests",
                "endpoint.json",
                "startup.lock",
                "operations",
                "bridge.log",
                "daemon.log",
            ],
            LocationKind::Ghidra => &["scripts", "projects", "results"],
            LocationKind::Workbench => &[
                "workspace.json",
                "environment.json",
                "previous-environment.json",
                "wukong.toml",
                "doctor.json",
                "import-record.json",
                "logs",
                "challenges",
            ],
            LocationKind::Cache => &[
                "releases",
                "probes",
                "Wukong-Recovery.exe",
                "Wukong-Recovery.AppImage",
                "pending.json",
                "previous.json",
                "cache-location.json",
                "cache.lock",
            ],
            LocationKind::Config => unreachable!(),
        };
        // Custom roots must carry recognizable Wukong metadata. Defaults live in
        // the dedicated application namespace; unknown siblings remain untouched.
        let known = path.starts_with(home)
            || locations::owns_directory(&path)
            || match location.kind {
                LocationKind::State => {
                    path.join("state.sqlite3").is_file()
                        && valid_database(&path.join("state.sqlite3"))
                }
                LocationKind::Workbench => {
                    safe(&path.join("workspace.json")).is_ok()
                        && path.join("workspace.json").is_file()
                        && crate::workbench::read_json::<serde_json::Value>(
                            &path.join("workspace.json"),
                        )
                        .is_ok_and(|value| {
                            value["schema_version"] == 1
                                && value["id"]
                                    .as_str()
                                    .is_some_and(|id| crate::workbench::valid_id(id).is_ok())
                        })
                }
                LocationKind::Ghidra | LocationKind::Cache | LocationKind::Worker => false,
                LocationKind::Config => false,
            };
        if !known {
            plan.retained.push(Retained {
                path: path.display().to_string(),
                reason: "unverified directory ownership; retained".into(),
            });
            continue;
        }
        plan.directories.push(path.clone());
        for name in names {
            let child = path.join(name);
            if child == home.join("updates/cache.lock") {
                recognized.insert(child);
                continue;
            }
            if (location.kind == LocationKind::Cache && *name == "probes" && keep_environments)
                || (location.kind == LocationKind::Workbench
                    && keep_environments
                    && matches!(
                        *name,
                        "workspace.json"
                            | "environment.json"
                            | "previous-environment.json"
                            | "challenges"
                    ))
            {
                if child.exists() {
                    plan.retained.push(Retained {
                        path: child.display().to_string(),
                        reason: "ownership/launch records for retained Docker environments".into(),
                    });
                }
                recognized.insert(child);
                continue;
            }
            let record = matches!(
                *name,
                "state.sqlite3"
                    | "state.sqlite3-wal"
                    | "state.sqlite3-shm"
                    | "workspace.json"
                    | "environment.json"
                    | "previous-environment.json"
                    | "wukong.toml"
                    | "pending.json"
                    | "previous.json"
                    | "cache-location.json"
            );
            if let Err(error) = add(&child, record, plan) {
                plan.errors.push(format!("{}: {error}", child.display()));
            }
            recognized.insert(child);
        }
        let marker = path.join(".wukong-owned.json");
        if locations::owns_directory(&path) {
            add(&marker, true, plan)?;
            recognized.insert(marker);
        }
        for entry in fs::read_dir(&path)? {
            let entry = entry?.path();
            if !recognized.contains(&entry) && !registry_child(&entry, home) {
                plan.retained.push(Retained {
                    path: entry.display().to_string(),
                    reason: "unrecognized content; retained".into(),
                });
            }
        }
    }
    for name in [
        "preferences.json",
        "managed-locations.json",
        "locations.lock",
    ] {
        if name == "managed-locations.json"
            && keep_environments
            && !containers::workspaces(home)?.is_empty()
        {
            plan.retained.push(Retained {
                path: home.join(name).display().to_string(),
                reason: "location records for retained Docker environments".into(),
            });
            continue;
        }
        add(&home.join(name), true, plan)?;
    }
    plan.directories.push(home.to_owned());
    plan.directories.push(home.join("updates"));
    Ok(())
}
fn registry_child(path: &Path, home: &Path) -> bool {
    path == home.join("updates/maintenance.lock")
        || path == home.join("updates/cleanup-in-progress.json")
}
pub(super) fn profiles(plan: &mut Plan) -> Result<()> {
    if Cleanup::current()?.home != plan.home {
        return Ok(());
    }
    let mut bases = vec![];
    #[cfg(windows)]
    for name in ["LOCALAPPDATA", "APPDATA"] {
        if let Some(path) = std::env::var_os(name) {
            bases.push(PathBuf::from(path));
        }
    }
    #[cfg(not(windows))]
    for (name, fallback) in [
        ("XDG_DATA_HOME", ".local/share"),
        ("XDG_CACHE_HOME", ".cache"),
        ("XDG_CONFIG_HOME", ".config"),
    ] {
        if let Some(path) = std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(fallback)))
        {
            bases.push(path);
        }
    }
    for base in bases {
        // Framework data only, never the shared WebView2 installation.
        add(&absolute(&base.join("io.wukong.desktop"))?, false, plan)?;
    }
    Ok(())
}
pub(super) fn record_order(file: &FileEntry) -> u8 {
    if !file.record {
        return 0;
    }
    match file.path.file_name().and_then(|name| name.to_str()) {
        Some("managed-locations.json") => 4,
        Some("wukong.toml") => 3,
        Some(".wukong-owned.json") => 2,
        _ => 1,
    }
}
fn valid_database(path: &Path) -> bool {
    let Ok(mut uri) = reqwest::Url::from_file_path(path) else {
        return false;
    };
    uri.query_pairs_mut().append_pair("immutable", "1");
    safe(path).is_ok()
        && rusqlite::Connection::open_with_flags(
            uri.as_str(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .is_ok_and(|connection| {
            connection
                .query_row(
                    "SELECT value FROM meta WHERE key='schema_version'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .is_ok_and(|schema| schema == "3")
                && connection
                    .prepare("SELECT id,run_id,data FROM entities LIMIT 0")
                    .is_ok()
        })
}
pub(super) fn remove(files: &[FileEntry], report: &mut Report) {
    for file in files {
        if file.record && !report.errors.is_empty() {
            continue;
        }
        let result = (|| -> Result<()> {
            ensure!(
                identity(&file.path, file.record)? == *file,
                "file changed after cleanup review"
            );
            fs::remove_file(&file.path)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                report.removed_files += 1;
                report.removed_bytes += file.bytes;
            }
            Err(error) => report
                .errors
                .push(format!("{}: {error}", file.path.display())),
        }
    }
}
