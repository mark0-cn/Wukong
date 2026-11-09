use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub struct Docker {
    executable: String,
}
impl Docker {
    pub fn new(executable: &str) -> Self {
        Self {
            executable: executable.into(),
        }
    }
    pub fn executable(&self) -> &str {
        &self.executable
    }
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command.stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command
    }
    pub fn run(&self, args: &[&str]) -> Result<String> {
        let output = self
            .command()
            .args(args)
            .output()
            .with_context(|| format!("start Docker at {}", self.executable))?;
        ensure!(
            output.status.success(),
            "Docker {} failed: {}",
            args.first().unwrap_or(&"command"),
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(4096)
                .collect::<String>()
        );
        ensure!(
            output.stdout.len() <= 16 * 1024 * 1024,
            "Docker response exceeds 16 MiB"
        );
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
    pub fn inspect(&self, kind: &str, name: &str) -> Result<Option<Value>> {
        let output = self.command().args([kind, "inspect", name]).output()?;
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            if error.to_ascii_lowercase().contains("no such ")
                || error.contains(&format!("network {name} not found"))
            {
                return Ok(None);
            }
            anyhow::bail!(
                "Docker inspection failed: {}",
                error.chars().take(4096).collect::<String>()
            );
        }
        let mut items: Vec<Value> = serde_json::from_slice(&output.stdout)?;
        ensure!(
            items.len() == 1,
            "Docker inspect did not return exactly one resource"
        );
        Ok(items.pop())
    }
    /// Long build/pull/save operations stream to disk instead of buffering logs.
    pub fn logged(&self, args: &[&str], log: &Path) -> Result<()> {
        fs::create_dir_all(log.parent().context("log needs parent")?)?;
        let file = fs::File::create(log)?;
        let status = self
            .command()
            .args(args)
            .stdout(Stdio::from(file.try_clone()?))
            .stderr(Stdio::from(file))
            .status()?;
        ensure!(
            status.success(),
            "Docker operation failed; inspect {}",
            log.display()
        );
        Ok(())
    }
    pub fn copy_to(&self, source: &Path, container: &str, destination: &str) -> Result<()> {
        self.run(&[
            "cp",
            &portable_path(source).to_string_lossy(),
            &format!("{container}:{destination}"),
        ])?;
        Ok(())
    }
    pub fn copy_from(&self, container: &str, source: &str, destination: &Path) -> Result<()> {
        self.run(&[
            "cp",
            &format!("{container}:{source}"),
            &portable_path(destination).to_string_lossy(),
        ])?;
        Ok(())
    }
}

pub fn portable_path(path: &Path) -> PathBuf {
    // Docker Desktop accepts drive paths, not Windows verbatim path prefixes.
    let value = path.to_string_lossy();
    PathBuf::from(value.strip_prefix(r"\\?\").unwrap_or(&value))
}
