//! Offline VM bootstrap. All network-capable asset preparation stays on the
//! controller; this module has no download, MSI custom-action or shell-script path.
use crate::{
    host::accounts::{self, Role},
    host::authority::{Authority, SignedEnrollment},
    host::deployment::{self, Deployment},
    host::install_service,
    host::network,
    host::staging,
    isolation::security::{self, AclPolicy},
    rpc::pipe_transport,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    mem::size_of,
    path::{Path, PathBuf},
    ptr::null_mut,
};
use windows_sys::Win32::{
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};
use wukong_worker_package::{
    Compatibility, ConnectionReceipt, PublicKey, RECEIPT_VERSION, VerifiedEnrollment,
    VerifiedManifest, verify_package,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    pub schema: u32,
    pub controller_public_key: String,
    pub controller_version: String,
}
pub struct Plan {
    pub base: PathBuf,
    pub setup: Setup,
    pub manifest: VerifiedManifest,
    pub enrollment: VerifiedEnrollment,
}
impl Plan {
    pub fn read(configuration: &Path, pinned: &PublicKey) -> Result<Self> {
        ensure!(
            configuration.is_absolute()
                && configuration.file_name() == Some(OsStr::new("setup.json")),
            "offline setup must use an absolute setup.json path"
        );
        let setup: Setup =
            serde_json::from_slice(&crate::artifacts::io::read_bytes(configuration, 16384)?)?;
        ensure!(
            setup.schema == 1 && setup.controller_public_key == pinned.to_hex(),
            "offline setup differs from the independently pinned controller key"
        );
        let base = configuration
            .parent()
            .context("offline setup has no directory")?
            .to_owned();
        let manifest = verify_package(
            &base.join("payload"),
            pinned,
            &Compatibility {
                runtime_protocol: 4,
                controller_version: &setup.controller_version,
            },
        )?;

        ensure!(
            manifest.manifest().version == setup.controller_version,
            "offline setup version differs from its signed worker manifest"
        );

        let enrollment = deployment::read_enrollment(&base.join("enrollment"), pinned)?;
        enrollment.bind_payload(&manifest)?;
        let bytes = crate::artifacts::io::read_bytes(&std::env::current_exe()?, 64 * 1024 * 1024)?;
        ensure!(
            hex::encode(Sha256::digest(bytes))
                == manifest.manifest().files[&manifest.manifest().entrypoints.worker].sha256,
            "offline setup executable differs from its signed worker payload"
        );
        Ok(Self {
            base,
            setup,
            manifest,
            enrollment,
        })
    }
}
pub fn elevated() -> Result<bool> {
    let mut token = null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no closing, and `token` is a
    // null-initialized out-param; `check` proves success before `Handle` takes ownership.
    security::check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
    let token = security::Handle(token);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0;
    // SAFETY: `token.0` is a live `Handle`, `elevation` is a live `TOKEN_ELEVATION` on this
    // frame, and the call's declared length is exactly `size_of::<TOKEN_ELEVATION>()`.
    security::check(unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    })?;
    Ok(elevation.TokenIsElevated != 0)
}
/// Recheck the reviewed enrollment in the elevated process before installing.
pub fn install_reviewed(
    configuration: &Path,
    key: &PublicKey,
    enrollment_hash: &str,
) -> Result<ConnectionReceipt> {
    ensure!(
        elevated()?,
        "administrator elevation is required for VM installation"
    );
    let plan = Plan::read(configuration, key)?;
    ensure!(
        hex::encode(Sha256::digest(
            plan.enrollment.enrollment().canonical_bytes()?
        )) == enrollment_hash,
        "offline enrollment changed after setup review"
    );
    install(plan)
}

/// Elevated VM-only operation. The setup GUI reviews this concrete plan first.
/// Task tests never invoke this on the developer's installed environment.
fn install(plan: Plan) -> Result<ConnectionReceipt> {
    ensure!(
        elevated()?,
        "offline worker installation requires VM administrator elevation"
    );
    let enrollment = plan.enrollment.enrollment();
    let base = super::installation_receipt::install_base()?;
    let private = AclPolicy::private_vm();
    let public = AclPolicy::code_vm();
    if base.exists() {
        public.verify(&base, true)?;
    } else {
        public.create_directory(&base)?;
    }
    let root = base.join(&enrollment.installation_id);
    let metadata = root.join("metadata");
    if metadata.join("deployment.json").exists() {
        return upgrade(root, plan);
    }
    let recovering = root.exists();
    if recovering {
        crate::host::recovery::early_account_phase(&root, &plan)?;
    } else {
        private.create_directory(&root)?;
        private.create_directory(&metadata)?;
        crate::artifacts::io::write_new_json(
            &metadata.join("installation-intent.json"),
            &json!({"schema":1,"installation_id":enrollment.installation_id,"manifest_sha256":plan.manifest.sha256(),"generation":enrollment.generation,"roles":["management","worker","debugger","target"]}),
            &private,
        )?;
        let key = PublicKey::from_hex(&plan.setup.controller_public_key)?;
        let signed = SignedEnrollment::read(&plan.base.join("enrollment"), &key)?;
        ensure!(
            signed.enrollment.canonical_bytes()? == enrollment.canonical_bytes()?,
            "installation enrollment changed after review"
        );
        Authority::new(signed, &key)?.persist(&metadata, &private)?;
    }
    let mut accounts = Vec::new();
    for role in [Role::Management, Role::Worker, Role::Debugger, Role::Target] {
        let account = if recovering && role == Role::Management {
            accounts::recover_management(&enrollment.installation_id)?
        } else {
            accounts::create(&enrollment.installation_id, role)?
        };
        crate::artifacts::io::write_new_json(
            &metadata.join(format!("account-{role:?}.json")),
            &account,
            &private,
        )?;
        accounts.push(account);
    }
    let deployment = Deployment {
        schema: 1,
        installation_id: enrollment.installation_id.clone(),
        controller_public_key: plan.setup.controller_public_key.clone(),
        controller_version: plan.setup.controller_version.clone(),
        active_manifest_sha256: plan.manifest.sha256().into(),
        accounts,
    };
    let code = deployment.code_policy()?;
    code.apply(&root, true)?;
    code.create_directory(&root.join("versions"))?;
    let package = deployment.package(&root);
    let key = PublicKey::from_hex(&deployment.controller_public_key)?;
    staging::stage_payload(
        &plan.base.join("payload"),
        &package,
        &key,
        &Compatibility {
            runtime_protocol: 4,
            controller_version: &deployment.controller_version,
        },
        &code,
    )?;
    for name in ["enrollment.json", "enrollment.sig"] {
        crate::artifacts::io::copy_new(
            &plan.base.join("enrollment").join(name),
            &metadata.join(name),
            &private,
            16384,
        )?;
    }
    let worker = deployment.account(Role::Worker)?;
    let data = deployment.data_policy()?;
    data.create_directory(&root.join("data"))?;
    data.create_directory(&root.join("data/tmp"))?;
    crate::artifacts::io::write_new_json(
        &root.join("data/installation.json"),
        &json!({"installation_id":deployment.installation_id}),
        &data,
    )?;
    crate::artifacts::io::write_new_json(
        &root.join("binding.json"),
        &json!({"schema":1,"protocol":4,"installation_id":deployment.installation_id,"package_sha256":plan.manifest.sha256()}),
        &private.clone().with_account(&worker.sid, false)?,
    )?;
    let entry = &plan.manifest.manifest().entrypoints;
    let host_public =
        super::install_ssh::configure(&root, &package, &deployment, enrollment, entry, &code)?;
    let fingerprint = host_public.fingerprint()?;
    let management = deployment.account(Role::Management)?;
    crate::artifacts::io::write_new_json(&metadata.join("deployment.json"), &deployment, &private)?;
    deployment
        .verify(&root)
        .context("verify installed worker state before activation")?;
    crate::isolation::firewall::add(
        &deployment.installation_id,
        &package.join(&entry.sshd),
        enrollment.ssh_port,
        enrollment.management_peer,
    )
    .context("configure owned inbound SSH firewall")?;
    crate::artifacts::io::write_new_json(
        &metadata.join("firewall-created.json"),
        &json!({"name":pipe_transport::service_name(&deployment.installation_id)?}),
        &private,
    )?;
    super::install_service::register(&root, &deployment, &plan.manifest).with_context(|| {
        format!(
            "register and start owned worker service; startup diagnostic: {}",
            super::startup::failure_summary(
                &metadata.join("deployment.json"),
                &deployment.installation_id,
                plan.manifest.sha256()
            )
            .unwrap_or_else(|_| "current bounded diagnostic unavailable".into())
        )
    })?;
    // Candidate addresses are captured after the guest network state is final for
    // this install attempt; enumeration is best effort and never fails the install.
    let candidates = network::receipt_candidates(&network::SystemAdapters);
    let receipt = ConnectionReceipt {
        schema: 1,
        receipt_version: RECEIPT_VERSION,
        installed: true,
        installation_id: deployment.installation_id.clone(),
        environment_id: enrollment.environment_id.clone(),
        generation: enrollment.generation,
        manifest_sha256: plan.manifest.sha256().into(),
        ssh_user: management.name.clone(),
        ssh_port: enrollment.ssh_port,
        host_key: host_public,
        host_fingerprint: fingerprint,
        candidate_addresses: candidates,
        production_isolation_verified: false,
    };
    crate::artifacts::io::write_new_json(&root.join("connection.json"), &receipt, &code)?;
    Ok(receipt)
}

/// Upgrade an existing verified installation through the offline SYSTEM boot
/// path. The old VM base is the only rollback boundary: any interrupted mutation
/// fails this attempt and the controller must restore that base, not resume a
/// partially updated guest in place.
fn upgrade(root: PathBuf, plan: Plan) -> Result<ConnectionReceipt> {
    security::require_system()
        .context("offline worker upgrade requires the reviewed SYSTEM boot path")?;
    let metadata = root.join("metadata");
    let deployment_path = metadata.join("deployment.json");
    let (current, _) = deployment::load(&deployment_path)?;
    let previous = current.verify_for_upgrade(&root)?;
    let key = PublicKey::from_hex(&current.controller_public_key)?;
    let current_enrollment = deployment::read_enrollment(&metadata, &key)?;
    let authority = Authority::load(&metadata, &AclPolicy::private_vm())?;
    authority.verify_active(&metadata, &current_enrollment, &AclPolicy::private_vm())?;
    let highest = authority.highest(&metadata, &AclPolicy::private_vm())?;
    plan.enrollment.authorize_after(&highest)?;
    let next_version = semver::Version::parse(&plan.manifest.manifest().version)?;
    let current_version = semver::Version::parse(&previous.manifest().version)?;
    ensure!(
        next_version > current_version,
        "offline worker upgrade requires a newer signed package version"
    );
    let incoming = plan.enrollment.enrollment();
    let active = current_enrollment.enrollment();
    ensure!(
        incoming.management_peer == active.management_peer
            && incoming.ssh_port == active.ssh_port
            && incoming.ssh_public_key == active.ssh_public_key,
        "worker upgrade cannot change the enrolled SSH connection"
    );
    let signed = SignedEnrollment::read(&plan.base.join("enrollment"), &key)?;
    ensure!(
        signed.enrollment.canonical_bytes()? == incoming.canonical_bytes()?,
        "upgrade enrollment changed after review"
    );
    let next = Deployment {
        schema: 1,
        installation_id: current.installation_id.clone(),
        controller_public_key: current.controller_public_key.clone(),
        controller_version: plan.setup.controller_version.clone(),
        active_manifest_sha256: plan.manifest.sha256().into(),
        accounts: current.accounts.clone(),
    };
    let code = current.code_policy()?;
    staging::stage_payload(
        &plan.base.join("payload"),
        &next.package(&root),
        &key,
        &Compatibility {
            runtime_protocol: 4,
            controller_version: &next.controller_version,
        },
        &code,
    )?;

    // Stage every signed byte before admitting the new generation. From this
    // point onward a failure requires VM-base rollback because the old active
    // enrollment is intentionally no longer the accepted high-water mark.
    let private = AclPolicy::private_vm();
    crate::artifacts::io::write_new_json(
        &metadata.join(format!("recovery-accepted-{}.json", incoming.generation)),
        &signed,
        &private,
    )?;
    install_service::stop_for_upgrade(&root, &current, &previous)?;
    for name in ["enrollment.json", "enrollment.sig"] {
        let bytes =
            crate::artifacts::io::read_bytes(&plan.base.join("enrollment").join(name), 16 * 1024)?;
        private.replace(&metadata.join(name), &bytes)?;
    }
    let worker = next.account(Role::Worker)?;
    crate::artifacts::io::replace_json_with_parent(
        &root.join("binding.json"),
        &json!({"schema":1,"protocol":4,"installation_id":next.installation_id,"package_sha256":plan.manifest.sha256()}),
        &private.clone().with_account(&worker.sid, false)?,
        &code,
    )?;
    crate::artifacts::io::replace_json(&deployment_path, &next, &private)?;
    next.verify(&root)
        .context("verify upgraded worker state before service activation")?;

    // SSH uses a stable ForceCommand path, so replace and hash-check that relay
    // as well as the SCM service. The program-scoped firewall must move to the
    // newly verified sshd executable without adopting a foreign rule.
    let entry = &plan.manifest.manifest().entrypoints;
    let relay_bytes = crate::artifacts::io::read_bytes(
        &next.package(&root).join(&entry.worker),
        64 * 1024 * 1024,
    )?;
    ensure!(
        hex::encode(Sha256::digest(&relay_bytes))
            == plan.manifest.manifest().files[&entry.worker].sha256,
        "staged SSH relay differs from its signed worker payload"
    );
    code.replace(&root.join("bootstrap/wukong-worker.exe"), &relay_bytes)?;
    crate::isolation::firewall::replace(
        &next.installation_id,
        &current
            .package(&root)
            .join(&previous.manifest().entrypoints.sshd),
        &next.package(&root).join(&entry.sshd),
        incoming.ssh_port,
        incoming.management_peer,
    )?;
    install_service::activate_upgrade(&root, &next, &plan.manifest)?;

    let mut receipt = ConnectionReceipt::parse(&crate::artifacts::io::read_bytes(
        &root.join("connection.json"),
        wukong_worker_package::MAX_RECEIPT_BYTES as u64,
    )?)?;
    ensure!(
        receipt.installation_id == next.installation_id
            && receipt.environment_id == incoming.environment_id,
        "installed connection receipt differs from the upgraded identity"
    );
    receipt.generation = incoming.generation;
    receipt.manifest_sha256 = plan.manifest.sha256().into();
    receipt.candidate_addresses = network::receipt_candidates(&network::SystemAdapters);
    receipt.validate()?;
    crate::artifacts::io::replace_json(&root.join("connection.json"), &receipt, &code)?;
    Ok(receipt)
}
