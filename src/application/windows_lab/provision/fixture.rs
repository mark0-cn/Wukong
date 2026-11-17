//! Signed offline-media fixture for provisioning unit tests.
//!
//! The helper builds a schema-1 finalized media directory exactly as the public
//! media contract describes, using the shared worker package signing types. It
//! never touches a VM, a key store or the network.
use super::{MediaIdentity, PublicConnection, SshKey};
use base64::Engine;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use wukong_worker_package::{
    Component, Enrollment, EntryPoints, ExecutionBoundary, FileEntry, Manifest, PayloadKind,
    PublicKey, Signer, SshPublicKey, VerifiedEnrollment,
};

pub const CONTROLLER_VERSION: &str = "1.0.0";

pub struct MediaFixture {
    pub root: PathBuf,
    pub identity: MediaIdentity,
    pub connection: PublicConnection,
    _directory: tempfile::TempDir,
}

fn ssh_key() -> SshPublicKey {
    // Canonical Ed25519 public key bytes; the private half is not needed here.
    let mut blob = vec![0, 0, 0, 11];
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&[0, 0, 0, 32]);
    blob.extend_from_slice(&[7_u8; 32]);
    SshPublicKey {
        algorithm: "ssh-ed25519".into(),
        key: base64::engine::general_purpose::STANDARD.encode(blob),
    }
}

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn file(sha256: &str, size: usize, kind: PayloadKind) -> FileEntry {
    FileEntry {
        sha256: sha256.into(),
        size: size as u64,
        kind,
    }
}

/// Build one finalized media directory and its matching public connection.
pub fn media() -> MediaFixture {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("media");
    fs::create_dir_all(root.join("payload/runtime")).unwrap();
    fs::create_dir_all(root.join("enrollment")).unwrap();

    let setup = b"MZ-offline-worker-setup-fixture".to_vec();
    let (signer, document) = Signer::generate().unwrap();
    fs::write(directory.path().join("controller.p8"), document.as_ref()).unwrap();
    let public_key = signer.public_key().to_hex();
    let controller = PublicKey::from_hex(&public_key).unwrap();

    let toolchain = digest(&[7_u8; 16]);
    let mut files = BTreeMap::new();
    let mut components = BTreeMap::new();
    components.insert(
        "python".to_string(),
        Component {
            version: "3.12.7".into(),
            channel: "stable".into(),
            original_payloads: vec!["originals/python.zip".into()],
            licenses: vec!["licenses/python.txt".into()],
        },
    );
    components.insert(
        "debugging_tools".to_string(),
        Component {
            version: "10.0.26100".into(),
            channel: "stable".into(),
            original_payloads: vec!["originals/debugging-tools.msi".into()],
            licenses: vec!["licenses/debugging-tools.txt".into()],
        },
    );
    components.insert(
        "openssh".to_string(),
        Component {
            version: "9.5".into(),
            channel: "stable".into(),
            original_payloads: vec!["originals/openssh.msi".into()],
            licenses: vec!["licenses/openssh.txt".into()],
        },
    );
    components.insert(
        "windbg_mcp".to_string(),
        Component {
            version: "0.1.0".into(),
            channel: "stable".into(),
            original_payloads: vec!["originals/windbg-mcp.zip".into()],
            licenses: vec!["licenses/windbg-mcp.txt".into()],
        },
    );
    let entrypoints = EntryPoints {
        worker: "runtime/native/wukong-worker.exe".into(),
        python: "runtime/python/python.exe".into(),
        selftest: "runtime/python/selftest.py".into(),
        windbg_mcp: "runtime/windbg-mcp/windbg-mcp.exe".into(),
        dbgeng_library: "runtime/native/dbgeng.dll".into(),
        sshd: "runtime/openssh/sshd.exe".into(),
        ssh_keygen: "runtime/openssh/ssh-keygen.exe".into(),
    };
    let entries: Vec<(&str, usize, PayloadKind)> = vec![
        ("runtime/native/wukong-worker.exe", 32, PayloadKind::Runtime),
        ("runtime/native/dbgeng.dll", 16, PayloadKind::Runtime),
        ("runtime/python/python.exe", 16, PayloadKind::Runtime),
        ("runtime/python/selftest.py", 8, PayloadKind::PythonSource),
        (
            "runtime/windbg-mcp/windbg-mcp.exe",
            16,
            PayloadKind::Runtime,
        ),
        ("runtime/openssh/sshd.exe", 16, PayloadKind::Runtime),
        ("runtime/openssh/ssh-keygen.exe", 16, PayloadKind::Runtime),
        ("runtime/toolchain.lock.json", 16, PayloadKind::Data),
        ("originals/python.zip", 8, PayloadKind::OriginalInstaller),
        (
            "originals/debugging-tools.msi",
            8,
            PayloadKind::OriginalInstaller,
        ),
        ("originals/openssh.msi", 8, PayloadKind::OriginalInstaller),
        (
            "originals/windbg-mcp.zip",
            8,
            PayloadKind::OriginalInstaller,
        ),
        ("licenses/python.txt", 8, PayloadKind::License),
        ("licenses/debugging-tools.txt", 8, PayloadKind::License),
        ("licenses/openssh.txt", 8, PayloadKind::License),
        ("licenses/windbg-mcp.txt", 8, PayloadKind::License),
    ];
    for (path, size, kind) in &entries {
        let bytes = vec![7_u8; *size];
        files.insert((*path).to_string(), file(&digest(&bytes), *size, *kind));
        let target = root.join("payload").join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(target, &bytes).unwrap();
    }
    let manifest = Manifest {
        schema: 2,
        platform: "windows-amd64".into(),
        version: CONTROLLER_VERSION.into(),
        source_revision: "a".repeat(40),
        toolchain_lock_sha256: toolchain,
        toolchain_lock_path: "runtime/toolchain.lock.json".into(),
        runtime_protocol: 3,
        minimum_controller_version: CONTROLLER_VERSION.into(),
        entrypoints,
        components,
        license_notices: vec!["licenses/python.txt".into()],
        files,
    };
    let (manifest_bytes, manifest_signature) = signer.sign(&manifest).unwrap();
    fs::write(root.join("payload/manifest.json"), &manifest_bytes).unwrap();
    fs::write(
        root.join("payload/manifest.sig"),
        serde_json::to_vec(&manifest_signature).unwrap(),
    )
    .unwrap();
    let manifest_sha256 = digest(&manifest_bytes);

    let installation_id = "958ef753fc7646598935ce8f79c53940".to_string();
    let enrollment = Enrollment {
        schema: 1,
        generation: 64,
        installation_id: installation_id.clone(),
        environment_id: "windows-vm".into(),
        controller_public_key: public_key.clone(),
        payload_manifest_sha256: manifest_sha256.clone(),
        management_peer: "192.168.199.1".parse().unwrap(),
        ssh_port: 22268,
        ssh_public_key: ssh_key(),
        execution_boundary: ExecutionBoundary::DisposableVm,
    };
    let (enrollment_bytes, enrollment_signature) = enrollment.sign(&signer).unwrap();
    let verified =
        VerifiedEnrollment::verify(&enrollment_bytes, &enrollment_signature, &controller).unwrap();
    verified
        .bind_payload(
            &wukong_worker_package::VerifiedManifest::verify(
                &manifest_bytes,
                &manifest_signature,
                &controller,
            )
            .unwrap(),
        )
        .unwrap();
    fs::write(root.join("enrollment/enrollment.json"), &enrollment_bytes).unwrap();
    fs::write(
        root.join("enrollment/enrollment.sig"),
        serde_json::to_vec(&enrollment_signature).unwrap(),
    )
    .unwrap();
    fs::write(root.join("WukongWorkerSetup.exe"), &setup).unwrap();
    fs::write(
        root.join("setup.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "controller_public_key": public_key,
            "controller_version": CONTROLLER_VERSION,
        }))
        .unwrap(),
    )
    .unwrap();
    let media_record = serde_json::json!({
        "schema": 1,
        "version": CONTROLLER_VERSION,
        "controller_public_key": public_key,
        "manifest_sha256": manifest_sha256,
        "setup_sha256": digest(&setup),
        "enrollment_sha256": digest(&enrollment_bytes),
        "installation_id": installation_id,
        "environment_id": "windows-vm",
        "generation": 64,
        "source_revision": "a".repeat(40),
    });
    let media_record_bytes = serde_json::to_vec(&media_record).unwrap();
    fs::write(root.join("media.json"), &media_record_bytes).unwrap();
    let media_record_sha256 = digest(&media_record_bytes);

    let ssh_public_key = SshKey::from_public(&ssh_key()).unwrap();
    let identity = MediaIdentity {
        media_root: root.clone(),
        media_record_sha256,
        setup_sha256: digest(&setup),
        enrollment_sha256: digest(&enrollment_bytes),
        manifest_sha256,
        controller_public_key: public_key,
        controller_version: CONTROLLER_VERSION.into(),
        installation_id,
        environment_id: "windows-vm".into(),
        generation: 64,
        ssh_port: 22268,
        ssh_user: "wkgm958ef753fc764659".into(),
        management_peer: "192.168.199.1".into(),
        ssh_public_key: ssh_public_key.clone(),
    };
    let host_public = ssh_key();
    let connection = PublicConnection {
        schema: 1,
        installed: true,
        installation_id: identity.installation_id.clone(),
        environment_id: identity.environment_id.clone(),
        generation: identity.generation,
        manifest_sha256: identity.manifest_sha256.clone(),
        ssh_user: identity.ssh_user.clone(),
        ssh_port: identity.ssh_port,
        host_key: SshKey::from_public(&host_public).unwrap(),
        host_fingerprint: host_public.fingerprint().unwrap(),
        production_isolation_verified: false,
    };
    MediaFixture {
        root: identity.media_root.clone(),
        identity,
        connection,
        _directory: directory,
    }
}

/// A valid guest provisioning receipt for the fixture, pre-cleanup.
pub fn guest_receipt(provision_id: &str, connection: &PublicConnection) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema": 3,
        "provision_id": provision_id,
        "installed": true,
        "already_installed": false,
        "exit_code": 0,
        "connection": connection,
        "boot_task_removed": true,
        "launcher_deleted": true,
        "cleanup_scheduled": true,
        "error": null,
    }))
    .unwrap()
}
