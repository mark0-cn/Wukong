//! Fixed VMware Workstation operations for one registered experiment VM.
//!
//! Only the configured `vmrun` executable and VMX identity are used. Arguments
//! are constructed here; no caller-supplied option, path or shell is accepted.
use crate::application::windows_lab::VmHost;
use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

const MAX_SNAPSHOT_NAME: usize = 96;

pub struct VmwareVm {
    executable: PathBuf,
    vmx: PathBuf,
    timeout: Duration,
}

impl VmwareVm {
    pub fn new(executable: &Path, vmx: &Path, timeout: Duration) -> Result<Self> {
        ensure!(
            executable.is_absolute(),
            "vmrun executable path must be absolute"
        );
        ensure!(vmx.is_absolute(), "registered VMX path must be absolute");
        ensure!(
            vmx.extension().is_some_and(|ext| ext == "vmx"),
            "registered VM must name a .vmx file"
        );
        ensure!(
            vmx.is_file(),
            "registered VMX does not exist: {}",
            vmx.display()
        );
        ensure!(
            timeout >= Duration::from_secs(1),
            "VMware operation timeout must be positive"
        );
        let executable = std::path::absolute(executable)?;
        let vmx = std::path::absolute(vmx)?;
        Ok(Self {
            executable,
            vmx,
            timeout,
        })
    }

    fn vmx(&self) -> Result<&str> {
        self.vmx.to_str().context("VMX path must be UTF-8")
    }

    async fn run(&self, arguments: &[&str]) -> Result<String> {
        let executable = self.executable.clone();
        let arguments: Vec<String> = arguments.iter().map(|value| (*value).to_owned()).collect();
        let timeout = self.timeout;
        let output = tokio::task::spawn_blocking(move || {
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
                    bail!("VMware operation exceeded {} seconds", timeout.as_secs());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })
        .await
        .context("VMware operation task")??;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let text = format!("{stdout} {stderr}").replace(['\r', '\n'], " ");
            bail!(
                "VMware operation failed ({}): {}",
                output.status,
                text.trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[async_trait]
impl VmHost for VmwareVm {
    async fn running(&self) -> Result<bool> {
        let output = self.run(&["-T", "ws", "list"]).await?;
        parse_running(&output, &self.vmx)
    }

    async fn snapshots(&self) -> Result<Vec<String>> {
        let output = self
            .run(&["-T", "ws", "listSnapshots", self.vmx()?])
            .await?;
        parse_snapshots(&output)
    }

    async fn snapshot(&self, name: &str) -> Result<()> {
        validate_snapshot_name(name)?;
        self.run(&["-T", "ws", "snapshot", self.vmx()?, name])
            .await?;
        Ok(())
    }

    async fn revert(&self, name: &str) -> Result<()> {
        validate_snapshot_name(name)?;
        self.run(&["-T", "ws", "revertToSnapshot", self.vmx()?, name])
            .await?;
        Ok(())
    }

    async fn start(&self) -> Result<()> {
        self.run(&["-T", "ws", "start", self.vmx()?, "nogui"])
            .await?;
        Ok(())
    }
}

fn comparable(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_ascii_lowercase()
}

fn parse_running(output: &str, vmx: &Path) -> Result<bool> {
    let mut lines = output.lines();
    let header = lines.next().unwrap_or_default().trim();
    ensure!(
        header.starts_with("Total running VMs:"),
        "unexpected vmrun list output: {header}"
    );
    let target = comparable(vmx);
    Ok(lines
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .any(|line| comparable(Path::new(line)) == target))
}

fn parse_snapshots(output: &str) -> Result<Vec<String>> {
    let mut lines = output.lines();
    let header = lines.next().unwrap_or_default().trim();
    ensure!(
        header.starts_with("Total snapshots:"),
        "unexpected vmrun listSnapshots output: {header}"
    );
    Ok(lines
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

fn validate_snapshot_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= MAX_SNAPSHOT_NAME
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)),
        "unsupported VMware snapshot name"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_output_is_compared_by_registered_identity() {
        let vmx = Path::new(r"D:\vm\win10\Windows 10 x64.vmx");
        let output = "Total running VMs: 1\nD:\\vm\\win10\\Windows 10 x64.vmx\n";
        assert!(parse_running(output, vmx).unwrap());
        let other = "Total running VMs: 1\nD:\\vm\\other\\vm.vmx\n";
        assert!(!parse_running(other, vmx).unwrap());
        let device = "Total running VMs: 1\n\\\\?\\D:\\vm\\win10\\Windows 10 x64.vmx\n";
        assert!(parse_running(device, vmx).unwrap());
    }

    #[test]
    fn malformed_list_headers_are_rejected() {
        assert!(parse_running("something else", Path::new(r"D:\vm\vm.vmx")).is_err());
        assert!(parse_snapshots("Total running VMs: 0").is_err());
    }

    #[test]
    fn snapshot_names_are_restricted() {
        assert!(validate_snapshot_name("wukong-lab-abc_1.2").is_ok());
        for invalid in ["", "a/b", "a b", "a\nb", "x".repeat(97).as_str()] {
            assert!(validate_snapshot_name(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn snapshots_are_read_in_order() {
        let names = parse_snapshots("Total snapshots: 3\nbaseline\nlab-1\nlab-2\n").unwrap();
        assert_eq!(names, ["baseline", "lab-1", "lab-2"]);
    }

    #[test]
    fn constructor_requires_an_absolute_existing_vmx() {
        let dir = tempfile::tempdir().unwrap();
        let vmx = dir.path().join("vm.vmx");
        std::fs::write(&vmx, b"config").unwrap();
        assert!(
            VmwareVm::new(Path::new("vmrun"), &vmx, Duration::from_secs(30)).is_err(),
            "relative executable"
        );
        assert!(
            VmwareVm::new(
                &dir.path().join("vmrun.exe"),
                Path::new("vm.vmx"),
                Duration::from_secs(30)
            )
            .is_err(),
            "relative VMX"
        );
        assert!(
            VmwareVm::new(&dir.path().join("vmrun.exe"), &vmx, Duration::from_secs(30)).is_ok()
        );
    }
}
