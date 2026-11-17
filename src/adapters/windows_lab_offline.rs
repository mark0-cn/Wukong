//! Offline-provisioning host adapters.
//!
//! `WslOfflineDisk` edits the powered-off guest disk through operator-configured
//! WSL `guestfish`/`hivexsh`; `MkisofsIsoBuilder` builds the `WUKONG_OFFLINE`
//! ISO; `VmrunGuestNetwork` reports the guest IPv4 from `vmrun`; and
//! `PythonMediaFinalizer` invokes the repository's media finalize module. Every
//! tool path is operator configuration, every argv is fixed and no shell is
//! involved. The feature is Windows-only: other hosts get inert stubs so the
//! crate stays portable.
#[cfg(windows)]
mod platform {
    use crate::application::windows_lab::provision::{
        self, DiskInspection, DiskInspectionRequest, FinalizeRequest, GuestFile, GuestNetwork,
        GuestObservation, GuestObservationRequest, GuestPaths, HivePatch, HivePatchEvidence,
        IsoBuilder, MAX_RECEIPT_BYTES, MEDIA_LABEL, OfflineDisk,
    };
    use anyhow::{Context, Result, bail, ensure};
    use async_trait::async_trait;
    use std::{
        path::{Path, PathBuf},
        process::Stdio,
        sync::Arc,
        time::Duration,
    };
    use tokio::io::AsyncWriteExt;

    const MAX_TOOL_OUTPUT: usize = 1024 * 1024;
    const MAX_TOOL_DIAGNOSTIC: usize = 4096;

    struct ToolOutput {
        success: bool,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    }

    impl ToolOutput {
        fn stdout_text(&self) -> String {
            String::from_utf8_lossy(&self.stdout).into_owned()
        }

        fn diagnostic(&self) -> String {
            let text = format!(
                "{} {}",
                String::from_utf8_lossy(&self.stdout),
                String::from_utf8_lossy(&self.stderr)
            )
            .replace(['\r', '\n'], " ");
            let text = text.trim();
            if text.len() <= MAX_TOOL_DIAGNOSTIC {
                text.to_owned()
            } else {
                format!("{}…", &text[..MAX_TOOL_DIAGNOSTIC])
            }
        }
    }

    fn require_success(output: &ToolOutput, operation: &str) -> Result<()> {
        ensure!(
            output.success,
            "{operation} failed: {}",
            output.diagnostic()
        );
        Ok(())
    }

    /// Run a bounded child process with optional stdin bytes. Output is captured
    /// into temporary files so a chatty tool cannot alias the controller memory.
    async fn run_bounded(
        executable: &Path,
        arguments: &[String],
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        working_directory: Option<&Path>,
    ) -> Result<ToolOutput> {
        use std::io::{Read, Seek, SeekFrom};
        let mut stdout = tempfile::tempfile()?;
        let mut stderr = tempfile::tempfile()?;
        let mut command = tokio::process::Command::new(executable);
        command
            .args(arguments)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(stdout.try_clone()?)
            .stderr(stderr.try_clone()?)
            .kill_on_drop(true);
        if let Some(directory) = working_directory {
            command.current_dir(directory);
        }
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command
            .spawn()
            .with_context(|| format!("start {}", executable.display()))?;
        if let Some(bytes) = stdin {
            let mut handle = child
                .stdin
                .take()
                .context("child process stdin is unavailable")?;
            handle.write_all(&bytes).await?;
            drop(handle);
        }
        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(status) => status?,
            Err(_) => {
                let _ = child.kill().await;
                bail!(
                    "{} timed out; the external effect requires reconciliation",
                    executable.display()
                );
            }
        };
        stdout.seek(SeekFrom::Start(0))?;
        stderr.seek(SeekFrom::Start(0))?;
        let mut out = Vec::new();
        let mut err = Vec::new();
        stdout
            .take((MAX_TOOL_OUTPUT + 1) as u64)
            .read_to_end(&mut out)?;
        stderr
            .take((MAX_TOOL_OUTPUT + 1) as u64)
            .read_to_end(&mut err)?;
        ensure!(
            out.len() <= MAX_TOOL_OUTPUT && err.len() <= MAX_TOOL_OUTPUT,
            "{} produced more output than the controller accepts",
            executable.display()
        );
        Ok(ToolOutput {
            success: status.success(),
            stdout: out,
            stderr: err,
        })
    }

    fn parse_booleans(text: &str, expected: usize) -> Result<Vec<bool>> {
        let values: Vec<bool> = text
            .lines()
            .map(str::trim)
            .filter(|line| matches!(*line, "true" | "false"))
            .map(|line| line == "true")
            .collect();
        ensure!(
            values.len() == expected,
            "unexpected libguestfs probe output ({expected} expected): {}",
            text.trim()
        );
        Ok(values)
    }

    fn parse_digest(text: &str) -> Result<String> {
        let digest = text
            .split_whitespace()
            .next()
            .context("sha256sum produced no digest")?;
        ensure!(
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "sha256sum produced an unexpected digest"
        );
        Ok(digest.to_owned())
    }

    fn parse_control_set(text: &str) -> Result<u32> {
        let value = text
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .map(str::trim)
            .context("hive Select output was empty")?;
        let number: u32 = value
            .parse()
            .with_context(|| format!("unexpected hive Select output: {value}"))?;
        ensure!(
            (1..=999).contains(&number),
            "unexpected Windows control set number"
        );
        Ok(number)
    }

    fn parse_lines(text: &str) -> Vec<String> {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn guestfish_path(windows: &str) -> Result<String> {
        GuestPaths::guestfish(windows)
    }

    fn parent_windows(windows: &str) -> Result<String> {
        let index = windows
            .rfind('\\')
            .context("guest path has no parent directory")?;
        ensure!(index > 2, "guest path has no parent directory");
        Ok(windows[..index].to_owned())
    }

    pub struct WslOfflineDisk {
        wsl: PathBuf,
        distribution: String,
        guestfish: String,
        hivexsh: String,
        timeout: Duration,
    }

    impl WslOfflineDisk {
        pub fn new(
            wsl: &Path,
            distribution: &str,
            guestfish: &str,
            hivexsh: &str,
            timeout: Duration,
        ) -> Result<Self> {
            ensure!(wsl.is_absolute(), "wsl executable path must be absolute");
            ensure!(
                !distribution.is_empty()
                    && !distribution.starts_with('-')
                    && distribution.len() <= 64,
                "invalid WSL distribution name"
            );
            ensure!(
                guestfish.starts_with('/') && hivexsh.starts_with('/'),
                "guestfish and hivexsh must be absolute POSIX paths"
            );
            ensure!(
                timeout >= Duration::from_secs(60),
                "offline tool timeout must be at least 60 seconds"
            );
            Ok(Self {
                wsl: wsl.to_path_buf(),
                distribution: distribution.into(),
                guestfish: guestfish.into(),
                hivexsh: hivexsh.into(),
                timeout,
            })
        }

        async fn wsl_run(
            &self,
            arguments: &[String],
            stdin: Option<Vec<u8>>,
        ) -> Result<ToolOutput> {
            let mut command = vec![
                "-d".to_owned(),
                self.distribution.clone(),
                "-u".to_owned(),
                "root".to_owned(),
                "--".to_owned(),
            ];
            command.extend_from_slice(arguments);
            run_bounded(&self.wsl, &command, stdin, self.timeout, None).await
        }

        async fn wsl_path(&self, path: &Path) -> Result<String> {
            let windows = path
                .to_str()
                .context("offline tooling path must be UTF-8")?
                .replace('\\', "/");
            let output = self
                .wsl_run(&["wslpath".into(), "-a".into(), windows], None)
                .await?;
            require_success(&output, "wslpath")?;
            let text = output.stdout_text();
            let posix = text.trim();
            ensure!(
                posix.starts_with('/')
                    && !posix.contains(['\n', '\r', '\0'])
                    && posix.len() <= 4096,
                "wslpath did not translate {}",
                path.display()
            );
            Ok(posix.to_owned())
        }

        async fn sha256(&self, posix: &str) -> Result<String> {
            let output = self
                .wsl_run(&["sha256sum".into(), posix.into()], None)
                .await?;
            require_success(&output, "sha256sum")?;
            parse_digest(&output.stdout_text())
        }

        async fn write_file(&self, posix: &str, bytes: &[u8]) -> Result<()> {
            let output = self
                .wsl_run(&["tee".into(), posix.into()], Some(bytes.to_vec()))
                .await?;
            require_success(&output, "write WSL staging file")
        }

        async fn temp_directory(&self) -> Result<String> {
            let directory = format!(
                "/tmp/wukong-lab-provision-{}",
                uuid::Uuid::new_v4().simple()
            );
            let output = self
                .wsl_run(&["mkdir".into(), "-p".into(), directory.clone()], None)
                .await?;
            require_success(&output, "create WSL staging directory")?;
            Ok(directory)
        }

        async fn remove_directory(&self, directory: &str) {
            if directory.starts_with("/tmp/wukong-lab-provision-") {
                let _ = self
                    .wsl_run(&["rm".into(), "-rf".into(), directory.into()], None)
                    .await;
            }
        }

        async fn guestfish(
            &self,
            disk_posix: &str,
            writable: bool,
            script: &str,
        ) -> Result<ToolOutput> {
            let arguments = vec![
                self.guestfish.clone(),
                if writable {
                    "--rw".to_owned()
                } else {
                    "--ro".to_owned()
                },
                "-a".to_owned(),
                disk_posix.to_owned(),
                "-i".to_owned(),
            ];
            self.wsl_run(&arguments, Some(script.as_bytes().to_vec()))
                .await
        }

        async fn hivexsh(&self, hive_posix: &str, script: &str, writable: bool) -> Result<String> {
            let mut arguments = vec![self.hivexsh.clone()];
            if writable {
                arguments.push("-w".into());
            }
            arguments.push(hive_posix.into());
            let output = self
                .wsl_run(&arguments, Some(script.as_bytes().to_vec()))
                .await?;
            require_success(&output, "hivexsh")?;
            Ok(output.stdout_text())
        }

        async fn inspect_in(
            &self,
            staging: &str,
            disk_posix: &str,
            request: &DiskInspectionRequest,
        ) -> Result<DiskInspection> {
            let system_hive = guestfish_path(&request.system_hive)?;
            let program_data = guestfish_path(&request.program_data)?;
            let provision_root = guestfish_path(&request.provision_root)?;
            let probe = format!(
                "is-dir /Windows/System32/config\nis-file {system_hive}\nis-dir {program_data}\nis-dir {provision_root}\n"
            );
            let output = self.guestfish(disk_posix, false, &probe).await?;
            require_success(&output, "libguestfs preflight")?;
            let values = parse_booleans(&output.stdout_text(), 4)?;
            let inspection = DiskInspection {
                windows_volume: values[0],
                system_hive_present: values[1],
                program_data_present: values[2],
                control_set: None,
                existing_service: None,
                provision_root_entries: Vec::new(),
            };
            if !inspection.windows_volume || !inspection.system_hive_present {
                return Ok(inspection);
            }
            let hive = format!("{staging}/SYSTEM");
            let script = format!("download {system_hive} {hive}\nclose\n");
            let output = self.guestfish(disk_posix, false, &script).await?;
            require_success(&output, "download guest SYSTEM hive")?;
            let select = self
                .hivexsh(&hive, "cd \\Select\nlsval Current\nquit\n", false)
                .await?;
            let control_set = parse_control_set(&select)?;
            let services = self
                .hivexsh(
                    &hive,
                    &format!("cd \\ControlSet{control_set:03}\\Services\nls\nquit\n"),
                    false,
                )
                .await?;
            let existing_service = parse_lines(&services)
                .into_iter()
                .find(|name| name.starts_with(&request.service_prefix));
            let mut provision_root_entries = Vec::new();
            if values[3] {
                let output = self
                    .guestfish(disk_posix, false, &format!("ls {provision_root}\n"))
                    .await?;
                require_success(&output, "list guest provision root")?;
                provision_root_entries = parse_lines(&output.stdout_text())
                    .into_iter()
                    .filter(|name| name != "." && name != "..")
                    .collect();
            }
            Ok(DiskInspection {
                control_set: Some(control_set),
                existing_service,
                provision_root_entries,
                ..inspection
            })
        }

        async fn write_files_in(
            &self,
            staging: &str,
            disk_posix: &str,
            files: &[GuestFile],
        ) -> Result<()> {
            let root = parent_windows(&files[0].windows_path)?;
            let root_guest = guestfish_path(&root)?;
            let mut script = format!("mkdir-p {root_guest}\n");
            let mut local_paths = Vec::new();
            for (index, file) in files.iter().enumerate() {
                let local = format!("{staging}/file{index}");
                self.write_file(&local, &file.bytes).await?;
                let guest = guestfish_path(&file.windows_path)?;
                script.push_str(&format!("upload {local} {guest}\n"));
                local_paths.push((guest, file.sha256.clone()));
            }
            script.push_str("sync\n");
            for (guest, _) in &local_paths {
                script.push_str(&format!("checksum sha256 {guest}\n"));
            }
            let output = self.guestfish(disk_posix, true, &script).await?;
            require_success(&output, "write guest provisioning files")?;
            let digests: Vec<String> = output
                .stdout_text()
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect();
            ensure!(
                digests.len() == local_paths.len(),
                "libguestfs did not report every guest file digest"
            );
            for ((guest, expected), actual) in local_paths.iter().zip(digests.iter()) {
                ensure!(actual == expected, "guest file {guest} failed verification");
            }
            Ok(())
        }

        async fn patch_hive_in(
            &self,
            staging: &str,
            disk_posix: &str,
            patch: &HivePatch,
        ) -> Result<HivePatchEvidence> {
            let hive_guest = guestfish_path(&patch.system_hive)?;
            let hive = format!("{staging}/SYSTEM");
            let patched = format!("{staging}/SYSTEM.patched");
            let output = self
                .guestfish(
                    disk_posix,
                    false,
                    &format!("download {hive_guest} {hive}\n"),
                )
                .await?;
            require_success(&output, "download guest SYSTEM hive")?;
            let before = self.sha256(&hive).await?;
            if let Some(backup) = &patch.host_backup {
                let backup_posix = self.wsl_path(backup).await?;
                if let Some(parent) = backup_posix.rfind('/') {
                    let directory = &backup_posix[..parent];
                    let output = self
                        .wsl_run(&["mkdir".into(), "-p".into(), directory.into()], None)
                        .await?;
                    require_success(&output, "create hive backup directory")?;
                }
                let output = self
                    .wsl_run(&["cp".into(), hive.clone(), backup_posix], None)
                    .await?;
                require_success(&output, "back up guest SYSTEM hive")?;
            }
            let output = self
                .wsl_run(&["cp".into(), hive.clone(), patched.clone()], None)
                .await?;
            require_success(&output, "stage guest SYSTEM hive")?;
            self.hivexsh(&patched, &patch.script, true).await?;
            let verify_output = self.hivexsh(&patched, &patch.verify_script, false).await?;
            let after = self.sha256(&patched).await?;
            let output = self
                .guestfish(
                    disk_posix,
                    true,
                    &format!("upload {patched} {hive_guest}\nsync\nchecksum sha256 {hive_guest}\n"),
                )
                .await?;
            require_success(&output, "upload patched SYSTEM hive")?;
            let uploaded = parse_digest(&output.stdout_text())?;
            ensure!(
                uploaded == after,
                "uploaded SYSTEM hive failed verification"
            );
            Ok(HivePatchEvidence {
                hive_sha256_before: before,
                hive_sha256_after: after,
                verify_output,
            })
        }

        async fn observe_in(
            &self,
            staging: &str,
            disk_posix: &str,
            request: &GuestObservationRequest,
        ) -> Result<GuestObservation> {
            let hive = format!("{staging}/SYSTEM.after");
            let hive_guest = guestfish_path(&request.system_hive)?;
            let wrapper_guest = guestfish_path(&request.wrapper)?;
            let plan_guest = guestfish_path(&request.plan)?;
            let receipt_guest = guestfish_path(&request.receipt)?;
            let script = format!(
                "is-file {wrapper_guest}\nis-file {plan_guest}\nis-file {receipt_guest}\ndownload {hive_guest} {hive}\n"
            );
            let output = self.guestfish(disk_posix, false, &script).await?;
            require_success(&output, "inspect provisioned guest")?;
            let values = parse_booleans(&output.stdout_text(), 3)?;
            let services = self
                .hivexsh(
                    &hive,
                    &format!(
                        "cd \\ControlSet{0:03}\\Services\nls\nquit\n",
                        request.control_set
                    ),
                    false,
                )
                .await?;
            let service_present = parse_lines(&services)
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&request.service_name));
            let mut receipt = None;
            if values[2] {
                let output = self
                    .guestfish(disk_posix, false, &format!("cat {receipt_guest}\n"))
                    .await?;
                require_success(&output, "read guest provisioning receipt")?;
                ensure!(
                    output.stdout.len() as u64 <= MAX_RECEIPT_BYTES,
                    "guest provisioning receipt exceeds the size budget"
                );
                ensure!(
                    !output.stdout.is_empty(),
                    "guest provisioning receipt is empty"
                );
                receipt = Some(output.stdout);
            }
            Ok(GuestObservation {
                receipt,
                service_present,
                wrapper_present: values[0],
                plan_present: values[1],
            })
        }
    }

    #[async_trait]
    impl OfflineDisk for WslOfflineDisk {
        async fn inspect(
            &self,
            disk: &Path,
            request: &DiskInspectionRequest,
        ) -> Result<DiskInspection> {
            ensure!(disk.is_absolute(), "guest disk path must be absolute");
            let disk_posix = self.wsl_path(disk).await?;
            let staging = self.temp_directory().await?;
            let result = self.inspect_in(&staging, &disk_posix, request).await;
            self.remove_directory(&staging).await;
            result
        }

        async fn write_guest_files(&self, disk: &Path, files: &[GuestFile]) -> Result<()> {
            ensure!(disk.is_absolute(), "guest disk path must be absolute");
            ensure!(!files.is_empty(), "no guest files were supplied");
            let disk_posix = self.wsl_path(disk).await?;
            let staging = self.temp_directory().await?;
            let result = self.write_files_in(&staging, &disk_posix, files).await;
            self.remove_directory(&staging).await;
            result
        }

        async fn patch_system_hive(
            &self,
            disk: &Path,
            patch: &HivePatch,
        ) -> Result<HivePatchEvidence> {
            ensure!(disk.is_absolute(), "guest disk path must be absolute");
            let disk_posix = self.wsl_path(disk).await?;
            let staging = self.temp_directory().await?;
            let result = self.patch_hive_in(&staging, &disk_posix, patch).await;
            self.remove_directory(&staging).await;
            result
        }

        async fn observe_guest(
            &self,
            disk: &Path,
            request: &GuestObservationRequest,
        ) -> Result<GuestObservation> {
            ensure!(disk.is_absolute(), "guest disk path must be absolute");
            let disk_posix = self.wsl_path(disk).await?;
            let staging = self.temp_directory().await?;
            let result = self.observe_in(&staging, &disk_posix, request).await;
            self.remove_directory(&staging).await;
            result
        }
    }

    pub struct MkisofsIsoBuilder {
        executable: PathBuf,
        timeout: Duration,
    }

    impl MkisofsIsoBuilder {
        pub fn new(executable: &Path, timeout: Duration) -> Result<Self> {
            ensure!(
                executable.is_absolute(),
                "mkisofs executable path must be absolute"
            );
            ensure!(
                timeout >= Duration::from_secs(60),
                "ISO build timeout must be at least 60 seconds"
            );
            Ok(Self {
                executable: executable.to_path_buf(),
                timeout,
            })
        }
    }

    impl std::fmt::Debug for MkisofsIsoBuilder {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("MkisofsIsoBuilder")
                .field("executable", &self.executable)
                .finish_non_exhaustive()
        }
    }

    #[async_trait]
    impl IsoBuilder for MkisofsIsoBuilder {
        fn with_executable(&self, executable: &Path) -> Result<Arc<dyn IsoBuilder>> {
            ensure!(
                executable.is_absolute(),
                "mkisofs executable path must be absolute"
            );
            Ok(Arc::new(Self {
                executable: executable.to_path_buf(),
                timeout: self.timeout,
            }))
        }

        async fn build(&self, media: &Path, output: &Path) -> Result<()> {
            ensure!(media.is_absolute(), "offline media path must be absolute");
            ensure!(output.is_absolute(), "ISO output path must be absolute");
            ensure!(
                !output.exists(),
                "ISO output already exists: {}",
                output.display()
            );
            let media = media.to_str().context("media path must be UTF-8")?;
            let output_text = output.to_str().context("ISO path must be UTF-8")?;
            let arguments = [
                "-quiet",
                "-iso-level",
                "4",
                "-joliet",
                "-joliet-long",
                "-V",
                MEDIA_LABEL,
                "-o",
                output_text,
                media,
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
            let command = crate::adapters::windows_lab::bounded_output(
                &self.executable,
                arguments,
                self.timeout,
            )
            .await?;
            if !command.status.success() {
                let text = format!(
                    "{} {}",
                    String::from_utf8_lossy(&command.stdout),
                    String::from_utf8_lossy(&command.stderr)
                )
                .replace(['\r', '\n'], " ");
                bail!("mkisofs failed ({}): {}", command.status, text.trim());
            }
            let metadata = std::fs::metadata(output)
                .with_context(|| format!("ISO builder produced no image: {}", output.display()))?;
            ensure!(
                metadata.is_file() && metadata.len() > 0,
                "ISO image is empty"
            );
            Ok(())
        }
    }

    pub struct VmrunGuestNetwork {
        executable: PathBuf,
        vmx: PathBuf,
        timeout: Duration,
    }

    impl VmrunGuestNetwork {
        pub fn new(executable: &Path, vmx: &Path, timeout: Duration) -> Result<Self> {
            ensure!(
                executable.is_absolute() && vmx.is_absolute(),
                "vmrun and VMX paths must be absolute"
            );
            Ok(Self {
                executable: executable.to_path_buf(),
                vmx: vmx.to_path_buf(),
                timeout,
            })
        }
    }

    #[async_trait]
    impl GuestNetwork for VmrunGuestNetwork {
        async fn guest_ip(&self) -> Result<String> {
            let vmx = self.vmx.to_str().context("VMX path must be UTF-8")?;
            let command = crate::adapters::windows_lab::bounded_output(
                &self.executable,
                vec![
                    "-T".into(),
                    "ws".into(),
                    "getGuestIPAddress".into(),
                    vmx.into(),
                ],
                self.timeout,
            )
            .await?;
            if !command.status.success() {
                let text = format!(
                    "{} {}",
                    String::from_utf8_lossy(&command.stdout),
                    String::from_utf8_lossy(&command.stderr)
                )
                .replace(['\r', '\n'], " ");
                bail!("vmrun getGuestIPAddress failed: {}", text.trim());
            }
            let text = String::from_utf8_lossy(&command.stdout);
            let reported = text
                .lines()
                .rfind(|line| !line.trim().is_empty())
                .map(str::trim)
                .context("vmrun reported no guest address")?;
            let address: std::net::Ipv4Addr = reported
                .parse()
                .with_context(|| format!("vmrun reported an invalid guest address: {reported}"))?;
            ensure!(
                address.to_string() == reported && !address.is_unspecified(),
                "vmrun reported an unusable guest address"
            );
            Ok(address.to_string())
        }
    }

    pub struct PythonMediaFinalizer {
        working_directory: PathBuf,
        timeout: Duration,
    }

    impl PythonMediaFinalizer {
        pub fn new() -> Result<Self> {
            let working_directory = std::env::current_dir()
                .context("the controller working directory is unavailable")?;
            Ok(Self {
                working_directory,
                timeout: Duration::from_secs(1800),
            })
        }
    }

    #[async_trait]
    impl provision::MediaFinalizer for PythonMediaFinalizer {
        async fn finalize(&self, request: &FinalizeRequest) -> Result<serde_json::Value> {
            ensure!(
                self.working_directory
                    .join("scripts/worker/media.py")
                    .is_file(),
                "offline media finalize requires the repository scripts package; run lab provision from the wukong checkout"
            );
            for (label, path) in [
                ("template", &request.template),
                ("key directory", &request.keys),
                ("finalize spec", &request.spec),
                ("media output", &request.output),
            ] {
                ensure!(
                    path.is_absolute(),
                    "offline media {label} path must be absolute"
                );
            }
            ensure!(
                request.template.is_dir(),
                "offline media template directory is missing"
            );
            ensure!(request.keys.is_dir(), "controller key directory is missing");
            ensure!(request.spec.is_file(), "finalize spec is missing");
            ensure!(
                !request.output.exists(),
                "offline media output already exists"
            );
            let mut arguments = vec![
                "-m".to_owned(),
                "scripts.worker.media".to_owned(),
                "finalize".to_owned(),
                "--template".to_owned(),
                path_text(&request.template)?,
                "--keys".to_owned(),
                path_text(&request.keys)?,
                "--spec".to_owned(),
                path_text(&request.spec)?,
                "--output".to_owned(),
                path_text(&request.output)?,
            ];
            if let Some(key) = &request.generate_ssh_key {
                ensure!(
                    key.is_absolute() && !key.exists(),
                    "generated SSH identity output must be a new absolute path"
                );
                arguments.push("--generate-ssh-key".into());
                arguments.push(path_text(key)?);
            }
            let output = run_bounded(
                Path::new("python"),
                &arguments,
                None,
                self.timeout,
                Some(&self.working_directory),
            )
            .await
            .context("run python -m scripts.worker.media finalize; Python must be on PATH")?;
            require_success(&output, "offline media finalize")?;
            let text = output.stdout_text();
            serde_json::from_str(text.trim()).context("parse offline media finalize output")
        }
    }

    fn path_text(path: &Path) -> Result<String> {
        Ok(path
            .to_str()
            .context("offline media path must be UTF-8")?
            .to_owned())
    }
}

#[cfg(not(windows))]
mod platform {
    use crate::application::windows_lab::provision::{
        DiskInspection, DiskInspectionRequest, FinalizeRequest, GuestFile, GuestNetwork,
        GuestObservation, GuestObservationRequest, HivePatch, HivePatchEvidence, IsoBuilder,
        OfflineDisk,
    };
    use anyhow::{Result, bail};
    use async_trait::async_trait;
    use std::{
        path::{Path, PathBuf},
        sync::Arc,
        time::Duration,
    };

    const WINDOWS_ONLY: &str =
        "offline lab VM provisioning requires a Windows controller with WSL libguestfs";

    pub struct WslOfflineDisk;

    impl WslOfflineDisk {
        pub fn new(
            _wsl: &Path,
            _distribution: &str,
            _guestfish: &str,
            _hivexsh: &str,
            _timeout: Duration,
        ) -> Result<Self> {
            Ok(Self)
        }
    }

    #[async_trait]
    impl OfflineDisk for WslOfflineDisk {
        async fn inspect(
            &self,
            _disk: &Path,
            _request: &DiskInspectionRequest,
        ) -> Result<DiskInspection> {
            bail!(WINDOWS_ONLY)
        }

        async fn write_guest_files(&self, _disk: &Path, _files: &[GuestFile]) -> Result<()> {
            bail!(WINDOWS_ONLY)
        }

        async fn patch_system_hive(
            &self,
            _disk: &Path,
            _patch: &HivePatch,
        ) -> Result<HivePatchEvidence> {
            bail!(WINDOWS_ONLY)
        }

        async fn observe_guest(
            &self,
            _disk: &Path,
            _request: &GuestObservationRequest,
        ) -> Result<GuestObservation> {
            bail!(WINDOWS_ONLY)
        }
    }

    pub struct MkisofsIsoBuilder;

    impl MkisofsIsoBuilder {
        pub fn new(_executable: &Path, _timeout: Duration) -> Result<Self> {
            Ok(Self)
        }
    }

    #[async_trait]
    impl IsoBuilder for MkisofsIsoBuilder {
        fn with_executable(&self, _executable: &Path) -> Result<Arc<dyn IsoBuilder>> {
            Ok(Arc::new(Self))
        }

        async fn build(&self, _media: &Path, _output: &Path) -> Result<()> {
            bail!(WINDOWS_ONLY)
        }
    }

    pub struct VmrunGuestNetwork;

    impl VmrunGuestNetwork {
        pub fn new(_executable: &Path, _vmx: &Path, _timeout: Duration) -> Result<Self> {
            Ok(Self)
        }
    }

    #[async_trait]
    impl GuestNetwork for VmrunGuestNetwork {
        async fn guest_ip(&self) -> Result<String> {
            bail!(WINDOWS_ONLY)
        }
    }

    pub struct PythonMediaFinalizer;

    impl PythonMediaFinalizer {
        pub fn new() -> Result<Self> {
            Ok(Self)
        }
    }

    #[async_trait]
    impl crate::application::windows_lab::provision::MediaFinalizer for PythonMediaFinalizer {
        async fn finalize(&self, _request: &FinalizeRequest) -> Result<serde_json::Value> {
            bail!(WINDOWS_ONLY)
        }
    }
}

pub use platform::{MkisofsIsoBuilder, PythonMediaFinalizer, VmrunGuestNetwork, WslOfflineDisk};
