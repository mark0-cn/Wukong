//! Reviewable, host-owned cleanup shared by CLI, desktop and native uninstallers.
mod admission;
pub mod cli;
mod containers;
mod files;
mod locations;
#[cfg(test)]
mod tests;
mod uninstall;
use anyhow::{Context, Result, ensure};
pub use containers::Resource;
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
    pub resources: Vec<Resource>,
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
    pub removed_resources: usize,
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
            resources: vec![],
            directories: vec![],
            retained: vec![],
            errors: vec![],
            bytes: 0,
            fingerprint: String::new(),
        };
        if (selection.tools || selection.challenges)
            && let Err(error) = containers::inventory(&self.home, &mut plan)
        {
            plan.errors.push(error.to_string());
        }
        if selection.data {
            files::inventory(&self.home, &mut plan)?;
            files::profiles(&mut plan)?;
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
            &plan.resources,
            &plan.directories,
        ))?));
        Ok(plan)
    }
    /// Rebuild the inventory under the existing cross-process maintenance lock.
    pub fn execute(&self, reviewed: &Plan) -> Result<Report> {
        self.execute_deferred(reviewed, false)
    }
    pub fn execute_deferred(&self, reviewed: &Plan, defer: bool) -> Result<Report> {
        ensure!(
            reviewed.schema_version == 1 && reviewed.home == self.home,
            "cleanup review belongs to another installation"
        );
        ensure!(reviewed.selection.any(), "no cleanup scope selected");
        files::safe(&self.home.join("updates/maintenance.lock"))?;
        files::safe(&self.home.join("updates/cache.lock"))?;
        let cache = crate::updates::storage::cache_lock_at(&self.home.join("updates"), true)?;
        let lock = crate::updates::maintenance::cleanup_lock(&self.home.join("updates"))?;
        ensure!(
            !self.home.join("updates/pending.json").exists(),
            "finish or recover the pending update before uninstalling"
        );
        let mut workbench_locks = vec![];
        for (root, _) in containers::workspaces(&self.home)? {
            files::safe(&root.join("operation.lock"))?;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(root.join("operation.lock"))?;
            file.try_lock()
                .context("another workbench operation is active")?;
            workbench_locks.push((root, file));
        }
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
        if reviewed.selection.data {
            admission::stop_workers(&self.home)?;
        }
        crate::workbench::write_json(
            &self.home.join("updates/cleanup-in-progress.json"),
            &reviewed.fingerprint,
        )?;
        let mut report = Report {
            complete: false,
            removed_files: 0,
            removed_resources: 0,
            removed_bytes: 0,
            retained: current.retained,
            errors: vec![],
        };
        containers::remove(&current.resources, &mut report);
        if report.errors.is_empty() {
            files::remove(&current.files, &mut report);
        }
        for (root, file) in workbench_locks {
            drop(file);
            if reviewed.selection.data
                && !root.join("workspace.json").exists()
                && files::safe(&root.join("operation.lock")).is_ok()
            {
                let _ = std::fs::remove_file(root.join("operation.lock"));
            }
        }
        drop(lock);
        drop(cache);
        // Only empty directories are removed; unknown/new content is never swept.
        for directory in current.directories.iter().rev() {
            let _ = std::fs::remove_dir(directory);
        }
        report.complete = report.errors.is_empty();
        if (!defer || !report.complete)
            && let Err(error) = self.finish(&reviewed.fingerprint)
        {
            report.errors.push(format!("cleanup finalization: {error}"));
            report.complete = false;
        }
        Ok(report)
    }
    pub fn finish(&self, fingerprint: &str) -> Result<()> {
        let marker = self.home.join("updates/cleanup-in-progress.json");
        files::safe(&marker)?;
        if !marker.exists() {
            return Ok(());
        }
        let cache = crate::updates::storage::cache_lock_at(&self.home.join("updates"), true)?;
        let lock = crate::updates::maintenance::cleanup_lock(&self.home.join("updates"))?;
        let identity: String = crate::workbench::read_json(&marker)?;
        ensure!(
            identity == fingerprint,
            "cleanup finalization belongs to another operation"
        );
        // Retire lock paths while admission is still closed. Removing an unlocked
        // lock file after admitting another process would split lock ownership.
        std::fs::remove_file(self.home.join("updates/cache.lock"))?;
        std::fs::remove_file(self.home.join("updates/maintenance.lock"))?;
        std::fs::remove_file(marker)?;
        drop(lock);
        drop(cache);
        let _ = std::fs::remove_dir(self.home.join("updates"));
        let _ = std::fs::remove_dir(&self.home);
        Ok(())
    }
}
pub fn check_output(plan: &Plan, output: &Path) -> Result<()> {
    let output = files::absolute(output)?;
    files::safe(&output)?;
    ensure!(
        !plan.directories.iter().any(|root| output.starts_with(root)),
        "review/report output must be outside selected cleanup directories"
    );
    Ok(())
}
pub fn read_plan(path: &Path) -> Result<Plan> {
    ensure!(
        std::fs::metadata(path)?.len() <= 64 * 1024 * 1024,
        "cleanup review is too large"
    );
    serde_json::from_slice(&std::fs::read(path)?).context("invalid cleanup review")
}
