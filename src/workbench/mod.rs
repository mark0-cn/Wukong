//! Host-owned workbench lifecycle. Targets never receive the Docker socket.
mod bundle;
pub mod challenge;
pub mod cli;
mod docker;
mod environment;
mod services;

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
            root: ".wukong/workbench".into(),
            image: "wukong:0.1".into(),
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
        format!("wukong-{}-{suffix}", self.id)
    }
    pub fn owned(&self, object: &Value, resource: &str) -> Result<()> {
        let labels = object
            .get("Config")
            .and_then(|c| c.get("Labels"))
            .unwrap_or(&object["Labels"]);
        ensure!(
            labels["wukong.workspace"].as_str() == Some(&self.id),
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
                .owned(&json!({"Labels":{"wukong.workspace":first.id}}), "network")
                .is_ok()
        );
        assert!(
            first
                .owned(
                    &json!({"Config":{"Labels":{"wukong.workspace":"foreign"}}}),
                    "container"
                )
                .is_err()
        );
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopRequest {
    Status,
    Up,
    Down,
    Doctor,
    List,
    Import {
        source: PathBuf,
        id: String,
        binary: Option<String>,
    },
    Prepare {
        id: String,
    },
    Build {
        id: String,
    },
    Start {
        id: String,
    },
    Stop {
        id: String,
    },
    Select {
        id: Option<String>,
    },
    Export {
        destination: PathBuf,
    },
}
impl Workbench {
    pub fn desktop(&self, request: DesktopRequest) -> Result<Value> {
        match request {
            DesktopRequest::Status => self.status(),
            DesktopRequest::Up => self.up(None),
            DesktopRequest::Down => self.down(),
            DesktopRequest::Doctor => self.doctor(),
            DesktopRequest::List => self.challenges(),
            DesktopRequest::Import { source, id, binary } => {
                self.import_challenge(&source, &id, binary.as_deref())
            }
            DesktopRequest::Prepare { id } => self.prepare_challenge(&id),
            DesktopRequest::Build { id } => self.build_challenge(&id),
            DesktopRequest::Start { id } => self.up_challenge(&id, false),
            DesktopRequest::Stop { id } => self.down_challenge(&id),
            DesktopRequest::Export { destination } => self.export_bundle(&destination),
            DesktopRequest::Select { .. } => {
                anyhow::bail!("selection requires the desktop context")
            }
        }
    }
}
