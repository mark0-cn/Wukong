//! Offline-provisioning host adapters.
//!
//! `WslOfflineDisk` reads and edits the powered-off guest disk through
//! operator-configured WSL `guestfish`/`hivexsh`; `MkisofsIsoBuilder` builds the
//! `WUKONG_OFFLINE` ISO; `VmrunGuestNetwork` reports the guest IPv4 from
//! `vmrun`; and `RustMediaFinalizer` runs the built-in Rust finalizer shared with
//! the GUI and CLI. Every tool path is operator configuration, every argv is
//! fixed and no shell is involved. The feature is Windows-only: other hosts get
//! inert stubs so the crate stays portable.
#[cfg(windows)]
mod platform {
    use crate::application::windows_lab::provision::{
        self, BootTaskEvidence, BootTaskInjection, BootTaskRegistry, BootTaskRemoval,
        BootTaskRemovalEvidence, DiskInspection, DiskInspectionRequest, GUEST_SOFTWARE_HIVE,
        GUEST_SYSTEM_CONFIG, GUEST_SYSTEM_HIVE, GuestFile, GuestNetwork, GuestObservation,
        GuestObservationRequest, GuestPaths, GuestPolicyFiles, GuestRemoval, GuestRemovalEvidence,
        IsoBuilder, MAX_POLICY_BYTES, MAX_RECEIPT_BYTES, MEDIA_LABEL, OfflineDisk, ServiceRemoval,
        ServiceRemovalEvidence, boot_task_cache_state, startup_command_lines,
        verify_boot_task_values,
    };
    use crate::application::worker_media::{FinalizeMediaRequest, FinalizedMedia, finalize_media};
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
                let mut boundary = MAX_TOOL_DIAGNOSTIC;
                while !text.is_char_boundary(boundary) {
                    boundary -= 1;
                }
                format!("{}…", &text[..boundary])
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

    fn valid_guestfish_device(device: &str) -> bool {
        !device.is_empty()
            && device.len() <= 256
            && device.starts_with("/dev/")
            && device
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte))
    }

    fn parse_ntfs_devices(text: &str) -> Result<Vec<String>> {
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        ensure!(
            lines.len() <= 256,
            "libguestfs returned too many filesystems"
        );
        let mut devices = Vec::new();
        for line in lines {
            let (device, kind) = line
                .split_once(':')
                .with_context(|| format!("unexpected list-filesystems output: {line}"))?;
            let device = device.trim();
            ensure!(
                valid_guestfish_device(device),
                "libguestfs returned an invalid device path"
            );
            if kind.trim() == "ntfs" && !devices.iter().any(|known| known == device) {
                devices.push(device.to_owned());
            }
        }
        ensure!(
            devices.len() <= 64,
            "too many NTFS filesystems were returned"
        );
        Ok(devices)
    }

    fn guestfish_session_script(root: &str, writable: bool, script: &str) -> Result<String> {
        ensure!(valid_guestfish_device(root), "invalid Windows root device");
        let mount = if writable { "mount" } else { "mount-ro" };
        Ok(format!("run\n{mount} {root} /\n{script}"))
    }

    /// guestfish ends a scripted session at EOF and has no `close` command
    /// (1.46.2 rejects it before running any later line), so the script stops
    /// after the download.
    fn guestfish_download_script(guest: &str, host: &str) -> String {
        format!("download {guest} {host}\n")
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

        async fn raw_guestfish(
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
            ];
            self.wsl_run(&arguments, Some(script.as_bytes().to_vec()))
                .await
        }

        async fn windows_root(&self, disk_posix: &str) -> Result<String> {
            let output = self
                .raw_guestfish(disk_posix, false, "run\nlist-filesystems\n")
                .await?;
            require_success(&output, "list guest filesystems")?;
            let candidates = parse_ntfs_devices(&output.stdout_text())?;
            let config = guestfish_path(GUEST_SYSTEM_CONFIG)?;
            let system = guestfish_path(GUEST_SYSTEM_HIVE)?;
            let software = guestfish_path(GUEST_SOFTWARE_HIVE)?;
            for device in &candidates {
                let script = guestfish_session_script(
                    device,
                    false,
                    &format!("is-dir {config}\nis-file {system}\nis-file {software}\n"),
                )?;
                let output = self.raw_guestfish(disk_posix, false, &script).await?;
                if output.success && parse_booleans(&output.stdout_text(), 3)? == [true, true, true]
                {
                    return Ok(device.clone());
                }
            }
            bail!(
                "no NTFS filesystem contains the fixed Windows SYSTEM and SOFTWARE hives ({} candidates)",
                candidates.len()
            )
        }

        async fn guestfish(
            &self,
            disk_posix: &str,
            writable: bool,
            script: &str,
        ) -> Result<ToolOutput> {
            // Do not use `-i`: libguestfs inspection resolves registry hives via
            // case_sensitive_path, which can fail on a written NTFS delta even
            // when the exact fixed paths remain readable. Select the system
            // volume by those exact paths, then mount it explicitly.
            let root = self.windows_root(disk_posix).await?;
            let script = guestfish_session_script(&root, writable, script)?;
            self.raw_guestfish(disk_posix, writable, &script).await
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

        async fn list_entries(
            &self,
            disk_posix: &str,
            guest: &str,
            operation: &str,
        ) -> Result<Vec<String>> {
            let output = self
                .guestfish(disk_posix, false, &format!("ls {guest}\n"))
                .await?;
            require_success(&output, operation)?;
            Ok(parse_lines(&output.stdout_text())
                .into_iter()
                .filter(|name| name != "." && name != "..")
                .collect())
        }

        async fn read_guest_bytes(
            &self,
            disk_posix: &str,
            guest: &str,
            limit: u64,
            operation: &str,
        ) -> Result<Vec<u8>> {
            let output = self
                .guestfish(disk_posix, false, &format!("cat {guest}\n"))
                .await?;
            require_success(&output, operation)?;
            ensure!(
                output.stdout.len() as u64 <= limit,
                "{operation} exceeded the controller size budget"
            );
            Ok(output.stdout)
        }

        async fn inspect_in(
            &self,
            staging: &str,
            disk_posix: &str,
            request: &DiskInspectionRequest,
        ) -> Result<DiskInspection> {
            let system_config = guestfish_path(GUEST_SYSTEM_CONFIG)?;
            let system_hive = guestfish_path(&request.system_hive)?;
            let software_hive = guestfish_path(&request.software_hive)?;
            let program_data = guestfish_path(&request.program_data)?;
            let provision_root = guestfish_path(&request.provision_root)?;
            let tasks_dir = guestfish_path(&request.tasks_dir)?;
            let task_file = guestfish_path(&request.task_file)?;
            let launcher = guestfish_path(&request.launcher)?;
            let scripts_dir = guestfish_path(&request.scripts_dir)?;
            let startup_dir = guestfish_path(&request.startup_dir)?;
            let scripts_ini = guestfish_path(&request.scripts_ini)?;
            let scripts_backup = guestfish_path(&request.scripts_ini_backup)?;
            let gpt_ini = guestfish_path(&request.gpt_ini)?;
            let gpt_backup = guestfish_path(&request.gpt_ini_backup)?;
            let gpo_root = guestfish_path(&request.gpo_root)?;
            let probe = format!(
                "is-dir {system_config}\nis-file {system_hive}\nis-file {software_hive}\nis-dir {program_data}\nis-dir {provision_root}\nis-dir {tasks_dir}\nis-file {task_file}\nis-file {launcher}\nis-dir {scripts_dir}\nis-dir {startup_dir}\nis-file {scripts_ini}\nis-file {scripts_backup}\nis-file {gpt_ini}\nis-file {gpt_backup}\nis-dir {gpo_root}\n"
            );
            let output = self.guestfish(disk_posix, false, &probe).await?;
            require_success(&output, "libguestfs preflight")?;
            let values = parse_booleans(&output.stdout_text(), 15)?;
            let mut inspection = DiskInspection {
                windows_volume: values[0],
                system_hive_present: values[1],
                software_hive_present: values[2],
                program_data_present: values[3],
                control_set: None,
                existing_service: None,
                provision_root_entries: Vec::new(),
                task_file_present: values[6],
                boot_task: provision::BootTaskCacheState::default(),
                launcher: None,
                policy: GuestPolicyFiles::default(),
            };
            if !inspection.windows_volume || !inspection.system_hive_present {
                return Ok(inspection);
            }
            if values[4] {
                inspection.provision_root_entries = self
                    .list_entries(disk_posix, &provision_root, "list guest provision root")
                    .await?;
            }
            if values[7] {
                inspection.launcher = Some(
                    self.read_guest_bytes(
                        disk_posix,
                        &launcher,
                        provision::MAX_LAUNCHER_BYTES,
                        "read guest boot launcher",
                    )
                    .await?,
                );
            }
            if values[8] {
                inspection.policy.scripts_directory_entries = self
                    .list_entries(disk_posix, &scripts_dir, "list guest scripts directory")
                    .await?;
            }
            if values[9] {
                inspection.policy.startup_directory_entries = self
                    .list_entries(disk_posix, &startup_dir, "list guest startup directory")
                    .await?;
            }
            if values[14] {
                inspection.policy.group_policy_entries = self
                    .list_entries(disk_posix, &gpo_root, "list guest Group Policy directory")
                    .await?;
            }
            if values[10] {
                let bytes = self
                    .read_guest_bytes(
                        disk_posix,
                        &scripts_ini,
                        MAX_POLICY_BYTES,
                        "read guest scripts.ini",
                    )
                    .await?;
                inspection.policy.startup_command_lines = startup_command_lines(&bytes)?;
                inspection.policy.scripts_ini = Some(bytes);
            }
            if values[11] {
                inspection.policy.scripts_ini_backup = Some(
                    self.read_guest_bytes(
                        disk_posix,
                        &scripts_backup,
                        MAX_POLICY_BYTES,
                        "read guest scripts.ini backup",
                    )
                    .await?,
                );
            }
            if values[12] {
                inspection.policy.gpt_ini = Some(
                    self.read_guest_bytes(
                        disk_posix,
                        &gpt_ini,
                        MAX_POLICY_BYTES,
                        "read guest gpt.ini",
                    )
                    .await?,
                );
            }
            if values[13] {
                inspection.policy.gpt_ini_backup = Some(
                    self.read_guest_bytes(
                        disk_posix,
                        &gpt_backup,
                        MAX_POLICY_BYTES,
                        "read guest gpt.ini backup",
                    )
                    .await?,
                );
            }
            if values[2] {
                let software = format!("{staging}/SOFTWARE");
                self.download_hive(
                    disk_posix,
                    &request.software_hive,
                    &software,
                    "download guest SOFTWARE hive",
                )
                .await?;
                let registry = provision::boot_task_registry();
                inspection.boot_task = self.boot_task_state_in(&software, &registry).await?;
            }
            let hive = format!("{staging}/SYSTEM");
            let script = guestfish_download_script(&system_hive, &hive);
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
            Ok(DiskInspection {
                control_set: Some(control_set),
                existing_service,
                ..inspection
            })
        }

        async fn write_files_in(
            &self,
            staging: &str,
            disk_posix: &str,
            files: &[GuestFile],
        ) -> Result<()> {
            ensure!(files.len() <= 16, "too many guest files were supplied");
            let mut directories: Vec<String> = Vec::new();
            for file in files {
                let parent = parent_windows(&file.windows_path)?;
                if !directories.contains(&parent) {
                    directories.push(parent);
                }
            }
            let mut script = String::new();
            for directory in &directories {
                script.push_str(&format!("mkdir-p {}\n", guestfish_path(directory)?));
            }
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

        async fn remove_service_in(
            &self,
            staging: &str,
            disk_posix: &str,
            removal: &ServiceRemoval,
        ) -> Result<ServiceRemovalEvidence> {
            provision::validate_service_name(&removal.service)?;
            let hive_guest = guestfish_path(&removal.system_hive)?;
            let hive = format!("{staging}/SYSTEM");
            let removed = format!("{staging}/SYSTEM.removed");
            let output = self
                .guestfish(
                    disk_posix,
                    false,
                    &format!("download {hive_guest} {hive}\n"),
                )
                .await?;
            require_success(&output, "download guest SYSTEM hive")?;
            let select = self
                .hivexsh(&hive, "cd \\Select\nlsval Current\nquit\n", false)
                .await?;
            let control_set = parse_control_set(&select)?;
            let list_script = format!("cd \\ControlSet{control_set:03}\\Services\nls\nquit\n");
            let services = self.hivexsh(&hive, &list_script, false).await?;
            let present = parse_lines(&services)
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&removal.service));
            if !present {
                return Ok(ServiceRemovalEvidence {
                    service_present: false,
                    removed: false,
                });
            }
            let output = self
                .wsl_run(&["cp".into(), hive.clone(), removed.clone()], None)
                .await?;
            require_success(&output, "stage guest SYSTEM hive")?;
            let script = provision::hive_remove_service_script(control_set, &removal.service)?;
            self.hivexsh(&removed, &script, true).await?;
            let verify = self.hivexsh(&removed, &list_script, false).await?;
            ensure!(
                !parse_lines(&verify)
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&removal.service)),
                "legacy one-shot service key was not removed"
            );
            let after = self.sha256(&removed).await?;
            let output = self
                .guestfish(
                    disk_posix,
                    true,
                    &format!("upload {removed} {hive_guest}\nsync\nchecksum sha256 {hive_guest}\n"),
                )
                .await?;
            require_success(&output, "upload cleaned SYSTEM hive")?;
            ensure!(
                parse_digest(&output.stdout_text())? == after,
                "uploaded SYSTEM hive failed verification"
            );
            Ok(ServiceRemovalEvidence {
                service_present: true,
                removed: true,
            })
        }

        async fn remove_paths_in(
            &self,
            disk_posix: &str,
            removal: &GuestRemoval,
        ) -> Result<GuestRemovalEvidence> {
            removal.validate()?;
            let mut directories = Vec::new();
            for directory in &removal.directories {
                directories.push(guestfish_path(directory)?);
            }
            let mut files = Vec::new();
            for file in &removal.files {
                files.push(guestfish_path(file)?);
            }
            let mut probe = String::new();
            for directory in &directories {
                probe.push_str(&format!("is-dir {directory}\n"));
            }
            for file in &files {
                probe.push_str(&format!("is-file {file}\n"));
            }
            let output = self.guestfish(disk_posix, false, &probe).await?;
            require_success(&output, "inspect guest cleanup paths")?;
            let values = parse_booleans(&output.stdout_text(), directories.len() + files.len())?;
            let mut evidence = GuestRemovalEvidence::default();
            let mut script = String::new();
            for (index, directory) in directories.iter().enumerate() {
                if values[index] {
                    script.push_str(&format!("rm-rf {directory}\n"));
                    evidence
                        .removed_directories
                        .push(removal.directories[index].clone());
                } else {
                    evidence.absent.push(removal.directories[index].clone());
                }
            }
            for (index, file) in files.iter().enumerate() {
                let value = values[directories.len() + index];
                if value {
                    script.push_str(&format!("rm {file}\n"));
                    evidence.removed_files.push(removal.files[index].clone());
                } else {
                    evidence.absent.push(removal.files[index].clone());
                }
            }
            if script.is_empty() {
                return Ok(evidence);
            }
            script.push_str("sync\n");
            let output = self.guestfish(disk_posix, true, &script).await?;
            require_success(&output, "remove guest cleanup paths")?;
            let mut verify = String::new();
            for directory in &directories {
                verify.push_str(&format!("is-dir {directory}\n"));
            }
            for file in &files {
                verify.push_str(&format!("is-file {file}\n"));
            }
            let output = self.guestfish(disk_posix, false, &verify).await?;
            require_success(&output, "verify guest cleanup paths")?;
            let remaining = parse_booleans(&output.stdout_text(), directories.len() + files.len())?;
            ensure!(
                remaining.iter().all(|value| !*value),
                "guest cleanup paths are still present after removal"
            );
            Ok(evidence)
        }

        async fn observe_in(
            &self,
            staging: &str,
            disk_posix: &str,
            request: &GuestObservationRequest,
        ) -> Result<GuestObservation> {
            let wrapper_guest = guestfish_path(&request.wrapper)?;
            let plan_guest = guestfish_path(&request.plan)?;
            let receipt_guest = guestfish_path(&request.receipt)?;
            let launcher_guest = guestfish_path(&request.launcher)?;
            let task_file_guest = guestfish_path(&request.task_file)?;
            let software_hive_guest = guestfish_path(&request.software_hive)?;
            let scripts_ini_guest = guestfish_path(&request.scripts_ini)?;
            let scripts_backup_guest = guestfish_path(&request.scripts_ini_backup)?;
            let gpt_backup_guest = guestfish_path(&request.gpt_ini_backup)?;
            let script = format!(
                "is-file {wrapper_guest}\nis-file {plan_guest}\nis-file {receipt_guest}\nis-file {launcher_guest}\nis-file {task_file_guest}\nis-file {scripts_ini_guest}\nis-file {scripts_backup_guest}\nis-file {gpt_backup_guest}\nis-file {software_hive_guest}\n"
            );
            let output = self.guestfish(disk_posix, false, &script).await?;
            require_success(&output, "inspect provisioned guest")?;
            let values = parse_booleans(&output.stdout_text(), 9)?;
            let mut startup_entry_present = false;
            if values[5] {
                let bytes = self
                    .read_guest_bytes(
                        disk_posix,
                        &scripts_ini_guest,
                        MAX_POLICY_BYTES,
                        "read guest scripts.ini after provisioning",
                    )
                    .await?;
                startup_entry_present = startup_command_lines(&bytes)?
                    .iter()
                    .any(|line| line == &request.startup_entry);
            }
            let mut receipt = None;
            if values[2] {
                let bytes = self
                    .read_guest_bytes(
                        disk_posix,
                        &receipt_guest,
                        MAX_RECEIPT_BYTES,
                        "read guest provisioning receipt",
                    )
                    .await?;
                ensure!(!bytes.is_empty(), "guest provisioning receipt is empty");
                receipt = Some(bytes);
            }
            let mut boot_task_registered = false;
            if values[8] {
                let software = format!("{staging}/SOFTWARE");
                self.download_hive(
                    disk_posix,
                    &request.software_hive,
                    &software,
                    "download guest SOFTWARE hive",
                )
                .await?;
                let registry = provision::boot_task_registry();
                boot_task_registered = self
                    .boot_task_state_in(&software, &registry)
                    .await?
                    .is_registered();
            }
            Ok(GuestObservation {
                receipt,
                wrapper_present: values[0],
                plan_present: values[1],
                task_file_present: values[4],
                boot_task_registered,
                launcher_present: values[3],
                scripts_ini_present: values[5],
                startup_entry_present,
                scripts_ini_backup_present: values[6],
                gpt_ini_backup_present: values[7],
            })
        }

        async fn download_hive(
            &self,
            disk_posix: &str,
            windows_hive: &str,
            local: &str,
            operation: &str,
        ) -> Result<()> {
            let guest = guestfish_path(windows_hive)?;
            let output = self
                .guestfish(disk_posix, false, &guestfish_download_script(&guest, local))
                .await?;
            require_success(&output, operation)
        }

        async fn hive_list(&self, hive: &str, path: &str) -> Result<Vec<String>> {
            let output = self
                .hivexsh(hive, &format!("cd {path}\nls\nquit\n"), false)
                .await?;
            Ok(parse_lines(&output)
                .into_iter()
                .filter(|name| name != "." && name != "..")
                .collect())
        }

        async fn boot_task_parents(
            &self,
            hive: &str,
            registry: &BootTaskRegistry,
        ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
            let tree = self
                .hive_list(hive, &format!(r"{}\Tree", registry.key))
                .await?;
            let tasks = self
                .hive_list(hive, &format!(r"{}\Tasks", registry.key))
                .await?;
            let boot = self
                .hive_list(hive, &format!(r"{}\Boot", registry.key))
                .await?;
            Ok((tree, tasks, boot))
        }

        async fn boot_task_state_in(
            &self,
            hive: &str,
            registry: &BootTaskRegistry,
        ) -> Result<provision::BootTaskCacheState> {
            let (tree, tasks, boot) = self.boot_task_parents(hive, registry).await?;
            Ok(boot_task_cache_state(&tree, &tasks, &boot))
        }

        /// Verify the canonical Tree/Tasks values and the empty Boot category key
        /// after an offline registration edit.
        async fn verify_boot_task_in_hive(
            &self,
            hive: &str,
            registry: &BootTaskRegistry,
        ) -> Result<()> {
            let values = self.hivexsh(hive, &registry.verify_script(), false).await?;
            verify_boot_task_values(&values)?;
            ensure!(
                self.boot_task_state_in(hive, registry).await?.is_complete(),
                "injected boot task registration is incomplete"
            );
            Ok(())
        }

        async fn inject_boot_task_in(
            &self,
            staging: &str,
            disk_posix: &str,
            injection: &BootTaskInjection,
        ) -> Result<BootTaskEvidence> {
            let registry = &injection.registry;
            let hive = format!("{staging}/SOFTWARE");
            self.download_hive(
                disk_posix,
                &registry.hive,
                &hive,
                "download guest SOFTWARE hive",
            )
            .await?;
            ensure!(
                !self
                    .boot_task_state_in(&hive, registry)
                    .await?
                    .is_registered(),
                "the guest SOFTWARE hive already registers the fixed boot task; clean the previous operation first"
            );
            let before = self.sha256(&hive).await?;
            let patched = format!("{staging}/SOFTWARE.patched");
            let output = self
                .wsl_run(&["cp".into(), hive.clone(), patched.clone()], None)
                .await?;
            require_success(&output, "stage guest SOFTWARE hive")?;
            self.hivexsh(&patched, &registry.write_script(), true)
                .await?;
            self.verify_boot_task_in_hive(&patched, registry).await?;
            let after = self.sha256(&patched).await?;
            ensure!(
                before != after,
                "the offline boot-task registration did not change the guest SOFTWARE hive"
            );
            let guest = guestfish_path(&registry.hive)?;
            let output = self
                .guestfish(
                    disk_posix,
                    true,
                    &format!("upload {patched} {guest}\nsync\nchecksum sha256 {guest}\n"),
                )
                .await?;
            require_success(&output, "upload boot-task SOFTWARE hive")?;
            ensure!(
                parse_digest(&output.stdout_text())? == after,
                "uploaded SOFTWARE hive failed verification"
            );
            Ok(BootTaskEvidence {
                hive_sha256_before: before,
                hive_sha256_after: after,
            })
        }

        async fn remove_boot_task_in(
            &self,
            staging: &str,
            disk_posix: &str,
            removal: &BootTaskRemoval,
        ) -> Result<BootTaskRemovalEvidence> {
            let registry = &removal.registry;
            let hive = format!("{staging}/SOFTWARE");
            self.download_hive(
                disk_posix,
                &removal.software_hive,
                &hive,
                "download guest SOFTWARE hive",
            )
            .await?;
            let state = self.boot_task_state_in(&hive, registry).await?;
            if !state.is_registered() {
                return Ok(BootTaskRemovalEvidence {
                    present: false,
                    removed: false,
                });
            }
            let patched = format!("{staging}/SOFTWARE.patched");
            let output = self
                .wsl_run(&["cp".into(), hive.clone(), patched.clone()], None)
                .await?;
            require_success(&output, "stage guest SOFTWARE hive")?;
            let script = registry
                .remove_script(state.tree, state.tasks, state.boot)
                .context("registered boot task has no TaskCache keys to remove")?;
            self.hivexsh(&patched, &script, true).await?;
            ensure!(
                !self
                    .boot_task_state_in(&patched, registry)
                    .await?
                    .is_registered(),
                "fixed boot task TaskCache keys were not removed"
            );
            let after = self.sha256(&patched).await?;
            let guest = guestfish_path(&removal.software_hive)?;
            let output = self
                .guestfish(
                    disk_posix,
                    true,
                    &format!("upload {patched} {guest}\nsync\nchecksum sha256 {guest}\n"),
                )
                .await?;
            require_success(&output, "upload cleaned SOFTWARE hive")?;
            ensure!(
                parse_digest(&output.stdout_text())? == after,
                "uploaded cleaned SOFTWARE hive failed verification"
            );
            Ok(BootTaskRemovalEvidence {
                present: true,
                removed: true,
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

        async fn inject_boot_task(
            &self,
            disk: &Path,
            injection: &BootTaskInjection,
        ) -> Result<BootTaskEvidence> {
            ensure!(disk.is_absolute(), "guest disk path must be absolute");
            let disk_posix = self.wsl_path(disk).await?;
            let staging = self.temp_directory().await?;
            let result = self
                .inject_boot_task_in(&staging, &disk_posix, injection)
                .await;
            self.remove_directory(&staging).await;
            result
        }

        async fn remove_boot_task(
            &self,
            disk: &Path,
            removal: &BootTaskRemoval,
        ) -> Result<BootTaskRemovalEvidence> {
            ensure!(disk.is_absolute(), "guest disk path must be absolute");
            let disk_posix = self.wsl_path(disk).await?;
            let staging = self.temp_directory().await?;
            let result = self
                .remove_boot_task_in(&staging, &disk_posix, removal)
                .await;
            self.remove_directory(&staging).await;
            result
        }

        async fn remove_service_key(
            &self,
            disk: &Path,
            removal: &ServiceRemoval,
        ) -> Result<ServiceRemovalEvidence> {
            ensure!(disk.is_absolute(), "guest disk path must be absolute");
            let disk_posix = self.wsl_path(disk).await?;
            let staging = self.temp_directory().await?;
            let result = self.remove_service_in(&staging, &disk_posix, removal).await;
            self.remove_directory(&staging).await;
            result
        }

        async fn remove_guest_paths(
            &self,
            disk: &Path,
            removal: &GuestRemoval,
        ) -> Result<GuestRemovalEvidence> {
            ensure!(disk.is_absolute(), "guest disk path must be absolute");
            let disk_posix = self.wsl_path(disk).await?;
            self.remove_paths_in(&disk_posix, removal).await
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

    pub struct RustMediaFinalizer;

    impl RustMediaFinalizer {
        pub fn new() -> Result<Self> {
            Ok(Self)
        }
    }

    #[async_trait]
    impl provision::MediaFinalizer for RustMediaFinalizer {
        async fn finalize(&self, request: &FinalizeMediaRequest) -> Result<FinalizedMedia> {
            let request = request.clone();
            tokio::task::spawn_blocking(move || finalize_media(&request))
                .await
                .context("offline media finalize worker stopped unexpectedly")?
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// guestfish 1.46.2 (WSL Ubuntu) knows only the verbs below; `close` is
        /// not one of them, so a script that emits it aborts before the
        /// download runs. The session ends at EOF.
        const SUPPORTED: &[&str] = &[
            "cat",
            "checksum",
            "download",
            "is-dir",
            "is-file",
            "list-filesystems",
            "ls",
            "mkdir-p",
            "mount",
            "mount-ro",
            "run",
            "sync",
            "upload",
        ];

        #[test]
        fn diagnostic_truncates_on_utf8_boundary() {
            let output = ToolOutput {
                success: false,
                stdout: format!("{}界", "a".repeat(MAX_TOOL_DIAGNOSTIC - 1)).into_bytes(),
                stderr: Vec::new(),
            };

            assert_eq!(
                output.diagnostic(),
                format!("{}…", "a".repeat(MAX_TOOL_DIAGNOSTIC - 1))
            );
        }

        #[test]
        fn ntfs_devices_are_strictly_parsed_and_deduplicated() {
            let devices = parse_ntfs_devices(
                "/dev/sda1: vfat\n/dev/sda3: ntfs\n/dev/sdb1: ntfs\n/dev/sda3: ntfs\n",
            )
            .unwrap();
            assert_eq!(devices, ["/dev/sda3", "/dev/sdb1"]);
            assert!(parse_ntfs_devices("").unwrap().is_empty());
            assert!(parse_ntfs_devices("/dev/sda3\n").is_err());
            assert!(parse_ntfs_devices("/dev/sda3 /: ntfs\n").is_err());
            assert!(parse_ntfs_devices("relative: ntfs\n").is_err());
        }

        #[test]
        fn guestfish_sessions_mount_the_verified_windows_root_explicitly() {
            let read = guestfish_session_script(
                "/dev/sda3",
                false,
                "is-file /Windows/System32/config/SOFTWARE\n",
            )
            .unwrap();
            assert_eq!(
                read,
                "run\nmount-ro /dev/sda3 /\nis-file /Windows/System32/config/SOFTWARE\n"
            );
            let write = guestfish_session_script("/dev/sda3", true, "sync\n").unwrap();
            assert_eq!(write, "run\nmount /dev/sda3 /\nsync\n");
            assert!(guestfish_session_script("/dev/sda3 /", false, "sync\n").is_err());
        }

        #[test]
        fn guestfish_download_script_uses_only_supported_commands() {
            let script = guestfish_download_script(
                "/Windows/System32/config/SYSTEM",
                "/tmp/wukong-lab-provision-0123456789abcdef/SYSTEM",
            );
            assert_eq!(
                script,
                "download /Windows/System32/config/SYSTEM /tmp/wukong-lab-provision-0123456789abcdef/SYSTEM\n",
                "the download script must stop at EOF instead of ending with a close command"
            );
            let commands: Vec<&str> = script
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect();
            assert_eq!(
                commands.len(),
                1,
                "download script has unexpected lines: {script:?}"
            );
            for command in commands {
                let verb = command
                    .split_whitespace()
                    .next()
                    .expect("guestfish script contains an empty command line");
                assert!(
                    SUPPORTED.contains(&verb),
                    "unsupported guestfish command {verb:?} in {script:?}"
                );
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use crate::application::windows_lab::provision::{
        BootTaskEvidence, BootTaskInjection, BootTaskRemoval, BootTaskRemovalEvidence,
        DiskInspection, DiskInspectionRequest, GuestFile, GuestNetwork, GuestObservation,
        GuestObservationRequest, GuestRemoval, GuestRemovalEvidence, IsoBuilder, OfflineDisk,
        ServiceRemoval, ServiceRemovalEvidence,
    };
    use crate::application::worker_media::{FinalizeMediaRequest, FinalizedMedia};
    use anyhow::{Result, bail};
    use async_trait::async_trait;
    use std::{path::Path, sync::Arc, time::Duration};

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

        async fn inject_boot_task(
            &self,
            _disk: &Path,
            _injection: &BootTaskInjection,
        ) -> Result<BootTaskEvidence> {
            bail!(WINDOWS_ONLY)
        }

        async fn remove_boot_task(
            &self,
            _disk: &Path,
            _removal: &BootTaskRemoval,
        ) -> Result<BootTaskRemovalEvidence> {
            bail!(WINDOWS_ONLY)
        }

        async fn remove_service_key(
            &self,
            _disk: &Path,
            _removal: &ServiceRemoval,
        ) -> Result<ServiceRemovalEvidence> {
            bail!(WINDOWS_ONLY)
        }

        async fn remove_guest_paths(
            &self,
            _disk: &Path,
            _removal: &GuestRemoval,
        ) -> Result<GuestRemovalEvidence> {
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

    pub struct RustMediaFinalizer;

    impl RustMediaFinalizer {
        pub fn new() -> Result<Self> {
            Ok(Self)
        }
    }

    #[async_trait]
    impl crate::application::windows_lab::provision::MediaFinalizer for RustMediaFinalizer {
        async fn finalize(&self, _request: &FinalizeMediaRequest) -> Result<FinalizedMedia> {
            bail!(WINDOWS_ONLY)
        }
    }
}

pub use platform::{MkisofsIsoBuilder, RustMediaFinalizer, VmrunGuestNetwork, WslOfflineDisk};
