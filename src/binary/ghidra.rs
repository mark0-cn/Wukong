//! Ghidra protocol, content-addressed caches, locks and process lifetime.
use super::*;
use std::{io::Write, process::Stdio, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhidraConfig {
    pub executable: PathBuf,
    pub cache_dir: PathBuf,
    pub script_dir: Option<PathBuf>,
    pub timeout_seconds: u64,
    pub analysis_timeout_seconds: u64,
    pub max_cpu: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum GhidraQuery {
    Functions {
        pattern: Option<String>,
        offset: usize,
        limit: usize,
    },
    /// Module-relative offset, not a runtime VA or file offset.
    Decompile { address: u64 },
    Instructions {
        address: u64,
        offset: usize,
        limit: usize,
    },
    Xrefs {
        address: u64,
        direction: XrefDirection,
        limit: usize,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XrefDirection {
    To,
    From,
    Both,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhidraResult {
    pub cache_key: String,
    pub target_sha256: String,
    pub tool_version: String,
    pub cached: bool,
    pub data: serde_json::Value,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}
pub struct GhidraBackend {
    config: GhidraConfig,
}
impl GhidraBackend {
    pub fn new(config: GhidraConfig) -> Result<Self> {
        if config.timeout_seconds == 0
            || config.analysis_timeout_seconds == 0
            || config.max_cpu == 0
        {
            bail!("Ghidra timeouts and max_cpu must be positive");
        }
        if !config.executable.is_file() {
            bail!(
                "Ghidra executable does not exist: {}",
                config.executable.display()
            );
        }
        Ok(Self { config })
    }
    pub async fn query(
        &self,
        target: impl AsRef<Path>,
        query: &GhidraQuery,
    ) -> Result<GhidraResult> {
        match query {
            GhidraQuery::Functions { limit, .. }
            | GhidraQuery::Xrefs { limit, .. }
            | GhidraQuery::Instructions { limit, .. }
                if *limit == 0 || *limit > 5000 =>
            {
                bail!("Ghidra query limit must be 1..5000")
            }
            _ => {}
        }
        let target = fs::canonicalize(target.as_ref())?;
        let hash = sha256_file(&target)?;
        let executable = fs::canonicalize(&self.config.executable)?;
        let props = executable
            .parent()
            .and_then(Path::parent)
            .map(|p| p.join("Ghidra/application.properties"));
        let properties = props
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .unwrap_or_default();
        let version = properties
            .lines()
            .find_map(|l| l.strip_prefix("application.version="))
            .unwrap_or("unknown")
            .trim()
            .to_owned();
        let (script_bytes, script_dir) = if let Some(path) = &self.config.script_dir {
            (
                fs::read(path.join("WukongBridge.java"))
                    .context("read configured Ghidra bridge script")?,
                fs::canonicalize(path)?,
            )
        } else {
            let bytes = include_bytes!("../../workers/ghidra/WukongBridge.java").to_vec();
            // A versioned directory prevents concurrent processes overwriting a running bridge.
            let dir = self
                .config
                .cache_dir
                .join("scripts")
                .join(sha256_bytes(&bytes));
            fs::create_dir_all(&dir)?;
            let script = dir.join("WukongBridge.java");
            if !script.is_file() {
                fs::write(&script, &bytes)?;
            }
            (bytes, fs::canonicalize(dir)?)
        };
        let analysis_binding = serde_json::json!({"schema":1,"target_sha256":hash,"tool_version":version,"tool_properties":sha256_bytes(properties.as_bytes()),"launcher":sha256_file(&executable)?,"bridge":sha256_bytes(&script_bytes),"analysis_timeout_seconds":self.config.analysis_timeout_seconds,"max_cpu":self.config.max_cpu});
        let analysis_key = sha256_bytes(&serde_json::to_vec(&analysis_binding)?);
        let binding = serde_json::json!({"analysis":analysis_binding,"query":query});
        let key = sha256_bytes(&serde_json::to_vec(&binding)?);
        crate::cleanup::create_owned_directory(&self.config.cache_dir)?;
        let root = self.config.cache_dir.join("results").join(&key);
        fs::create_dir_all(&root)?;
        let root = fs::canonicalize(root)?;
        let cache = root.join("result.json");
        let cached = |root: &Path| -> Option<Result<GhidraResult>> {
            let cache = root.join("result.json");
            if !cache.is_file() {
                return None;
            }
            Some((|| {
                let mut result: GhidraResult = serde_json::from_slice(&fs::read(cache)?)
                    .context("invalid Ghidra cache; remove entry and retry")?;
                if result.target_sha256 != hash || result.cache_key != key {
                    bail!("Ghidra cache identity mismatch");
                }
                result.cached = true;
                Ok(result)
            })())
        };
        if let Some(result) = cached(&root) {
            return result;
        }
        // Keep a single imported program per target/config/tool identity. Queries
        // share this project while a lock guards Ghidra's single-writer database.
        let project = self.config.cache_dir.join("projects").join(&analysis_key);
        fs::create_dir_all(&project)?;
        let project = fs::canonicalize(project)?;
        // A holder keeps the lock for the whole Ghidra run, so waiting must
        // span the configured query budget. Concurrent identical queries are
        // served from the cache the moment the holder publishes its result,
        // which usually ends the wait long before the deadline.
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(self.config.timeout_seconds);
        let lock = loop {
            match ProjectLock::try_acquire(&project) {
                Ok(lock) => break lock,
                Err(std::fs::TryLockError::WouldBlock) => {
                    if let Some(result) = cached(&root) {
                        return result;
                    }
                    if tokio::time::Instant::now() >= deadline {
                        bail!("Ghidra project is busy; the analyzer still owns worker.lock");
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        };
        // The lock can be won between the last poll and the successful open;
        // recheck once so a just-published result is not recomputed.
        if let Some(result) = cached(&root) {
            return result;
        }
        lock.require_idle()?;
        let ready = project.join("analysis-complete.json");
        let snapshot = project.join("target.bin");
        if !ready.exists() {
            fs::copy(&target, &snapshot)?;
            if sha256_file(&snapshot)? != hash {
                bail!("target changed while creating Ghidra snapshot");
            }
        }
        let work = root.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&work)?;
        let request = work.join("request.json");
        let output = work.join("response.json");
        fs::write(
            &request,
            serde_json::to_vec(&serde_json::json!({"query":query,"target_sha256":hash}))?,
        )?;
        let stdout_path = work.join("stdout.log");
        let stderr_path = work.join("stderr.log");
        fs::write(
            work.join("binding.json"),
            serde_json::to_vec_pretty(&binding)?,
        )?;
        let mut cmd = tokio::process::Command::new(&executable);
        cmd.arg(&project).arg("analysis");
        if ready.is_file() {
            cmd.arg("-process").arg("target.bin").arg("-noanalysis");
        } else {
            cmd.arg("-import").arg(&snapshot).arg("-overwrite");
        }
        cmd.arg("-analysisTimeoutPerFile")
            .arg(self.config.analysis_timeout_seconds.to_string())
            .arg("-max-cpu")
            .arg(self.config.max_cpu.to_string())
            .arg("-scriptPath")
            .arg(&script_dir)
            .arg("-postScript")
            .arg("WukongBridge.java")
            .arg(&request)
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout_path)?)
            .stderr(fs::File::create(&stderr_path)?)
            .kill_on_drop(false);
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000);
        }
        let mut killer = TreeKill::spawn_locked(cmd, lock)
            .with_context(|| format!("launch Ghidra {}", executable.display()))?;
        let status = match tokio::time::timeout(
            Duration::from_secs(self.config.timeout_seconds),
            killer
                .child
                .as_mut()
                .context("Ghidra child missing")?
                .wait(),
        )
        .await
        {
            Ok(status) => status?,
            Err(_) => {
                killer.terminate().await;
                bail!(
                    "Ghidra timed out after {} seconds; logs: {} and {}",
                    self.config.timeout_seconds,
                    stdout_path.display(),
                    stderr_path.display()
                );
            }
        };
        // Finish cancellation/reaping while the project lease is still held.
        killer.terminate().await;
        if !status.success() {
            bail!(
                "Ghidra exited {status}; logs: {} and {}",
                stdout_path.display(),
                stderr_path.display()
            );
        }
        let response = fs::read(&output).with_context(|| {
            format!(
                "Ghidra produced no response; inspect {}",
                stdout_path.display()
            )
        })?;
        if response.len() > 16 * 1024 * 1024 {
            bail!("Ghidra response exceeds 16 MiB");
        }
        let data: serde_json::Value =
            serde_json::from_slice(&response).context("Ghidra response is not JSON")?;
        if data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            bail!(
                "Ghidra bridge failed: {data}; logs: {}",
                stdout_path.display()
            );
        }
        if data.get("target_sha256").and_then(|v| v.as_str()) != Some(&hash) {
            bail!("Ghidra response target identity mismatch");
        }
        if sha256_file(&target)? != hash {
            bail!("target changed during Ghidra analysis; result rejected");
        }
        fs::write(ready, serde_json::to_vec_pretty(&analysis_binding)?)?;
        let result = GhidraResult {
            cache_key: key,
            target_sha256: hash,
            tool_version: version,
            cached: false,
            data,
            stdout_path,
            stderr_path,
        };
        let staged = work.join("result.json");
        fs::write(&staged, serde_json::to_vec_pretty(&result)?)?;
        // Same-key concurrent queries can both complete; either successful result is valid.
        if !cache.exists()
            && let Err(e) = fs::rename(&staged, &cache)
            && !cache.exists()
        {
            return Err(e.into());
        }
        Ok(result)
    }
}

struct ProjectLock {
    _file: fs::File,
    activity: PathBuf,
}
impl ProjectLock {
    fn try_acquire(project: &Path) -> std::result::Result<Self, fs::TryLockError> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(project.join("worker.lock"))
            .map_err(fs::TryLockError::Error)?;
        file.try_lock()?;
        Ok(Self {
            _file: file,
            activity: project.join("analysis.active.json"),
        })
    }
    fn require_idle(&self) -> Result<()> {
        ensure!(
            !self.activity.exists(),
            "prior Ghidra analysis outcome is unknown; inspect analysis.active.json and the analyzer before clearing the claim"
        );
        Ok(())
    }
    fn begin(&self) -> Result<()> {
        self.require_idle()?;
        let mut marker = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&self.activity)?;
        marker.write_all(&serde_json::to_vec(
            &serde_json::json!({"owner_pid":std::process::id()}),
        )?)?;
        marker.sync_all()?;
        Ok(())
    }
    fn finish(&self) -> Result<()> {
        fs::remove_file(&self.activity)?;
        Ok(())
    }
}

/// Owns the Ghidra child and terminates its whole process tree on drop or
/// timeout. analyzeHeadless launchers spawn the real analyzer as a
/// grandchild (a shell or cmd shim in between), which kill_on_drop alone
/// would leave running after a cancellation.
struct TreeKill {
    child: Option<Box<dyn process_wrap::tokio::ChildWrapper>>,
    project: Option<ProjectLock>,
}
impl TreeKill {
    fn spawn(command: tokio::process::Command) -> std::io::Result<Self> {
        use process_wrap::tokio::*;
        let mut command = CommandWrap::from(command);
        command.wrap(KillOnDrop);
        #[cfg(unix)]
        command.wrap(ProcessGroup::leader());
        #[cfg(windows)]
        command.wrap(JobObject);
        Ok(Self {
            child: Some(command.spawn()?),
            project: None,
        })
    }

    fn spawn_locked(command: tokio::process::Command, project: ProjectLock) -> Result<Self> {
        project.begin()?;
        // On a failed/ambiguous spawn the claim remains for inspection.
        let mut process = Self::spawn(command)?;
        process.project = Some(project);
        Ok(process)
    }

    async fn terminate(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            if child.wait().await.is_ok()
                && let Some(project) = &self.project
            {
                let _ = project.finish();
            }
        }
    }
}
impl Drop for TreeKill {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Signal the entire group/job synchronously; only reaping is deferred.
            let _ = child.start_kill();
            let project = self.project.take();
            tokio::spawn(async move {
                if child.wait().await.is_ok()
                    && let Some(project) = &project
                {
                    let _ = project.finish();
                }
                // Hold the kernel lock through cancellation and reaping.
                drop(project);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(unix)]
    #[tokio::test]
    async fn process_wrap_cancels_descendants_after_the_launcher_exits() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let marker = directory.path().join("survived");
        let ready = directory.path().join("ready");
        let mut command = tokio::process::Command::new("python");
        command.arg("-c").arg(r#"import subprocess,sys,pathlib
subprocess.Popen([sys.executable,'-c',"import pathlib,time,sys;pathlib.Path(sys.argv[2]).write_text('ready');time.sleep(0.5);pathlib.Path(sys.argv[1]).write_text('alive')",sys.argv[1],sys.argv[2]])
"#).arg(&marker).arg(&ready).stdout(Stdio::null()).stderr(Stdio::null());
        let mut process = TreeKill::spawn(command)?;
        tokio::time::timeout(Duration::from_secs(5), async {
            process.child.as_mut().unwrap().wait().await?;
            while !ready.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        drop(process);
        tokio::time::sleep(Duration::from_millis(650)).await;
        ensure!(
            !marker.exists(),
            "launcher exit left its descendant running"
        );
        Ok(())
    }

    fn analysis_key(backend: &GhidraBackend, target: &Path) -> Result<String> {
        let hash = sha256_file(target)?;
        let executable = fs::canonicalize(&backend.config.executable)?;
        let binding = json!({"schema":1,"target_sha256":hash,"tool_version":"unknown","tool_properties":sha256_bytes(b""),"launcher":sha256_file(&executable)?,"bridge":sha256_bytes(include_bytes!("../../workers/ghidra/WukongBridge.java")),"analysis_timeout_seconds":backend.config.analysis_timeout_seconds,"max_cpu":backend.config.max_cpu});
        Ok(sha256_bytes(&serde_json::to_vec(&binding)?))
    }

    fn query_key(backend: &GhidraBackend, target: &Path, query: &GhidraQuery) -> Result<String> {
        let hash = sha256_file(target)?;
        let executable = fs::canonicalize(&backend.config.executable)?;
        let analysis_binding = json!({"schema":1,"target_sha256":hash,"tool_version":"unknown","tool_properties":sha256_bytes(b""),"launcher":sha256_file(&executable)?,"bridge":sha256_bytes(include_bytes!("../../workers/ghidra/WukongBridge.java")),"analysis_timeout_seconds":backend.config.analysis_timeout_seconds,"max_cpu":backend.config.max_cpu});
        Ok(sha256_bytes(&serde_json::to_vec(
            &json!({"analysis":analysis_binding,"query":query}),
        )?))
    }

    #[tokio::test]
    async fn lock_wait_polls_the_cache_and_never_reruns_the_holder_work() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let executable = dir.path().join("fake-ghidra");
        fs::write(&executable, b"exit 1")?;
        let target = dir.path().join("sample.bin");
        fs::write(&target, b"target bytes")?;
        let backend = GhidraBackend::new(GhidraConfig {
            executable,
            cache_dir: dir.path().join("cache"),
            script_dir: None,
            timeout_seconds: 5,
            analysis_timeout_seconds: 2,
            max_cpu: 1,
        })
        .unwrap();
        let query = GhidraQuery::Functions {
            pattern: None,
            offset: 0,
            limit: 5,
        };
        let project = backend
            .config
            .cache_dir
            .join("projects")
            .join(analysis_key(&backend, &target)?);
        fs::create_dir_all(&project)?;
        // A concurrent holder already owns the project lock.
        let lock_path = project.join("worker.lock");
        let holder_file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .read(true)
            .open(&lock_path)?;
        holder_file.try_lock()?;
        let root = backend
            .config
            .cache_dir
            .join("results")
            .join(query_key(&backend, &target, &query)?);
        fs::create_dir_all(&root)?;
        // The holder publishes a valid cache result after a short delay; the
        // waiting query must serve it instead of timing out or invoking the
        // fake Ghidra executable.
        let result = GhidraResult {
            cache_key: query_key(&backend, &target, &query)?,
            target_sha256: sha256_file(&target)?,
            tool_version: "unknown".into(),
            cached: false,
            data: json!({"functions":[{"name":"main"}]}),
            stdout_path: root.join("stdout.log"),
            stderr_path: root.join("stderr.log"),
        };
        let holder_root = root.clone();
        let holder = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let staged = holder_root.join("result.json.tmp");
            fs::write(&staged, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
            fs::rename(&staged, holder_root.join("result.json")).unwrap();
            drop(holder_file);
        });
        let started = std::time::Instant::now();
        let served = backend.query(&target, &query).await?;
        assert!(served.cached);
        assert_eq!(served.data["functions"][0]["name"], "main");
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "the query should have waited for the holder"
        );
        assert!(
            !root.join("response.json").exists(),
            "the fake Ghidra executable must not have been invoked"
        );
        holder.await.unwrap();
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_holds_the_project_lock_until_analyzer_reaping() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let lock = ProjectLock::try_acquire(directory.path())?;
        let mut command = tokio::process::Command::new("python");
        command.arg("-c").arg("import time;time.sleep(60)");
        let process = TreeKill::spawn_locked(command, lock)?;
        drop(process);
        assert!(matches!(
            ProjectLock::try_acquire(directory.path()),
            Err(fs::TryLockError::WouldBlock)
        ));
        tokio::time::timeout(Duration::from_secs(5), async {
            while directory.path().join("analysis.active.json").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        ProjectLock::try_acquire(directory.path())?.require_idle()?;
        Ok(())
    }

    #[test]
    fn project_lock_releases_without_unlinking_and_preserves_uncertain_analysis() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let lock = ProjectLock::try_acquire(directory.path())?;
        assert!(matches!(
            ProjectLock::try_acquire(directory.path()),
            Err(fs::TryLockError::WouldBlock)
        ));
        lock.begin()?;
        drop(lock); // Simulate an owner lost before analysis completion.
        assert!(directory.path().join("worker.lock").exists());
        let successor = ProjectLock::try_acquire(directory.path())?;
        assert!(successor.require_idle().is_err());
        successor.finish()?;
        successor.require_idle()?;
        Ok(())
    }
}
