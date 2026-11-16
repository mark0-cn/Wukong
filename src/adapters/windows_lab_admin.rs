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

    async fn run(&self, arguments: Vec<String>, deadline: Instant) -> Result<std::process::Output> {
        let timeout = deadline
            .saturating_duration_since(Instant::now())
            .min(self.timeout);
        ensure!(!timeout.is_zero(), "installation deadline elapsed");
        crate::adapters::windows_lab::bounded_output(&self.executable, arguments, timeout).await
    }

    fn arguments(&self, operation: &str, tail: Vec<String>) -> Result<Vec<String>> {
        let mut args = vec![
            "-T".into(),
            "ws".into(),
            "-gu".into(),
            self.user.clone(),
            "-gp".into(),
            self.password.clone(),
            operation.into(),
            self.vmx()?.into(),
        ];
        args.extend(tail);
        Ok(args)
    }
}

#[async_trait]
impl LabAdmin for VmwareAdmin {
    async fn install(&self, request: AdminRequest) -> Result<AdminOutcome> {
        use sha2::{Digest, Sha256};
        let deadline = Instant::now() + Duration::from_secs(request.timeout_secs);
        let guest_dir = format!(r"C:\Windows\Temp\wukong-lab-{}", request.install_id);
        let temporary = tempfile::tempdir()?;
        let shell = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
        let output = self
            .run(
                self.arguments(
                    "runProgramInGuest",
                    vec![
                        shell.into(),
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-Command".into(),
                        format!(
                            "New-Item -ItemType Directory -Force -Path {} | Out-Null",
                            quote(&guest_dir)
                        ),
                    ],
                )?,
                deadline,
            )
            .await?;
        ensure!(
            output.status.success(),
            "create guest media directory failed: {}",
            output_text(&output)
        );
        let wrapper = installation_script(&request, &guest_dir)?;
        let wrapper_path = temporary.path().join("wukong-receipt.ps1");
        std::fs::write(&wrapper_path, wrapper)?;
        let mut copies = Vec::new();
        for file in &request.files {
            ensure!(
                crate::application::windows_lab::package::valid_artifact_name(&file.name),
                "unsafe media file name"
            );
            ensure!(
                !file.name.eq_ignore_ascii_case("wukong-receipt.ps1"),
                "reserved installation wrapper filename"
            );
            let bytes = crate::application::windows_lab::package::read_bounded(
                &file.path,
                crate::application::windows_lab::package::MAX_ARTIFACT_BYTES,
            )?;
            ensure!(
                hex::encode(Sha256::digest(&bytes)) == file.sha256,
                "media changed before transport"
            );
            let copy = temporary.path().join(&file.name);
            std::fs::write(&copy, bytes)?;
            copies.push((file.name.clone(), copy));
        }
        copies.push(("wukong-receipt.ps1".into(), wrapper_path));
        for (name, path) in copies {
            let output = self
                .run(
                    self.arguments(
                        "copyFileFromHostToGuest",
                        vec![
                            path.to_str().context("media path must be UTF-8")?.into(),
                            format!(r"{guest_dir}\{name}"),
                        ],
                    )?,
                    deadline,
                )
                .await?;
            ensure!(
                output.status.success(),
                "guest copy failed: {}",
                output_text(&output)
            );
        }
        let output = self
            .run(
                self.arguments(
                    "runProgramInGuest",
                    vec![
                        shell.into(),
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-ExecutionPolicy".into(),
                        "Bypass".into(),
                        "-File".into(),
                        format!(r"{guest_dir}\wukong-receipt.ps1"),
                    ],
                )?,
                deadline,
            )
            .await?;
        ensure!(
            output.status.success(),
            "guest wrapper transport failed: {}",
            output_text(&output)
        );
        let receipt_path = temporary.path().join("receipt.json");
        let output = self
            .run(
                self.arguments(
                    "copyFileFromGuestToHost",
                    vec![
                        format!(r"{guest_dir}\receipt.json"),
                        receipt_path
                            .to_str()
                            .context("receipt path must be UTF-8")?
                            .into(),
                    ],
                )?,
                deadline,
            )
            .await?;
        ensure!(
            output.status.success(),
            "guest execution receipt is unavailable; installation outcome is unknown"
        );
        ensure!(
            std::fs::metadata(&receipt_path)?.len() <= 1024 * 1024,
            "guest receipt exceeds size budget"
        );
        let receipt_bytes = std::fs::read(receipt_path)?;
        let receipt: serde_json::Value = serde_json::from_slice(
            receipt_bytes
                .strip_prefix(&[0xef, 0xbb, 0xbf])
                .unwrap_or(&receipt_bytes),
        )?;
        ensure!(
            receipt["schema"] == 1
                && receipt["install_id"] == request.install_id
                && receipt["media_sha256"] == request.media_sha256
                && receipt["script_sha256"] == request.script_sha256,
            "guest receipt identity mismatch"
        );
        let exit_code = receipt["exit_code"].as_i64();
        ensure!(
            exit_code.is_some(),
            "guest execution did not produce an exit code; outcome is unknown"
        );
        Ok(AdminOutcome {
            exit_code,
            stdout: String::new(),
            stderr: receipt["error"].as_str().unwrap_or_default().into(),
            guest_script: format!(r"{guest_dir}\install.ps1"),
            guest_media: guest_dir,
            settled: true,
            verified: receipt["verified"] == true,
            guest_receipt: receipt,
        })
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// The fixed wrapper emits a guest receipt only after the script exits and
/// declared installation postconditions have been inspected in that guest.
fn installation_script(request: &AdminRequest, guest_dir: &str) -> Result<String> {
    let mut checks = String::new();
    for file in &request.files {
        checks.push_str(&format!("if ((Get-FileHash -LiteralPath {} -Algorithm SHA256).Hash.ToLowerInvariant() -ne {}) {{ throw 'media identity mismatch' }}\n", quote(&format!(r"{guest_dir}\{}",file.name)), quote(&file.sha256)));
    }
    let mut post = String::new();
    for target in &request.plan.targets {
        post.push_str(&format!("if (!(Test-Path -LiteralPath {} -PathType Leaf)) {{ throw 'install target missing' }}\n", quote(target)));
    }
    if let Some(service) = &request.plan.service {
        let class = if service.kernel_driver {
            "Win32_SystemDriver"
        } else {
            "Win32_Service"
        };
        post.push_str(&format!("$service = Get-CimInstance {class} -Filter {}\nif (!$service -or $service.PathName.Trim('\"') -ine {}) {{ throw 'service identity mismatch' }}\n", quote(&format!("Name='{}'", service.name)), quote(&service.binary)));
        if service.start {
            post.push_str("if ($service.State -ne 'Running') { throw 'service not running' }\n");
        }
    }
    if let Some(account) = &request.plan.account {
        post.push_str(&format!(
            "$null = Get-LocalUser -Name {} -ErrorAction Stop\n",
            quote(&account.name)
        ));
    }
    Ok(format!(
        r#"$ErrorActionPreference = 'Stop'
$receipt = @{{ schema=1; install_id={}; media_sha256={}; script_sha256={}; exit_code=$null; verified=$false; error=$null; cleanup='baseline_restore_required' }}
try {{
{checks}
$process = Start-Process -FilePath "$PSHOME\powershell.exe" -ArgumentList @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File','"{}\install.ps1"') -WindowStyle Hidden -Wait -PassThru
$receipt.exit_code = $process.ExitCode
if ($process.ExitCode -ne 0) {{ throw 'install script failed' }}
{post}
$receipt.verified = $true
}} catch {{ $receipt.error = $_.Exception.Message }}
$receipt | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath '{}\receipt.tmp' -Encoding UTF8
Move-Item -LiteralPath '{}\receipt.tmp' -Destination '{}\receipt.json' -Force
"#,
        quote(&request.install_id),
        quote(&request.media_sha256),
        quote(&request.script_sha256),
        guest_dir,
        guest_dir,
        guest_dir,
        guest_dir
    ))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::application::windows_lab::{admin::AdminFile, install::InstallPlan};
    use sha2::{Digest, Sha256};
    use std::os::windows::process::CommandExt;

    #[test]
    fn guest_wrapper_distinguishes_exit_status_and_verified_postconditions() -> Result<()> {
        for (script_body, target_exists, expected_exit, verified) in [
            ("exit 0", true, 0, true),
            ("exit 0", false, 0, false),
            ("exit 17", true, 17, false),
        ] {
            let dir = tempfile::tempdir()?;
            let target = dir.path().join("installed.bin");
            if target_exists {
                std::fs::write(&target, b"trusted fixture")?;
            }
            let script = dir.path().join("install.ps1");
            std::fs::write(&script, script_body)?;
            let digest = hex::encode(Sha256::digest(script_body.as_bytes()));
            let request = AdminRequest {
                install_id: "fixture-install".into(),
                media_sha256: "a".repeat(64),
                script_sha256: digest.clone(),
                files: vec![AdminFile {
                    name: "install.ps1".into(),
                    path: script,
                    sha256: digest,
                }],
                timeout_secs: 30,
                plan: InstallPlan {
                    targets: vec![target.to_string_lossy().into_owned()],
                    service: None,
                    account: None,
                    cleanup: vec![],
                },
            };
            let wrapper = dir.path().join("wrapper.ps1");
            std::fs::write(
                &wrapper,
                installation_script(&request, dir.path().to_str().unwrap())?,
            )?;
            let status = std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ])
                .arg(wrapper)
                // A guest starts Windows PowerShell with its own default modules.
                // Do not inherit a PowerShell 7 host's incompatible module path.
                .env_remove("PSModulePath")
                .creation_flags(0x08000000)
                .status()?;
            ensure!(status.success(), "trusted wrapper fixture failed");
            let bytes = std::fs::read(dir.path().join("receipt.json"))?;
            let receipt: serde_json::Value =
                serde_json::from_slice(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))?;
            assert_eq!(receipt["install_id"], request.install_id);
            assert_eq!(receipt["exit_code"], expected_exit, "{receipt}");
            assert_eq!(receipt["verified"], verified);
            assert_eq!(receipt["script_sha256"], request.script_sha256);
        }
        Ok(())
    }
}
