//! Offline folder bundle: images, named-volume snapshots and challenge attachments.
use super::challenge::{copy_tree, inventory};
use super::environment::Environment;
use super::*;
use std::collections::BTreeSet;

impl Workbench {
    fn managed_containers(&self) -> Result<Vec<String>> {
        let value = self.docker.run(&[
            "ps",
            "-a",
            "--filter",
            &format!("label=re-agent.workspace={}", self.id),
            "--format",
            "{{.Names}}",
        ])?;
        Ok(value
            .lines()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect())
    }
    pub fn export_bundle(&self, destination: &Path) -> Result<Value> {
        ensure!(!destination.exists(), "bundle destination already exists");
        self.require_idle()?;
        let containers = self.managed_containers()?;
        let mut running = vec![];
        let mut namespace_dependents = BTreeSet::new();
        for name in &containers {
            let container = self
                .docker
                .inspect("container", name)?
                .context("container disappeared")?;
            self.owned(&container, name)?;
            if container["State"]["Running"] == true {
                if container["Config"]["Labels"]["re-agent.kind"] == "tools" {
                    let probe: Value = serde_json::from_str(&self.docker.run(&[
                        "exec",
                        name,
                        "python3",
                        "/opt/re-agent/runtime/workbench_control.py",
                        "idle",
                        "/state",
                    ])?)?;
                    ensure!(
                        probe["idle"] == true,
                        "close active work in {name} before exporting"
                    );
                }
                if container["HostConfig"]["PidMode"]
                    .as_str()
                    .is_some_and(|p| p.starts_with("container:"))
                {
                    namespace_dependents.insert(name.clone());
                }
                running.push(name.clone());
            }
        }
        // A debug companion cannot start before its service's PID namespace exists.
        running.sort_by_key(|name| namespace_dependents.contains(name));
        // Stop before snapshotting; even an idle bridge can mutate journals.
        for name in running.iter().rev() {
            self.docker.run(&["stop", name])?;
        }
        let result = self.export_stopped(destination);
        let mut restart_errors = vec![];
        for name in running {
            if let Err(error) = self.docker.run(&["start", &name]) {
                restart_errors.push(format!("{name}: {error}"));
            }
        }
        let mut result = result?;
        result["restart_errors"] = json!(restart_errors);
        Ok(result)
    }
    fn export_stopped(&self, destination: &Path) -> Result<Value> {
        let env = self.environment()?;
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let temporary = tempfile::tempdir_in(parent)?;
        let output = portable_path(&fs::canonicalize(temporary.path())?);
        ensure!(
            !output.starts_with(&self.root),
            "bundle output must be outside workbench"
        );
        let mut images = BTreeSet::from([env.image_id.clone()]);
        let mut ids = vec![];
        if self.root.join("challenges").exists() {
            for entry in fs::read_dir(self.root.join("challenges"))? {
                let entry = entry?;
                if !entry.path().join("challenge.toml").is_file() {
                    continue;
                }
                let id = entry.file_name().to_string_lossy().into_owned();
                self.verify_original(&id)?;
                let source = entry.path();
                let dest = output.join("challenges").join(&id);
                fs::create_dir_all(&dest)?;
                // Generated host config and model credentials are intentionally absent.
                for name in ["original", "prepared", "scripts", "results"] {
                    let path = source.join(name);
                    if path.exists() {
                        inventory(&path)?;
                        copy_tree(&path, &dest.join(name))?;
                    }
                }
                for name in ["challenge.toml", "environment.lock.json", "launch.json"] {
                    if source.join(name).is_file() {
                        fs::copy(source.join(name), dest.join(name))?;
                    }
                }
                let lock: Value = read_json(&source.join("environment.lock.json"))?;
                if let Some(image) = lock["challenge_image_id"].as_str() {
                    images.insert(image.into());
                }
                ids.push(id);
            }
        }
        let mut args = vec![
            "image".into(),
            "save".into(),
            "-o".into(),
            output.join("images.tar").to_string_lossy().into_owned(),
        ];
        args.extend(images.iter().cloned());
        self.docker.logged(
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            &output.join("image-export.log"),
        )?;
        let volumes = self.docker.run(&[
            "volume",
            "ls",
            "--filter",
            &format!("label=re-agent.workspace={}", self.id),
            "--format",
            "{{.Name}}",
        ])?;
        let mut suffixes = vec![];
        fs::create_dir_all(output.join("volumes"))?;
        for volume in volumes.lines().filter(|s| !s.is_empty()) {
            let inspect = self
                .docker
                .inspect("volume", volume)?
                .context("volume disappeared")?;
            self.owned(&inspect, volume)?;
            let prefix = format!("re-agent-{}-", self.id);
            let suffix = volume
                .strip_prefix(&prefix)
                .context("unexpected managed volume name")?;
            valid_id(suffix)?;
            self.archive_volume(
                "export",
                volume,
                &output.join("volumes"),
                suffix,
                &env.image_id,
            )?;
            suffixes.push(suffix.to_owned());
        }
        let history = self.snapshot_history(&output.join("history"))?;
        let files = inventory(&output)?;
        let manifest = json!({"schema_version":1,"platform":"linux/amd64","tool_image_id":env.image_id,"image_ids":images,"challenge_ids":ids,"volumes":suffixes,"files":files,"history":history,"source_workspace":self.root,"source_id":self.id});
        write_json(&output.join("bundle.json"), &manifest)?;
        fs::rename(temporary.keep(), destination)?;
        Ok(
            json!({"bundle":destination,"challenge_count":ids.len(),"image_count":images.len(),"credential_stores_included":false,"history":"read-only archive; start fresh executions after import"}),
        )
    }
    fn snapshot_history(&self, destination: &Path) -> Result<bool> {
        let source = self.config.state_dir.join("state.sqlite3");
        if !source.is_file() {
            return Ok(false);
        }
        let connection = rusqlite::Connection::open(&source)?;
        connection.execute_batch("BEGIN IMMEDIATE")?;
        let active: i64 = connection.query_row(
            "SELECT COUNT(*) FROM leases WHERE expires_at > unixepoch()",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            active == 0,
            "assessment is active; pause it before exporting"
        );
        fs::create_dir_all(destination)?;
        let reader = rusqlite::Connection::open_with_flags(
            &source,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        reader.backup(
            rusqlite::DatabaseName::Main,
            destination.join("state.sqlite3"),
            None,
        )?;
        let artifacts = self.config.state_dir.join("artifacts");
        if artifacts.exists() {
            inventory(&artifacts)?;
            copy_tree(&artifacts, &destination.join("artifacts"))?;
        }
        connection.execute_batch("ROLLBACK")?;
        Ok(true)
    }
    fn archive_volume(
        &self,
        action: &str,
        volume: &str,
        directory: &Path,
        suffix: &str,
        image: &str,
    ) -> Result<()> {
        let directory = portable_path(&fs::canonicalize(directory)?);
        ensure!(
            !directory.to_string_lossy().contains(','),
            "bundle directory cannot contain comma (Docker mount syntax)"
        );
        self.docker.logged(
            &[
                "run",
                "--rm",
                "--network",
                "none",
                "--read-only",
                "--cap-drop",
                "ALL",
                "--cap-add",
                "DAC_OVERRIDE",
                "--cap-add",
                "CHOWN",
                "--cap-add",
                "FOWNER",
                "--security-opt",
                "no-new-privileges",
                "--mount",
                &format!(
                    "type=volume,source={volume},target=/snapshot{}",
                    if action == "export" { ",readonly" } else { "" }
                ),
                "--mount",
                &format!(
                    "type=bind,source={},target=/bundle{}",
                    directory.display(),
                    if action == "restore" { ",readonly" } else { "" }
                ),
                "--entrypoint",
                "python3",
                image,
                "/opt/re-agent/runtime/workbench_archive.py",
                action,
                "/snapshot",
                &format!("/bundle/{suffix}.tar"),
            ],
            &self.root.join(format!("logs/{action}-{suffix}.log")),
        )
    }
    pub fn import_bundle(&self, source: &Path) -> Result<Value> {
        ensure!(
            !self.root.join("environment.json").exists() && !self.root.join("challenges").exists(),
            "import requires an empty workbench root"
        );
        let source = portable_path(&fs::canonicalize(source)?);
        ensure!(
            !source.starts_with(&self.root) && !self.root.starts_with(&source),
            "bundle and workspace cannot contain one another"
        );
        let manifest: Value = read_json(&source.join("bundle.json"))?;
        ensure!(
            manifest["schema_version"] == 1 && manifest["platform"] == "linux/amd64",
            "unsupported bundle version/platform"
        );
        let mut actual = inventory(&source)?;
        actual.as_object_mut().unwrap().remove("bundle.json");
        ensure!(
            actual == manifest["files"],
            "bundle hash/inventory mismatch"
        );
        let image = manifest["tool_image_id"]
            .as_str()
            .context("tool image ID missing")?;
        ensure!(
            image.starts_with("sha256:") && image.len() == 71,
            "invalid tool image ID"
        );
        let suffixes: Vec<String> = serde_json::from_value(manifest["volumes"].clone())?;
        for suffix in &suffixes {
            valid_id(suffix)?;
            ensure!(
                self.docker.inspect("volume", &self.name(suffix))?.is_none(),
                "import volume already exists"
            );
        }
        let images: Vec<String> = serde_json::from_value(manifest["image_ids"].clone())?;
        ensure!(
            images.iter().any(|id| id == image),
            "tool image absent from bundle image list"
        );
        let mut missing = false;
        for id in &images {
            ensure!(
                id.starts_with("sha256:") && id.len() == 71,
                "invalid bundled image ID"
            );
            missing |= self.docker.inspect("image", id)?.is_none();
        }
        if missing {
            self.docker.logged(
                &[
                    "image",
                    "load",
                    "-i",
                    &source.join("images.tar").to_string_lossy(),
                ],
                &self.root.join("logs/image-import.log"),
            )?;
        }
        for id in &images {
            ensure!(
                self.docker.inspect("image", id)?.is_some(),
                "bundle did not load declared image {id}"
            );
        }
        for suffix in &suffixes {
            let volume = self.name(suffix);
            self.docker.run(&[
                "volume",
                "create",
                "--label",
                &format!("re-agent.workspace={}", self.id),
                &volume,
            ])?;
            self.archive_volume("restore", &volume, &source.join("volumes"), suffix, image)?;
        }
        for name in ["challenges", "history"] {
            if source.join(name).exists() {
                copy_tree(&source.join(name), &self.root.join(name))?;
            }
        }
        // Preserve historical evidence verbatim. It is not resumed under stale absolute paths.
        write_json(
            &self.root.join("import-record.json"),
            &json!({"source_workspace":manifest["source_workspace"],"source_id":manifest["source_id"],"history_read_only":true,"new_workspace":self.root}),
        )?;
        let result = self.up(Some(image))?;
        for id in manifest["challenge_ids"]
            .as_array()
            .context("challenge IDs missing")?
        {
            let id = id.as_str().context("invalid challenge ID")?;
            self.challenge(id)?;
            let env = Environment {
                image: image.into(),
                image_id: image.into(),
                container: self.name(&format!("{id}-tools")),
                volume: self.name(&format!("{id}-state")),
                network: self.name(&format!("{id}-net")),
            };
            self.write_worker_config(&env, &self.challenge_dir(id)?.join("re-agent.toml"))?;
        }
        Ok(
            json!({"environment":result,"imported":true,"history_read_only":true,"credentials_required":true}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_snapshot_copies_wal_and_refuses_active_leases() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut config = AppConfig {
            state_dir: temp.path().join("state"),
            ..AppConfig::default()
        };
        config.workbench.root = temp.path().join("workbench");
        let _store = crate::store::Store::open(&config.state_dir)?;
        let source = rusqlite::Connection::open(config.state_dir.join("state.sqlite3"))?;
        source.execute(
            "INSERT INTO events(run_id,kind,data,created_at) VALUES('test','snapshot','{}','now')",
            [],
        )?;
        let workbench = Workbench::open(&config)?;
        let dest = temp.path().join("archive");
        assert!(workbench.snapshot_history(&dest)?);
        let snapshot = rusqlite::Connection::open(dest.join("state.sqlite3"))?;
        assert_eq!(
            snapshot.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))?,
            1
        );
        source.execute(
            "INSERT INTO leases VALUES('test','worker',unixepoch()+60)",
            [],
        )?;
        assert!(
            workbench
                .snapshot_history(&temp.path().join("blocked"))
                .is_err()
        );
        Ok(())
    }
}
