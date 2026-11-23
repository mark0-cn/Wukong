//! Inspect the current VM and prepare the deployment inputs and dry-run report.
use super::super::provision::{
    BOOT_TASK_NAME, DiskInspectionRequest, GuestFile, GuestPaths, IsoBuilder, MediaFinalizer,
    OfflineDisk, PROVISION_WRAPPER, SnapshotPlan, attach_offline_iso, boot_launcher,
    boot_task_injection, injected_file, provision_plan, read_finalize_spec, resolve_guest_disk,
    verify_media,
};
use super::super::{InjectedFile, MediaIdentity, ProvisionSource, SshKey, WindowsLab};
use super::{
    PlannedProvision, ProvisionArtifactPaths, ProvisionDiskPlan, ProvisionIntent, ProvisionOptions,
    ProvisionOutcome,
};
use crate::application::worker_media::FinalizeMediaRequest;
use anyhow::{Context, Result, bail, ensure};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

impl WindowsLab {
    /// Collect the media-derived guest files and the durable intent fields that
    /// do not depend on the guest disk. Nothing is mutated here.
    pub(super) fn prepare_provision_intent(
        &self,
        provision_id: String,
        paths: ProvisionArtifactPaths,
        guest: GuestPaths,
        media: MediaIdentity,
        source: ProvisionSource,
        identity_file: Option<PathBuf>,
    ) -> Result<ProvisionIntent> {
        let plan_bytes = provision_plan(&provision_id, &media, &guest)?;
        let boot_task = boot_task_injection()?;
        let guest_files = vec![
            injected_file(&guest.wrapper, PROVISION_WRAPPER.as_bytes().to_vec()),
            injected_file(&guest.plan, plan_bytes),
            injected_file(
                &guest.launcher,
                boot_launcher(&provision_id, &guest.wrapper)?,
            ),
            boot_task.file.clone(),
        ];
        let injected = injected_records(&guest_files)?;
        Ok(ProvisionIntent {
            provision_id,
            paths,
            guest,
            media,
            source,
            identity_file,
            guest_files,
            injected,
        })
    }

    pub(super) fn read_vmx_bytes(&self) -> Result<Vec<u8>> {
        super::super::provision::read_bounded(&self.vmx, 4 * 1024 * 1024)
            .with_context(|| format!("read registered VMX {}", self.vmx.display()))
    }

    /// Resolve and inspect the selected powered-off boot disk. Initial installs
    /// use the full disk; replacements deliberately use the restored base delta.
    pub(super) async fn plan_guest_disk(
        &self,
        vmx_bytes: &[u8],
        intent: &ProvisionIntent,
        offline: &dyn OfflineDisk,
    ) -> Result<ProvisionDiskPlan> {
        let vmx_text =
            std::str::from_utf8(vmx_bytes).context("registered VMX is not valid UTF-8")?;
        let guest_disk = resolve_guest_disk(vmx_text, &self.vmx)?;
        let inspection = offline
            .inspect(
                &guest_disk.path,
                &DiskInspectionRequest::for_provision(&intent.guest),
            )
            .await
            .context("offline preflight of the powered-off guest disk")?;
        ensure!(
            inspection.windows_volume && inspection.system_hive_present,
            "guest disk has no inspectable Windows system volume"
        );
        ensure!(
            inspection.software_hive_present,
            "guest disk has no inspectable SOFTWARE hive"
        );
        ensure!(
            inspection.control_set.is_some(),
            "guest SYSTEM hive has no readable current control set"
        );
        self.refuse_leftover_guest_state(&inspection)?;
        let vmx_edit = attach_offline_iso(vmx_text, &intent.paths.iso)?;
        Ok(ProvisionDiskPlan {
            guest_disk,
            vmx_edit,
        })
    }

    /// A stale artifact of a previous attempt must be refused with the exact
    /// operator command that removes it, never silently retried over.
    fn refuse_leftover_guest_state(
        &self,
        inspection: &super::super::provision::DiskInspection,
    ) -> Result<()> {
        let ids = inspection.leftover_operation_ids();
        if ids.is_empty()
            && inspection.existing_service.is_none()
            && inspection.provision_root_entries.is_empty()
            && !inspection.has_boot_task_leftovers()
        {
            return Ok(());
        }
        let operations = if ids.is_empty() {
            "an unknown earlier operation".to_owned()
        } else {
            ids.join(", ")
        };
        let example = ids
            .first()
            .map(|id| {
                format!(
                    "`wukong lab clean-guest --vm {} --operation {id}`",
                    self.vm_id
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "`wukong lab clean-guest --vm {} --operation <operation_id>`",
                    self.vm_id
                )
            });
        bail!(
            "guest still carries offline provisioning leftovers from {operations} (scheduled task registration or launcher, provision directory, \
             retired startup policy entry or legacy service key); power off the VM and clean that operation first with {example}, then retry provisioning"
        )
    }

    pub(super) async fn resolve_media(
        &self,
        options: &ProvisionOptions,
        paths: &ProvisionArtifactPaths,
        finalizer: Option<&dyn MediaFinalizer>,
    ) -> Result<(MediaIdentity, ProvisionSource, Option<PathBuf>)> {
        match &options.source {
            ProvisionSource::Media { directory } => {
                ensure!(
                    directory.is_absolute(),
                    "offline media directory must be absolute"
                );
                let media = verify_media(directory)?;
                ensure!(
                    !options.register,
                    "--register requires --template media so lab provision can generate the controller-managed SSH identity key"
                );
                Ok((media, options.source.clone(), None))
            }
            ProvisionSource::Template {
                template,
                keys,
                spec,
            } => {
                ensure!(
                    template.is_absolute() && keys.is_absolute() && spec.is_absolute(),
                    "template, key directory and finalize spec paths must be absolute"
                );
                ensure!(
                    template.is_dir(),
                    "offline media template directory is missing: {}",
                    template.display()
                );
                ensure!(
                    keys.is_dir(),
                    "controller key directory is missing: {}",
                    keys.display()
                );
                let finalize_spec = read_finalize_spec(spec)?;
                let identity_file = if finalize_spec.ssh_public_key.is_none() {
                    let ssh = paths.root.join("ssh");
                    fs::create_dir_all(&ssh).with_context(|| {
                        format!("create controller SSH identity directory {}", ssh.display())
                    })?;
                    Some(ssh.join("management_ed25519"))
                } else {
                    None
                };
                ensure!(
                    !options.register || identity_file.is_some(),
                    "--register requires a controller-generated SSH identity; omit ssh_public_key from the finalize spec"
                );
                let finalizer = finalizer.context(
                    "the offline media finalizer is unavailable; run lab provision on a Windows controller",
                )?;
                let request = FinalizeMediaRequest {
                    template: template.clone(),
                    keys: keys.clone(),
                    output: paths.media.clone(),
                    installation_id: finalize_spec.installation_id.clone(),
                    environment_id: finalize_spec.environment_id.clone(),
                    generation: finalize_spec.generation,
                    management_peer: finalize_spec.management_peer.parse()?,
                    ssh_port: finalize_spec.ssh_port,
                    ssh_public_key: finalize_spec
                        .ssh_public_key
                        .as_ref()
                        .map(SshKey::public)
                        .transpose()?,
                    generate_ssh_key: identity_file.clone(),
                    controller_version: None,
                };
                let finalized = finalizer
                    .finalize(&request)
                    .await
                    .context("finalize offline worker media")?;
                let verified = verify_media(&paths.media)?;
                verified.check_finalize_spec(&finalize_spec)?;
                ensure!(
                    finalized.media_path == paths.media
                        && finalized.ssh_identity_file == identity_file
                        && finalized.record.manifest_sha256 == verified.manifest_sha256,
                    "finalizer result differs from the verified media"
                );
                if let Some(identity) = &identity_file {
                    let public = read_generated_identity(identity)?;
                    ensure!(
                        public == verified.ssh_public_key,
                        "generated controller SSH identity differs from the finalized media"
                    );
                }
                Ok((verified, options.source.clone(), identity_file))
            }
        }
    }

    /// Validate and describe the plan without mutating the VM, the guest, the
    /// controller state or (for `--template`) the finalized media directory.
    pub(super) async fn dry_run_plan(
        &self,
        options: &ProvisionOptions,
        paths: ProvisionArtifactPaths,
        guest: GuestPaths,
        snapshot_plan: SnapshotPlan,
        iso_builder: Arc<dyn IsoBuilder>,
        offline: Arc<dyn OfflineDisk>,
    ) -> Result<ProvisionOutcome> {
        ensure!(
            options
                .iso_tool
                .as_ref()
                .is_none_or(|path| path.is_absolute()),
            "--iso-tool must be an absolute path"
        );
        if snapshot_plan == SnapshotPlan::ReplaceBaseline {
            return self.dry_run_replace_baseline(options, paths, guest).await;
        }
        let mut actions = Vec::new();
        match &options.source {
            ProvisionSource::Media { directory } => {
                ensure!(
                    directory.is_absolute(),
                    "offline media directory must be absolute"
                );
                let media = verify_media(directory)?;
                ensure!(
                    !options.register,
                    "--register requires --template media so lab provision can generate the controller-managed SSH identity key"
                );
                let builder = match &options.iso_tool {
                    Some(executable) => iso_builder.with_executable(executable)?,
                    None => iso_builder,
                };
                let intent = self.prepare_provision_intent(
                    guest.provision_id().to_owned(),
                    paths,
                    guest,
                    media,
                    options.source.clone(),
                    None,
                )?;
                let vmx_bytes = self.read_vmx_bytes()?;
                let disk = self
                    .plan_guest_disk(&vmx_bytes, &intent, offline.as_ref())
                    .await?;
                let plan = PlannedProvision::assemble(
                    intent,
                    vmx_bytes,
                    disk,
                    boot_task_injection()?,
                    self.baseline.clone(),
                    snapshot_plan == SnapshotPlan::ReplaceBaseline,
                    builder,
                    Duration::from_secs(options.timeout_seconds),
                );
                actions = planned_actions(&plan);
            }
            ProvisionSource::Template {
                template,
                keys,
                spec,
            } => {
                ensure!(
                    template.is_absolute() && keys.is_absolute() && spec.is_absolute(),
                    "template, key directory and finalize spec paths must be absolute"
                );
                ensure!(
                    template.is_dir(),
                    "offline media template directory is missing: {}",
                    template.display()
                );
                ensure!(
                    keys.is_dir(),
                    "controller key directory is missing: {}",
                    keys.display()
                );
                let finalize_spec = read_finalize_spec(spec)?;
                let identity_file = if finalize_spec.ssh_public_key.is_none() {
                    Some(paths.root.join("ssh").join("management_ed25519"))
                } else {
                    None
                };
                ensure!(
                    !options.register || identity_file.is_some(),
                    "--register requires a controller-generated SSH identity; omit ssh_public_key from the finalize spec"
                );
                ensure!(
                    !paths.media.exists(),
                    "finalize media output already exists: {}",
                    paths.media.display()
                );
                let vmx_bytes =
                    super::super::provision::read_bounded(&self.vmx, 4 * 1024 * 1024)
                        .with_context(|| format!("read registered VMX {}", self.vmx.display()))?;
                let vmx_text =
                    String::from_utf8(vmx_bytes).context("registered VMX is not valid UTF-8")?;
                let guest_disk = resolve_guest_disk(&vmx_text, &self.vmx)?;
                let inspection = offline
                    .inspect(
                        &guest_disk.path,
                        &DiskInspectionRequest::for_provision(&guest),
                    )
                    .await
                    .context("offline preflight of the powered-off guest disk")?;
                ensure!(
                    inspection.windows_volume && inspection.system_hive_present,
                    "guest disk has no inspectable Windows system volume"
                );
                ensure!(
                    inspection.software_hive_present,
                    "guest disk has no inspectable SOFTWARE hive"
                );
                self.refuse_leftover_guest_state(&inspection)?;
                let edit = attach_offline_iso(&vmx_text, &paths.iso)?;
                actions.push(format!(
                    "validate the finalize spec for environment {} (installation {}, generation {}, ssh_port {})",
                    finalize_spec.environment_id,
                    finalize_spec.installation_id,
                    finalize_spec.generation,
                    finalize_spec.ssh_port
                ));
                actions.push(format!(
                    "run the built-in Rust media finalizer for template {} --keys {} --spec {} --output {}{}",
                    template.display(),
                    keys.display(),
                    spec.display(),
                    paths.media.display(),
                    match &identity_file {
                        Some(key) => format!(" --generate-ssh-key {}", key.display()),
                        None => String::new(),
                    }
                ));
                actions.push(
                    "re-verify the finalized media signatures, receipt hashes and generated SSH identity"
                        .into(),
                );
                actions.push(format!(
                    "build ISO {} with the configured mkisofs (-V WUKONG_OFFLINE)",
                    paths.iso.display()
                ));
                actions.push(format!(
                    "back up VMX bytes to {}",
                    paths.vmx_backup.display()
                ));
                actions.push(format!(
                    "attach {} to {} in the VMX",
                    paths.iso.display(),
                    edit.device
                ));
                actions.push(format!(
                    "write fixed guest files {}, {} and {} through libguestfs",
                    guest.wrapper, guest.plan, guest.launcher
                ));
                actions.push(format!(
                    "write the fixed scheduled-task file {} and register the boot task {} ({}) in the guest SOFTWARE hive",
                    guest.task_file, BOOT_TASK_NAME, guest.task_guid
                ));
                actions.push(format!(
                    "start the VM; the {BOOT_TASK_NAME} boot task runs the wrapper as SYSTEM without a login, then powers the guest off"
                ));
                actions.push(
                    "restore the original VMX bytes and verify the guest receipt plus the removed boot task offline"
                        .into(),
                );
                actions.push(format!(
                    "create the baseline snapshot '{}' and leave the VM powered off",
                    self.baseline
                ));
                actions.push(format!(
                    "publish the provisioning receipt under {}/provisions",
                    self.store.root().display()
                ));
                actions.push(format!(
                    "guest disk {} (device {}) is provisioned offline",
                    guest_disk.path.display(),
                    guest_disk.device
                ));
            }
        }
        Ok(ProvisionOutcome {
            dry_run: true,
            planned_actions: actions,
            provision: None,
            receipt: None,
            environment_block: None,
            registration: None,
            verification: None,
        })
    }

    /// `--dry-run` with `--replace-baseline`: report staged verification and
    /// fixed-base publication without mutating the VM, guest or controller state.
    async fn dry_run_replace_baseline(
        &self,
        options: &ProvisionOptions,
        paths: ProvisionArtifactPaths,
        guest: GuestPaths,
    ) -> Result<ProvisionOutcome> {
        let mut actions = Vec::new();
        match &options.source {
            ProvisionSource::Media { directory } => {
                ensure!(
                    directory.is_absolute(),
                    "offline media directory must be absolute"
                );
                let media = verify_media(directory)?;
                ensure!(
                    !options.register,
                    "--register requires --template media so lab provision can generate the controller-managed SSH identity key"
                );
                actions.push(format!(
                    "re-verify finalized media {} (setup {}, enrollment {}, manifest {})",
                    media.media_root.display(),
                    media.setup_sha256,
                    media.enrollment_sha256,
                    media.manifest_sha256
                ));
            }
            ProvisionSource::Template {
                template,
                keys,
                spec,
            } => {
                ensure!(
                    template.is_absolute() && keys.is_absolute() && spec.is_absolute(),
                    "template, key directory and finalize spec paths must be absolute"
                );
                ensure!(
                    template.is_dir(),
                    "offline media template directory is missing: {}",
                    template.display()
                );
                ensure!(
                    keys.is_dir(),
                    "controller key directory is missing: {}",
                    keys.display()
                );
                let finalize_spec = read_finalize_spec(spec)?;
                let identity_file = if finalize_spec.ssh_public_key.is_none() {
                    Some(paths.root.join("ssh").join("management_ed25519"))
                } else {
                    None
                };
                ensure!(
                    !options.register || identity_file.is_some(),
                    "--register requires a controller-generated SSH identity; omit ssh_public_key from the finalize spec"
                );
                ensure!(
                    !paths.media.exists(),
                    "finalize media output already exists: {}",
                    paths.media.display()
                );
                actions.push(format!(
                    "validate the finalize spec for environment {} (installation {}, generation {}, ssh_port {})",
                    finalize_spec.environment_id,
                    finalize_spec.installation_id,
                    finalize_spec.generation,
                    finalize_spec.ssh_port
                ));
                actions.push(format!(
                    "run the built-in Rust media finalizer for template {} --keys {} --spec {} --output {}{}",
                    template.display(),
                    keys.display(),
                    spec.display(),
                    paths.media.display(),
                    match &identity_file {
                        Some(key) => format!(" --generate-ssh-key {}", key.display()),
                        None => String::new(),
                    }
                ));
                actions.push(
                    "re-verify the finalized media signatures, receipt hashes and generated SSH identity"
                        .into(),
                );
            }
        }
        let vmx_bytes = self.read_vmx_bytes()?;
        let vmx_text = String::from_utf8(vmx_bytes).context("registered VMX is not valid UTF-8")?;
        let edit = attach_offline_iso(&vmx_text, &paths.iso)?;
        actions.push(format!(
            "back up VMX bytes to {}",
            paths.vmx_backup.display()
        ));
        actions.push(format!(
            "persist the provisioning intent and update the restored delta while preserving fixed base '{}'",
            self.baseline
        ));
        actions.push(
            "stage the replacement, verify its worker identity, then delete the old base and capture the verified state under the fixed base name"
                .into(),
        );
        actions.push(format!(
            "build ISO {} with the configured mkisofs (-V WUKONG_OFFLINE)",
            paths.iso.display()
        ));
        actions.push(format!(
            "attach {} to {} in the VMX",
            paths.iso.display(),
            edit.device
        ));
        actions.push(format!(
            "write fixed guest files {}, {} and {} through libguestfs",
            guest.wrapper, guest.plan, guest.launcher
        ));
        actions.push(format!(
            "write the fixed scheduled-task file {} and register the boot task {} ({}) in the guest SOFTWARE hive",
            guest.task_file, BOOT_TASK_NAME, guest.task_guid
        ));
        actions.push(format!(
            "start the VM; the {BOOT_TASK_NAME} boot task runs the wrapper as SYSTEM without a login, then powers the guest off"
        ));
        actions.push(
            "restore the original VMX bytes and verify the guest receipt plus the removed boot task offline"
                .into(),
        );
        actions.push(
            "create an operation-owned staged base snapshot and leave the VM powered off".into(),
        );
        actions.push(format!(
            "publish the provisioning receipt under {}/provisions",
            self.store.root().display()
        ));
        Ok(ProvisionOutcome {
            dry_run: true,
            planned_actions: actions,
            provision: None,
            receipt: None,
            environment_block: None,
            registration: None,
            verification: None,
        })
    }
}

pub(super) fn planned_actions(plan: &PlannedProvision) -> Vec<String> {
    let mut actions = vec![
        format!(
            "re-verify finalized media {} (setup {}, enrollment {}, manifest {})",
            plan.media.media_root.display(),
            plan.media.setup_sha256,
            plan.media.enrollment_sha256,
            plan.media.manifest_sha256
        ),
        format!(
            "build ISO {} from the verified media with the configured mkisofs (-V WUKONG_OFFLINE)",
            plan.paths.iso.display()
        ),
        format!("back up VMX bytes to {}", plan.paths.vmx_backup.display()),
        format!(
            "attach {} to {} in the VMX",
            plan.paths.iso.display(),
            plan.vmx_edit.device
        ),
        format!(
            "write fixed guest files {}, {} and {} through libguestfs",
            plan.guest.wrapper, plan.guest.plan, plan.guest.launcher
        ),
        format!(
            "write the fixed scheduled-task file {} and register the boot task {} ({}) in the guest SOFTWARE hive",
            plan.guest.task_file, BOOT_TASK_NAME, plan.guest.task_guid
        ),
        format!(
            "start the VM; the {BOOT_TASK_NAME} boot task runs the wrapper as SYSTEM without a login, then powers the guest off"
        ),
        "restore the original VMX bytes, verify the guest receipt and the removed scheduled-task registration offline"
            .into(),
        if plan.replaces_baseline {
            "create an operation-owned staged base snapshot, verify it, retire the old base and capture the verified state under the fixed name"
                .into()
        } else {
            format!(
                "create the fixed base snapshot '{}' and leave the VM powered off",
                plan.baseline
            )
        },
        format!(
            "publish the provisioning receipt under {}/provisions",
            plan.paths
                .root
                .parent()
                .unwrap_or(&plan.paths.root)
                .display()
        ),
    ];
    actions.push(format!(
        "guest disk {} (device {}) is provisioned offline; the ISO is {:.1} MiB",
        plan.guest_disk.path.display(),
        plan.guest_disk.device,
        plan.media
            .media_root
            .metadata()
            .map(|m| m.len() as f64 / 1_048_576.0)
            .unwrap_or_default()
    ));
    actions
}

pub(super) fn injected_name(file: &GuestFile) -> Result<&str> {
    file.windows_path
        .rsplit('\\')
        .next()
        .filter(|name| {
            !name.is_empty() && *name != "." && *name != ".." && !name.contains(['/', ':'])
        })
        .with_context(|| format!("guest file has no file name: {}", file.windows_path))
}

fn injected_records(files: &[GuestFile]) -> Result<Vec<InjectedFile>> {
    files
        .iter()
        .map(|file| {
            Ok(InjectedFile {
                guest_path: file.windows_path.clone(),
                sha256: file.sha256.clone(),
                size: file.bytes.len() as u64,
                // The scheduled-task mechanism only writes files it creates:
                // nothing on the guest is preserved or backed up. The retired
                // local-GPO mechanism recorded preserved policy files this way.
                preexisting: false,
            })
        })
        .collect()
}

fn read_generated_identity(private: &Path) -> Result<SshKey> {
    let public = private.with_extension("pub");
    let bytes = super::super::provision::read_bounded(&public, 4 * 1024)
        .with_context(|| format!("read the generated SSH public key {}", public.display()))?;
    let text = String::from_utf8(bytes).context("generated SSH public key is not UTF-8")?;
    let mut fields = text.split_whitespace();
    let algorithm = fields
        .next()
        .context("generated SSH public key is empty")?
        .to_owned();
    let key = fields
        .next()
        .context("generated SSH public key is incomplete")?;
    let key = SshKey {
        algorithm,
        key: key.to_owned(),
    };
    key.validate()?;
    Ok(key)
}
