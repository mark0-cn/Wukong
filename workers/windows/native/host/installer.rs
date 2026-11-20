//! Offline VM bootstrap. All network-capable asset preparation stays on the
//! controller; this module has no download, MSI custom-action or shell-script path.
use crate::{
    host::accounts::{self, Role},
    host::authority::{Authority, SignedEnrollment},
    host::deployment::{self, Deployment},
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
                runtime_protocol: 3,
                controller_version: &setup.controller_version,
            },
        )?;

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
            runtime_protocol: 3,
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
        &json!({"schema":1,"protocol":3,"installation_id":deployment.installation_id,"package_sha256":plan.manifest.sha256()}),
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
