use super::*;
use std::collections::BTreeMap;
use wukong_worker_package::{
    Component, EntryPoints, FileEntry, Manifest, PayloadKind, Signer, seal_package,
};

pub(crate) fn inert_package(root: &Path, signer: &Signer) -> Result<()> {
    fs::create_dir(root)?;
    let entry = |kind| FileEntry {
        sha256: hex::encode(Sha256::digest(b"fixture")),
        size: 7,
        kind,
    };
    let mut files = BTreeMap::new();
    for name in [
        "worker.exe",
        "python.exe",
        "dbgeng-helper.exe",
        "dbgeng.dll",
        "sshd.exe",
        "ssh-keygen.exe",
    ] {
        files.insert(name.into(), entry(PayloadKind::Runtime));
    }
    files.insert("bridge.py".into(), entry(PayloadKind::WorkerSource));
    files.insert("lock.json".into(), entry(PayloadKind::Data));
    files.insert(
        format!("long/{}/payload.bin", "x".repeat(190)),
        entry(PayloadKind::Data),
    );
    files.insert("licenses/LICENSE".into(), entry(PayloadKind::License));
    let mut components = BTreeMap::new();
    for name in [
        "python",
        "debugging_tools",
        "openssh",
        "worker_python_dependencies",
    ] {
        let path = format!("originals/{name}");
        files.insert(path.clone(), entry(PayloadKind::OriginalInstaller));
        components.insert(
            name.into(),
            Component {
                version: "fixture".into(),
                channel: "stable".into(),
                original_payloads: vec![path],
                licenses: vec!["licenses/LICENSE".into()],
            },
        );
    }
    for name in files.keys() {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, b"fixture")?;
    }
    let manifest = Manifest {
        schema: 1,
        platform: "windows-amd64".into(),
        version: "1.0.0".into(),
        source_revision: "a".repeat(40),
        toolchain_lock_sha256: hex::encode(Sha256::digest(b"fixture")),
        toolchain_lock_path: "lock.json".into(),
        runtime_protocol: 3,
        minimum_controller_version: "1.0.0".into(),
        entrypoints: EntryPoints {
            worker: "worker.exe".into(),
            python: "python.exe".into(),
            bridge: "bridge.py".into(),
            dbgeng_helper: Some("dbgeng-helper.exe".into()),
            windbg_mcp: None,
            dbgeng_library: "dbgeng.dll".into(),
            sshd: "sshd.exe".into(),
            ssh_keygen: "ssh-keygen.exe".into(),
        },
        components,
        license_notices: vec!["licenses/LICENSE".into()],
        files,
    };
    seal_package(
        root,
        &manifest,
        signer,
        &Compatibility {
            runtime_protocol: 3,
            controller_version: "1.0.0",
        },
    )?;
    Ok(())
}

#[test]
fn native_staging_creates_protected_exact_copies_without_overwrite_or_execution() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let source = temporary.path().join("source");
    let destination = temporary.path().join("stage");
    let (signer, _) = Signer::generate()?;
    inert_package(&source, &signer)?;
    let expected = Compatibility {
        runtime_protocol: 3,
        controller_version: "1.0.0",
    };
    let policy = AclPolicy::current_user()?;
    let copied = stage_payload(
        &source,
        &destination,
        &signer.public_key(),
        &expected,
        &policy,
    )?;
    copied.verify_tree(&destination)?;
    policy.verify(&destination, true)?;
    policy.verify(&destination.join("worker.exe"), false)?;
    assert!(
        stage_payload(
            &source,
            &destination,
            &signer.public_key(),
            &expected,
            &policy
        )
        .is_err()
    );
    assert_eq!(fs::read(destination.join("worker.exe"))?, b"fixture");
    fs::write(source.join("worker.exe"), b"changed")?;
    let rejected = temporary.path().join("rejected");
    assert!(stage_payload(&source, &rejected, &signer.public_key(), &expected, &policy).is_err());
    assert!(!rejected.exists());
    Ok(())
}

#[test]
fn staging_checks_held_source_identity_before_publishing_and_excludes_other_writers() -> Result<()>
{
    let temporary = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    let source = temporary.path().join("source.bin");
    let target = temporary.path().join("published.bin");
    let entry = FileEntry {
        sha256: hex::encode(Sha256::digest(b"fixture")),
        size: 7,
        kind: PayloadKind::Data,
    };
    fs::write(&source, b"private")?;
    assert!(copy_member(&source, &target, &entry, &policy).is_err());
    assert!(
        !target.exists(),
        "unverified content must not be exposed before rejection"
    );
    fs::write(&source, b"fixture")?;
    let writer = fs::OpenOptions::new().write(true).open(&source)?;
    assert!(copy_member(&source, &target, &entry, &policy).is_err());
    assert!(!target.exists());
    drop(writer);
    let held = source_file(&source)?;
    assert!(fs::OpenOptions::new().write(true).open(&source).is_err());
    assert!(fs::rename(&source, temporary.path().join("moved.bin")).is_err());
    drop(held);
    let alias = temporary.path().join("source-alias.bin");
    fs::hard_link(&source, &alias)?;
    assert!(copy_member(&source, &target, &entry, &policy).is_err());
    assert!(!target.exists());
    fs::remove_file(alias)?;
    copy_member(&source, &target, &entry, &policy)?;
    assert_eq!(fs::read(target)?, b"fixture");
    Ok(())
}

#[test]
#[ignore = "requires explicitly pinned, mounted read-only offline payload media; copies without execution"]
fn stages_explicit_read_only_offline_media_without_execution() -> Result<()> {
    let source = std::env::var_os("WUKONG_STAGE_TEST_SOURCE")
        .context("set WUKONG_STAGE_TEST_SOURCE to the mounted sealed payload")?;
    let source = std::path::PathBuf::from(source);
    let key = PublicKey::from_hex(&std::env::var("WUKONG_STAGE_TEST_PUBLIC_KEY")?)?;
    let version = std::env::var("WUKONG_STAGE_TEST_CONTROLLER_VERSION")?;
    ensure!(
        fs::metadata(source.join("manifest.json"))?
            .permissions()
            .readonly(),
        "explicit media fixture must be read-only"
    );
    let temporary = tempfile::tempdir()?;
    let destination = temporary.path().join("stage");
    let verified = stage_payload(
        &source,
        &destination,
        &key,
        &Compatibility {
            runtime_protocol: 3,
            controller_version: &version,
        },
        &AclPolicy::current_user()?,
    )?;
    verified.verify_tree(&destination)?;
    Ok(())
}

#[test]
fn native_permissions_reject_extra_writers_and_relative_roots() -> Result<()> {
    use std::{os::windows::process::CommandExt, process::Command};
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("private");
    let policy = AclPolicy::current_user()?;
    policy.create_directory(&root)?;
    let path = root.join("record.json");
    let mut file = policy.create_file(&path)?;
    file.write_all(b"owned fixture")?;
    drop(file);
    policy.verify(&path, false)?;
    let result = Command::new("icacls.exe")
        .arg(&path)
        .args(["/grant", "*S-1-5-32-545:(W)"])
        .creation_flags(0x08000000)
        .output()?;
    ensure!(
        result.status.success(),
        "cannot prepare task-owned ACL fixture"
    );
    assert!(policy.verify(&path, false).is_err());
    policy.apply(&path, false)?;
    policy.verify(&path, false)?;
    assert_eq!(fs::read(&path)?, b"owned fixture");
    assert!(crate::security::no_links(Path::new("relative-root")).is_err());
    Ok(())
}
