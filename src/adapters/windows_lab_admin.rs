//! VMware guest-operations administrative channel for installation media.
//!
//! The password comes from the configured environment variable and is never
//! persisted in configuration or receipts. `vmrun` necessarily receives it on
//! its command line; the operator accepts that exposure on the controller.
use crate::application::windows_lab::admin::{AdminOutcome, AdminRequest, LabAdmin};
use crate::config::WindowsLabAdmin;
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

fn output_text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .replace(['\r', '\n'], " ")
    .trim()
    .to_owned()
}

pub struct VmwareAdmin {
    executable: PathBuf,
    vmx: PathBuf,
    user: String,
    password: String,
    timeout: Duration,
}

impl VmwareAdmin {
    /// Returns `None` when the password environment variable is absent, so
    /// status/prepare/run keep working without an administrative channel.
    pub fn from_env(
        executable: &Path,
        vmx: &Path,
        admin: &WindowsLabAdmin,
        timeout: Duration,
    ) -> Result<Option<Self>> {
        ensure!(
            executable.is_absolute() && vmx.is_absolute() && vmx.is_file(),
            "administrative channel requires absolute existing vmrun and VMX paths"
        );
        let password = match std::env::var(&admin.password_env) {
            Ok(value) if !value.is_empty() => value,
            _ => return Ok(None),
        };
        Ok(Some(Self {
            executable: std::path::absolute(executable)?,
            vmx: std::path::absolute(vmx)?,
            user: admin.user.clone(),
            password,
            timeout,
        }))
    }

    fn vmx(&self) -> Result<&str> {
        self.vmx.to_str().context("VMX path must be UTF-8")
    }

    async fn run(&self, arguments: Vec<String>) -> Result<std::process::Output> {
        let executable = self.executable.clone();
        let timeout = self.timeout;
        tokio::task::spawn_blocking(move || {
            let mut child = std::process::Command::new(&executable)
                .args(&arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .with_context(|| format!("start {}", executable.display()))?;
            let deadline = Instant::now() + timeout;
            loop {
                if child.try_wait()?.is_some() {
                    return child.wait_with_output().context("collect VMware output");
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!("administrative VMware operation exceeded its deadline");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })
        .await
        .context("administrative VMware task")?
    }
}

#[async_trait]
impl LabAdmin for VmwareAdmin {
    async fn install(&self, request: AdminRequest) -> Result<AdminOutcome> {
        let guest_dir = format!(r"C:\Windows\Temp\wukong-lab-{}", request.install_id);
        let mkdir_script =
            format!("New-Item -ItemType Directory -Force -Path '{guest_dir}' | Out-Null");
        let mkdir = vec![
            "-T".into(),
            "ws".into(),
            "-gu".into(),
            self.user.clone(),
            "-gp".into(),
            self.password.clone(),
            "runProgramInGuest".into(),
            self.vmx()?.into(),
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            mkdir_script,
        ];
        let mkdir_output = self.run(mkdir).await?;
        ensure!(
            mkdir_output.status.success(),
            "create guest media directory failed: {}",
            output_text(&mkdir_output)
        );
        for file in &request.files {
            ensure!(
                !file.name.is_empty()
                    && file
                        .name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
                "unsafe media file name"
            );
            let host = file
                .path
                .to_str()
                .context("media file path must be UTF-8")?
                .to_owned();
            let guest = format!(r"{guest_dir}\{}", file.name);
            let copy = vec![
                "-T".into(),
                "ws".into(),
                "-gu".into(),
                self.user.clone(),
                "-gp".into(),
                self.password.clone(),
                "copyFileFromHostToGuest".into(),
                self.vmx()?.into(),
                host,
                guest,
            ];
            let output = self.run(copy).await?;
            ensure!(
                output.status.success(),
                "administrative media copy of {} failed: {}",
                file.name,
                output_text(&output)
            );
        }
        let script = format!(r"& '{guest_dir}\install.ps1'");
        let run = vec![
            "-T".into(),
            "ws".into(),
            "-gu".into(),
            self.user.clone(),
            "-gp".into(),
            self.password.clone(),
            "runProgramInGuest".into(),
            self.vmx()?.into(),
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-Command".into(),
            script,
        ];
        let output = self.run(run).await?;
        Ok(AdminOutcome {
            exit_code: output.status.code().map(i64::from),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            guest_script: format!(r"{guest_dir}\install.ps1"),
            guest_media: guest_dir,
            settled: true,
        })
    }
}
