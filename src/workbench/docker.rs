use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub struct Docker {
    executable: String,
    pub cancellation: Option<tokio_util::sync::CancellationToken>,
}
impl Docker {
    pub fn new(executable: &str) -> Self {
        Self {
            executable: executable.into(),
            cancellation: None,
        }
    }
    pub fn executable(&self) -> &str {
        &self.executable
    }
    pub fn command(&self) -> Command {
        #[cfg(not(windows))]
        let mut command = Command::new(&self.executable);
        #[cfg(windows)]
        let mut command = super::docker_preflight::configure_command(&self.executable);
        command.stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command
    }
    pub fn run(&self, args: &[&str]) -> Result<String> {
        let seconds = if matches!(
            args.first(),
            Some(&"info" | &"inspect" | &"version" | &"ps")
        ) {
            30
        } else {
            600
        };
        let output = super::command_output::capture(
            self.command().args(args),
            std::time::Duration::from_secs(seconds),
            16 * 1024 * 1024,
            self.cancellation.as_ref(),
        )?;
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
        let output = super::command_output::capture(
            self.command().args([kind, "inspect", name]),
            std::time::Duration::from_secs(30),
            16 * 1024 * 1024,
            self.cancellation.as_ref(),
        )?;
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
        self.logged_internal(args, log, None)
    }
    pub fn pull_observed(
        &self,
        image: &str,
        log: &Path,
        mut report: impl FnMut(super::pull_progress::PullProgress),
    ) -> Result<()> {
        self.logged_internal(&["pull", image], log, Some(&mut report))
    }
    fn logged_internal(
        &self,
        args: &[&str],
        log: &Path,
        report: Option<&mut dyn FnMut(super::pull_progress::PullProgress)>,
    ) -> Result<()> {
        if args.first() == Some(&"pull")
            && let Some(image) = args.get(1)
        {
            self.check_credentials(image)?;
        }
        fs::create_dir_all(log.parent().context("log needs parent")?)?;
        let file = fs::File::create(log)?;
        let mut child = self
            .command()
            .args(args)
            .stdout(Stdio::from(file.try_clone()?))
            .stderr(Stdio::from(file))
            .spawn()?;
        let result = match report {
            Some(report) => {
                super::pull_progress::monitor(&mut child, log, report, self.cancellation.as_ref())
            }
            None if self.cancellation.is_some() => super::pull_progress::monitor(
                &mut child,
                log,
                &mut |_| {},
                self.cancellation.as_ref(),
            ),
            None => child.wait().map_err(anyhow::Error::from),
        };
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let status = result?;
        if !status.success() {
            return Err(logged_failure(log, &status.to_string()).into());
        }
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

fn logged_failure(log: &Path, status: &str) -> crate::i18n::Message {
    use std::io::{Read, Seek, SeekFrom};
    let tail = (|| -> std::io::Result<String> {
        let mut file = fs::File::open(log)?;
        let size = file.metadata()?.len();
        file.seek(SeekFrom::Start(size.saturating_sub(4096)))?;
        let mut bytes = Vec::new();
        file.take(4096).read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes);
        Ok(text
            .lines()
            .rev()
            .take(30)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n"))
    })()
    .unwrap_or_default();
    crate::i18n::Message::new("system-docker-command-failed")
        .arg("status", status)
        .arg("log", portable_path(log).to_string_lossy().into_owned())
        .detail(tail)
}

#[cfg(test)]
mod tests {
    #[test]
    fn cancellation_of_a_streaming_command_reaps_the_client() {
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("pull.log");
        let token = tokio_util::sync::CancellationToken::new();
        let cancel = token.clone();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(80));
            cancel.cancel();
        });
        let mut docker = super::Docker::new(if cfg!(windows) { "cmd.exe" } else { "sh" });
        docker.cancellation = Some(token);
        let args: &[&str] = if cfg!(windows) {
            &["/D", "/C", "for /L %i in (1,0,2) do @rem wait"]
        } else {
            &["-c", "while :; do :; done"]
        };
        let started = std::time::Instant::now();
        let error = docker
            .logged_internal(args, &log, Some(&mut |_| {}))
            .unwrap_err();
        trigger.join().unwrap();
        assert!(error.is::<crate::workbench::cancellation::Cancelled>());
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        std::fs::remove_file(log).unwrap();
    }
    #[test]
    fn failed_command_includes_bounded_tail_and_preserves_the_full_log() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("pull.log");
        let content = format!(
            "{}\nerror getting credentials: helper missing\n",
            "download layer\n".repeat(1000)
        );
        std::fs::write(&log, &content).unwrap();
        let message = super::logged_failure(&log, "exit code: 1");
        let detail = message.detail.unwrap();
        assert!(detail.contains("error getting credentials: helper missing"));
        assert!(detail.len() <= 4096 && detail.lines().count() <= 30);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), content);
        assert!(message.args.contains_key("log"));
    }
}

pub fn portable_path(path: &Path) -> PathBuf {
    // Docker Desktop accepts drive paths, not Windows verbatim path prefixes.
    let value = path.to_string_lossy();
    PathBuf::from(value.strip_prefix(r"\\?\").unwrap_or(&value))
}
