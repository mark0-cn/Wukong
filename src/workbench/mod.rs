//! Host-owned workbench lifecycle. Targets never receive the Docker socket.
pub mod challenge;
pub mod cli;
mod docker;
mod environment;

use crate::config::AppConfig;
use anyhow::{Context, Result, ensure};
pub use docker::{Docker, portable_path};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkbenchConfig {
    pub root: PathBuf,
    pub image: String,
    pub memory: String,
    pub cpus: u32,
}
impl Default for WorkbenchConfig {
    fn default() -> Self {
        Self {
            root: ".re-agent/workbench".into(),
            image: "re-agent-workbench:0.1".into(),
            memory: "4g".into(),
            cpus: 2,
        }
    }
}

pub struct Workbench {
    pub root: PathBuf,
    pub id: String,
    pub docker: Docker,
    pub config: AppConfig,
}
impl Workbench {
    pub fn open(config: &AppConfig) -> Result<Self> {
        fs::create_dir_all(&config.workbench.root)?;
        let root = portable_path(&fs::canonicalize(&config.workbench.root)?);
        let operation = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join("operation.lock"))?;
        operation.lock()?;
        let identity = root.join("workspace.json");
        if !identity.exists() {
            write_json(
                &identity,
                &json!({"schema_version":1,"id":uuid::Uuid::new_v4().simple().to_string()[..12]}),
            )?;
        }
        let value: Value = read_json(&identity)?;
        ensure!(
            value["schema_version"] == 1,
            "unsupported workspace version"
        );
        let id = value["id"]
            .as_str()
            .context("workspace id missing")?
            .to_owned();
        valid_id(&id)?;
        Ok(Self {
            root,
            id,
            docker: Docker::new(&config.runtime.docker_executable),
            config: config.clone(),
        })
    }
    pub fn lock(&self) -> Result<fs::File> {
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join("operation.lock"))?;
        file.try_lock()
            .context("another workbench operation is active")?;
        Ok(file)
    }
    pub fn name(&self, suffix: &str) -> String {
        format!("re-agent-{}-{suffix}", self.id)
    }
    pub fn owned(&self, object: &Value, resource: &str) -> Result<()> {
        let labels = object
            .get("Config")
            .and_then(|c| c.get("Labels"))
            .unwrap_or(&object["Labels"]);
        ensure!(
            labels["re-agent.workspace"].as_str() == Some(&self.id),
            "refusing to modify foreign Docker resource {resource}"
        );
        Ok(())
    }
}

pub fn valid_id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'),
        "id must contain 1-64 lowercase letters, digits, '_' or '-'"
    );
    Ok(())
}
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("read {}", path.display()))?)
        .context("invalid workbench JSON")
}
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    fs::create_dir_all(path.parent().context("file needs a parent")?)?;
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    use std::io::Write;
    temporary.write_all(&serde_json::to_vec_pretty(value)?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_identity_and_lock_are_stable() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut config = AppConfig::default();
        config.workbench.root = temp.path().join("workbench");
        let first = Workbench::open(&config)?;
        let second = Workbench::open(&config)?;
        assert_eq!(first.id, second.id);
        let lock = first.lock()?;
        assert!(second.lock().is_err());
        drop(lock);
        assert!(second.lock().is_ok());
        assert!(
            first
                .owned(
                    &json!({"Labels":{"re-agent.workspace":first.id}}),
                    "network"
                )
                .is_ok()
        );
        assert!(
            first
                .owned(
                    &json!({"Config":{"Labels":{"re-agent.workspace":"foreign"}}}),
                    "container"
                )
                .is_err()
        );
        Ok(())
    }
}
