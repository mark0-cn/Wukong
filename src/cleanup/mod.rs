//! Reviewable, host-owned cleanup shared by CLI, desktop and native uninstallers.
pub mod cli;
mod files;
mod locations;
#[cfg(test)]
mod tests;
use anyhow::{Context, Result, ensure};
pub(crate) use locations::register_configuration_locked;
pub use locations::{
    Location, LocationKind, Registry, create_owned_directory, register_cache,
    register_configuration,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub data: bool,
    pub tools: bool,
    pub challenges: bool,
}
impl Selection {
    pub fn any(self) -> bool {
        self.data || self.tools || self.challenges
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub bytes: u64,
    pub modified: Option<u128>,
    pub record: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Retained {
    pub path: String,
    pub reason: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Plan {
    pub schema_version: u32,
    pub selection: Selection,
    pub home: PathBuf,
    pub files: Vec<FileEntry>,
    pub directories: Vec<PathBuf>,
    pub retained: Vec<Retained>,
    pub errors: Vec<String>,
    pub bytes: u64,
    pub fingerprint: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Report {
    pub complete: bool,
    pub removed_files: usize,
    pub removed_bytes: u64,
    pub retained: Vec<Retained>,
    pub errors: Vec<String>,
}
pub struct Cleanup {
    pub home: PathBuf,
}
impl Cleanup {
    pub fn current() -> Result<Self> {
        Self::at(crate::desktop::paths::home()?)
    }
    pub fn at(home: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            home: files::absolute(home.as_ref())?,
        })
    }
    /// Read-only: no Store, worker, model or directory is initialized.
    pub fn plan(&self, selection: Selection) -> Result<Plan> {
        let mut plan = Plan {
            schema_version: 1,
            selection,
            home: self.home.clone(),
            files: vec![],
            directories: vec![],
            retained: vec![],
            errors: vec![],
            bytes: 0,
            fingerprint: String::new(),
        };
        if selection.tools || selection.challenges {
            plan.errors
                .push("Docker cleanup is not available in this build".into());
        }
        if selection.data {
            files::inventory(&self.home, &mut plan)?;
        }
        let mut files = std::collections::BTreeMap::<PathBuf, FileEntry>::new();
        for file in plan.files {
            files
                .entry(file.path.clone())
                .and_modify(|entry| entry.record |= file.record)
                .or_insert(file);
        }
        plan.files = files.into_values().collect();
        plan.files.sort_by(|a, b| {
            files::record_order(a)
                .cmp(&files::record_order(b))
                .then(a.path.cmp(&b.path))
        });
        plan.directories.sort();
        plan.directories.dedup();
        plan.bytes = plan.files.iter().map(|file| file.bytes).sum();
        plan.fingerprint = hex::encode(Sha256::digest(serde_json::to_vec(&(
            &plan.selection,
            &plan.home,
            &plan.files,
            &plan.directories,
        ))?));
        Ok(plan)
    }
    /// Rebuild the inventory under the existing cross-process maintenance lock.
    pub fn execute(&self, reviewed: &Plan) -> Result<Report> {
        ensure!(
            reviewed.schema_version == 1 && reviewed.home == self.home,
            "cleanup review belongs to another installation"
        );
        ensure!(reviewed.selection.any(), "no cleanup scope selected");
        files::safe(&self.home.join("updates/maintenance.lock"))?;
        let lock = crate::updates::maintenance::lock(&self.home.join("updates"), true)?;
        let current = self.plan(reviewed.selection)?;
        ensure!(
            current.errors.is_empty(),
            "cleanup inspection failed: {}",
            current.errors.join("; ")
        );
        ensure!(
            current.fingerprint == reviewed.fingerprint,
            "cleanup contents changed; review a fresh plan"
        );
        let mut report = Report {
            complete: false,
            removed_files: 0,
            removed_bytes: 0,
            retained: current.retained,
            errors: vec![],
        };
        files::remove(&current.files, &mut report);
        drop(lock);
        if reviewed.selection.data && report.errors.is_empty() {
            let path = self.home.join("updates/maintenance.lock");
            if files::safe(&path).is_ok() {
                let _ = std::fs::remove_file(path);
            }
        }
        // Only empty directories are removed; unknown/new content is never swept.
        for directory in current.directories.iter().rev() {
            let _ = std::fs::remove_dir(directory);
        }
        report.complete = report.errors.is_empty();
        Ok(report)
    }
}
pub fn read_plan(path: &Path) -> Result<Plan> {
    ensure!(
        std::fs::metadata(path)?.len() <= 64 * 1024 * 1024,
        "cleanup review is too large"
    );
    serde_json::from_slice(&std::fs::read(path)?).context("invalid cleanup review")
}
