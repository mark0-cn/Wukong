//! Protected native installation state. Incoming SSH requests cannot choose
//! executables, trust roots, process credentials, or worker metadata paths.
use crate::artifacts::io::read_bytes;
use crate::{
    host::accounts::{Account, Role},
    host::authority::Authority,
    isolation::security::AclPolicy,
    rpc::pipe_transport::service_name,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};
use wukong_worker_package::{
    Compatibility, PublicKey, SignatureRecord, VerifiedEnrollment, VerifiedManifest, verify_package,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deployment {
    pub schema: u32,
    pub installation_id: String,
    pub controller_public_key: String,
    pub controller_version: String,
    pub active_manifest_sha256: String,
    pub accounts: Vec<Account>,
}
impl Deployment {
    pub fn account(&self, role: Role) -> Result<&Account> {
        self.accounts
            .iter()
            .find(|a| a.role == role)
            .context("missing owned installation account")
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema == 1, "unsupported native installation state");
        service_name(&self.installation_id)?;
        PublicKey::from_hex(&self.controller_public_key)?;
        ensure!(
            self.active_manifest_sha256.len() == 64
                && self
                    .active_manifest_sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid native active version"
        );
        ensure!(
            self.accounts.len() == 4,
            "incomplete owned installation accounts"
        );
        for role in [Role::Management, Role::Worker, Role::Debugger, Role::Target] {
            ensure!(
                self.accounts.iter().filter(|a| a.role == role).count() == 1,
                "duplicate or missing native account role"
            );
            self.account(role)?.validate(&self.installation_id)?;
        }
        let mut sids = self.accounts.iter().map(|a| &a.sid).collect::<Vec<_>>();
        sids.sort();
        sids.dedup();
        ensure!(sids.len() == 4, "worker accounts must have distinct SIDs");
        Ok(())
    }
    pub fn code_policy(&self) -> Result<AclPolicy> {
        self.accounts
            .iter()
            .try_fold(AclPolicy::code_vm(), |policy, a| {
                policy.with_account(&a.sid, false)
            })
    }
    pub fn data_policy(&self) -> Result<AclPolicy> {
        AclPolicy::private_vm().with_account(&self.account(Role::Worker)?.sid, true)
    }
    pub fn package(&self, root: &Path) -> PathBuf {
        root.join("versions").join(&self.active_manifest_sha256)
    }
    pub fn verify(&self, root: &Path) -> Result<VerifiedManifest> {
        self.validate()?;
        self.code_policy()?.verify(root, true)?;
        AclPolicy::private_vm().verify(&root.join("metadata"), true)?;
        self.data_policy()?.verify(&root.join("data"), true)?;
        let metadata = root.join("metadata");
        let private = AclPolicy::private_vm();
        private.verify(&metadata.join("system-tmp"), true)?;
        private.verify(&root.join("ssh"), true)?;
        for name in [
            "ssh_host_ed25519_key",
            "ssh_host_ed25519_key.pub",
            "sshd_config",
        ] {
            private.verify(&root.join("ssh").join(name), false)?;
        }
        private
            .clone()
            .with_account(&self.account(Role::Management)?.sid, false)?
            .verify(&root.join("bootstrap/authorized_keys"), false)?;
        let authority = Authority::load(&metadata, &private)?;
        let key = authority.key()?;
        ensure!(
            key.to_hex() == self.controller_public_key,
            "deployment key differs from the protected first-owner authority"
        );
        let package = self.package(root);
        self.code_policy()?.verify(&package, true)?;
        let verified = verify_package(
            &package,
            &key,
            &Compatibility {
                runtime_protocol: 3,
                controller_version: &self.controller_version,
            },
        )?;
        ensure!(
            verified.sha256() == self.active_manifest_sha256,
            "active package differs from protected state"
        );
        let enrollment = read_enrollment(&root.join("metadata"), &key)?;
        authority.verify_active(&metadata, &enrollment, &private)?;
        ensure!(
            enrollment.enrollment().installation_id == self.installation_id,
            "native enrollment installation changed"
        );
        // An accepted failed update can have a newer payload than the active
        // version. Activation/rollback receipts must authorize that case later.
        enrollment.bind_payload(&verified)?;
        Ok(verified)
    }
}
pub fn load(path: &Path) -> Result<(Deployment, PathBuf)> {
    AclPolicy::private_vm().verify(path, false)?;
    ensure!(
        path.file_name() == Some(std::ffi::OsStr::new("deployment.json")),
        "invalid native configuration name"
    );
    let metadata = path.parent().context("missing native metadata directory")?;
    ensure!(
        metadata.file_name() == Some(std::ffi::OsStr::new("metadata")),
        "invalid native metadata location"
    );
    let root = metadata
        .parent()
        .context("missing native installation root")?
        .to_owned();
    let deployment: Deployment = serde_json::from_slice(&read_bytes(path, 64 * 1024)?)?;
    deployment.validate()?;
    Ok((deployment, root))
}
pub fn read_enrollment(directory: &Path, key: &PublicKey) -> Result<VerifiedEnrollment> {
    let bytes = read_bytes(&directory.join("enrollment.json"), 16 * 1024)?;
    let signature: SignatureRecord =
        serde_json::from_slice(&read_bytes(&directory.join("enrollment.sig"), 1024)?)?;
    VerifiedEnrollment::verify(&bytes, &signature, key)
}
pub fn verify_running_binary(package: &Path, manifest: &VerifiedManifest) -> Result<()> {
    let executable = std::env::current_exe()?;
    let expected = package
        .join(&manifest.manifest().entrypoints.worker)
        .canonicalize()?;
    ensure!(
        executable.canonicalize()? == expected,
        "SCM worker must execute directly from its verified active version"
    );
    let bytes = read_bytes(&executable, 64 * 1024 * 1024)?;
    let expected = &manifest.manifest().files[&manifest.manifest().entrypoints.worker].sha256;
    ensure!(
        hex::encode(Sha256::digest(bytes)) == *expected,
        "running native service binary differs from signed payload"
    );
    Ok(())
}
pub fn worker_environment(root: &Path, package: &Path) -> Result<BTreeMap<String, OsString>> {
    let system = std::env::var_os("SystemRoot").context("Windows system directory is missing")?;
    let system_path = Path::new(&system);
    ensure!(system_path.is_absolute(), "invalid Windows system root");
    let path = std::env::join_paths([
        system_path.join("System32"),
        system_path.to_owned(),
        system_path.join("System32/WindowsPowerShell/v1.0"),
        package.join("runtime/openssh"),
    ])?;
    let temporary = root.join("data/tmp");
    ensure!(
        temporary.is_dir(),
        "private native worker temporary directory is missing"
    );
    Ok([
        ("SYSTEMROOT".into(), system.clone()),
        ("WINDIR".into(), system),
        ("PATH".into(), path),
        ("TEMP".into(), temporary.clone().into_os_string()),
        ("TMP".into(), temporary.into_os_string()),
        ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
        ("PYTHONUNBUFFERED".into(), "1".into()),
    ]
    .into_iter()
    .collect())
}
