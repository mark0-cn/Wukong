use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs};
use wukong_worker_package::{
    Compatibility, Component, EntryPoints, FileEntry, Manifest, PayloadKind, Signer,
    VerifiedManifest, validate_path,
};

fn fixture() -> Manifest {
    let entry = |kind| FileEntry {
        sha256: hex::encode(Sha256::digest(b"fixture")),
        size: 7,
        kind,
    };
    let mut files = BTreeMap::new();
    for path in [
        "worker.exe",
        "python/python.exe",
        "worker-dbgeng.exe",
        "debuggers/dbgeng.dll",
        "ssh/sshd.exe",
        "ssh/ssh-keygen.exe",
    ] {
        files.insert(path.into(), entry(PayloadKind::Runtime));
    }
    files.insert("runtime/bridge.py".into(), entry(PayloadKind::WorkerSource));
    files.insert("toolchain.lock.json".into(), entry(PayloadKind::Data));
    files.insert("licenses/NOTICE.txt".into(), entry(PayloadKind::License));
    let mut components = BTreeMap::new();
    for name in [
        "python",
        "debugging_tools",
        "openssh",
        "worker_python_dependencies",
    ] {
        let path = format!("payloads/{name}.bin");
        files.insert(path.clone(), entry(PayloadKind::OriginalInstaller));
        components.insert(
            name.into(),
            Component {
                version: "fixture-version".into(),
                channel: "stable".into(),
                original_payloads: vec![path],
                licenses: vec!["licenses/NOTICE.txt".into()],
            },
        );
    }
    Manifest {
        schema: 1,
        platform: "windows-amd64".into(),
        version: "1.0.0".into(),
        source_revision: "a".repeat(40),
        toolchain_lock_sha256: hex::encode(Sha256::digest(b"fixture")),
        toolchain_lock_path: "toolchain.lock.json".into(),
        runtime_protocol: 2,
        minimum_controller_version: "1.0.0".into(),
        entrypoints: EntryPoints {
            worker: "worker.exe".into(),
            python: "python/python.exe".into(),
            bridge: "runtime/bridge.py".into(),
            dbgeng_helper: "worker-dbgeng.exe".into(),
            dbgeng_library: "debuggers/dbgeng.dll".into(),
            sshd: "ssh/sshd.exe".into(),
            ssh_keygen: "ssh/ssh-keygen.exe".into(),
        },
        components,
        license_notices: vec!["licenses/NOTICE.txt".into()],
        files,
    }
}

#[test]
fn controller_keys_are_stable_private_and_fail_closed_on_tampering() -> anyhow::Result<()> {
    use wukong_worker_package::load_controller_signer;
    let temporary = tempfile::tempdir()?;
    let keys = temporary.path().join("controller");
    let signer = load_controller_signer(&keys)?;
    let repeated = load_controller_signer(&keys)?;
    assert_eq!(signer.public_key().to_hex(), repeated.public_key().to_hex());
    let stored = keys.join("signing-key.bin");
    #[cfg(windows)]
    assert!(
        Signer::from_pkcs8(&fs::read(&stored)?).is_err(),
        "key must be DPAPI protected on disk"
    );
    let (bytes, signature) = signer.sign(&fixture())?;
    VerifiedManifest::verify(&bytes, &signature, &repeated.public_key())?;
    fs::write(stored, b"damaged private key fixture")?;
    assert!(load_controller_signer(&keys).is_err());
    assert!(load_controller_signer(std::path::Path::new("relative-controller-keys")).is_err());
    let inherited = temporary.path().join("unsafe-existing");
    fs::create_dir(&inherited)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&inherited, fs::Permissions::from_mode(0o755))?;
    }
    assert!(load_controller_signer(&inherited).is_err());
    assert!(!inherited.join("signing-key.bin").exists());
    #[cfg(windows)]
    {
        use std::{os::windows::process::CommandExt, process::Command};
        let excessive = temporary.path().join("excessive-file-access");
        load_controller_signer(&excessive)?;
        let result = Command::new("icacls.exe")
            .arg(excessive.join("signing-key.bin"))
            .args(["/grant", "*S-1-5-32-545:(R)"])
            .creation_flags(0x08000000)
            .output()?;
        anyhow::ensure!(
            result.status.success(),
            "could not prepare task-owned ACL fixture"
        );
        assert!(load_controller_signer(&excessive).is_err());
    }
    Ok(())
}

#[test]
fn concurrent_key_creation_keeps_one_identity() -> anyhow::Result<()> {
    let temporary = tempfile::tempdir()?;
    let keys = temporary.path().join("controller");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let keys = keys.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || -> anyhow::Result<_> {
                barrier.wait();
                Ok(wukong_worker_package::load_controller_signer(&keys)?
                    .public_key()
                    .to_hex())
            })
        })
        .collect();
    let identities: std::collections::BTreeSet<_> = threads
        .into_iter()
        .map(|thread| thread.join().expect("key publication thread panicked"))
        .collect::<anyhow::Result<_>>()?;
    assert_eq!(identities.len(), 1);
    assert_eq!(fs::read_dir(keys)?.count(), 1);
    Ok(())
}

#[test]
fn controller_sealing_checks_payloads_before_signing_and_never_overwrites() -> anyhow::Result<()> {
    use wukong_worker_package::{load_controller_signer, seal_package};
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("package");
    fs::create_dir(&root)?;
    let signer = load_controller_signer(&temporary.path().join("controller"))?;
    let manifest = fixture();
    for name in manifest.files.keys() {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, b"fixture")?;
    }
    let expected = Compatibility {
        runtime_protocol: 2,
        controller_version: "1.0.0",
    };
    fs::write(root.join("worker.exe"), b"changed")?;
    assert!(seal_package(&root, &manifest, &signer, &expected).is_err());
    assert!(!root.join("manifest.json").exists());
    fs::write(root.join("worker.exe"), b"fixture")?;
    fs::write(root.join("unlisted-file"), b"fixture")?;
    assert!(seal_package(&root, &manifest, &signer, &expected).is_err());
    fs::remove_file(root.join("unlisted-file"))?;
    let sealed = seal_package(&root, &manifest, &signer, &expected)?;
    assert_eq!(
        sealed.signer_fingerprint(),
        signer.public_key().fingerprint()
    );
    let original = fs::read(root.join("manifest.sig"))?;
    assert!(seal_package(&root, &manifest, &signer, &expected).is_err());
    assert_eq!(original, fs::read(root.join("manifest.sig"))?);
    Ok(())
}

#[cfg(windows)]
#[test]
#[ignore = "requires WUKONG_PACKAGE_CLI pointing to the self-built controller package utility"]
fn native_controller_seals_with_persistent_private_keys() -> anyhow::Result<()> {
    use std::{os::windows::process::CommandExt, process::Command};
    let binary = std::env::var_os("WUKONG_PACKAGE_CLI")
        .ok_or_else(|| anyhow::anyhow!("set WUKONG_PACKAGE_CLI"))?;
    let temporary = tempfile::tempdir()?;
    let keys = temporary.path().join("controller");
    let package = temporary.path().join("payload");
    let draft = temporary.path().join("draft.json");
    let manifest = fixture();
    for name in manifest.files.keys() {
        let path = package.join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, b"fixture")?;
    }
    fs::write(&draft, serde_json::to_vec(&manifest)?)?;
    let public = Command::new(&binary)
        .arg("key")
        .arg(&keys)
        .creation_flags(0x08000000)
        .output()?;
    anyhow::ensure!(
        public.status.success(),
        "{}",
        String::from_utf8_lossy(&public.stderr)
    );
    let public: serde_json::Value = serde_json::from_slice(&public.stdout)?;
    let sealed = Command::new(&binary)
        .arg("seal")
        .arg(&package)
        .arg(&draft)
        .arg(&keys)
        .arg("1.0.0")
        .creation_flags(0x08000000)
        .output()?;
    anyhow::ensure!(
        sealed.status.success(),
        "{}",
        String::from_utf8_lossy(&sealed.stderr)
    );
    let public =
        wukong_worker_package::PublicKey::from_hex(public["public_key"].as_str().unwrap())?;
    wukong_worker_package::verify_package(
        &package,
        &public,
        &Compatibility {
            runtime_protocol: 2,
            controller_version: "1.0.0",
        },
    )?;
    let forbidden = Command::new(&binary)
        .arg("seal")
        .arg(&package)
        .arg(&draft)
        .arg(package.join("private-key"))
        .arg("1.0.0")
        .creation_flags(0x08000000)
        .output()?;
    assert!(!forbidden.status.success());
    assert!(!package.join("private-key").exists());
    Ok(())
}

#[test]
fn ed25519_pins_an_independent_key_and_rejects_tampering() -> anyhow::Result<()> {
    let (signer, _private) = Signer::generate()?;
    let (bytes, signature) = signer.sign(&fixture())?;
    let public = signer.public_key();
    let verified = VerifiedManifest::verify(&bytes, &signature, &public)?;
    assert_eq!(verified.signer_fingerprint(), public.fingerprint());
    assert_eq!(verified.sha256(), hex::encode(Sha256::digest(&bytes)));
    let mut tampered = bytes.clone();
    tampered[20] ^= 1;
    assert!(VerifiedManifest::verify(&tampered, &signature, &public).is_err());
    let (stranger, _) = Signer::generate()?;
    assert!(VerifiedManifest::verify(&bytes, &signature, &stranger.public_key()).is_err());
    let mut forged = signature.clone();
    forged.signature = "0".repeat(128);
    assert!(VerifiedManifest::verify(&bytes, &forged, &public).is_err());
    Ok(())
}

#[test]
fn incomplete_payloads_bad_licenses_collisions_and_protocols_fail_closed() -> anyhow::Result<()> {
    let original = fixture();
    original.check_compatibility(&Compatibility {
        runtime_protocol: 2,
        controller_version: "1.1.0",
    })?;
    assert!(
        original
            .check_compatibility(&Compatibility {
                runtime_protocol: 1,
                controller_version: "1.1.0"
            })
            .is_err()
    );
    assert!(
        original
            .check_compatibility(&Compatibility {
                runtime_protocol: 2,
                controller_version: "0.9.0"
            })
            .is_err()
    );
    let mut missing = original.clone();
    missing.components.remove("openssh");
    assert!(missing.validate().is_err());
    let mut missing = original.clone();
    missing.files.remove("payloads/debugging_tools.bin");
    assert!(missing.validate().is_err());
    let mut collision = original.clone();
    collision
        .files
        .insert("WORKER.EXE".into(), collision.files["worker.exe"].clone());
    assert!(collision.validate().is_err());
    let mut collision = original.clone();
    collision
        .files
        .insert("python".into(), collision.files["worker.exe"].clone());
    assert!(collision.validate().is_err());
    let mut license = original.clone();
    license.files.get_mut("licenses/NOTICE.txt").unwrap().kind = PayloadKind::Data;
    assert!(license.validate().is_err());
    let mut lock = original;
    lock.toolchain_lock_sha256 = "0".repeat(64);
    assert!(lock.validate().is_err());
    Ok(())
}

#[test]
fn windows_path_normalization_never_creates_package_aliases() {
    for invalid in [
        "../escape",
        "dir/../escape",
        "/absolute",
        "C:/absolute",
        "dir\\escape",
        "file.exe:stream",
        "CON.txt",
        "COM1",
        "LPT9.txt",
        "name.",
        "name ",
        "CON .txt",
        "a//b",
        "a/./b",
        "目录/file",
    ] {
        assert!(validate_path(invalid).is_err(), "{invalid}");
    }
    assert!(validate_path("payloads/X64 Debuggers And Tools-x64_en-us.msi").is_ok());
}

#[test]
fn signed_noncanonical_json_and_duplicate_map_keys_are_rejected() -> anyhow::Result<()> {
    let manifest = fixture();
    let (signer, private) = Signer::generate()?;
    let (bytes, signature) = signer.sign(&manifest)?;
    let key = ring::signature::Ed25519KeyPair::from_pkcs8(private.as_ref())
        .map_err(|_| anyhow::anyhow!("fixture key rejected"))?;
    let original = String::from_utf8(bytes)?;
    let duplicate = original.replace(
        "\"files\":{",
        &format!(
            "\"files\":{{\"worker.exe\":{},",
            serde_json::to_string(&manifest.files["worker.exe"])?
        ),
    );
    for noncanonical in [format!(" {original}"), duplicate] {
        let mut signature = signature.clone();
        signature.signature = hex::encode(key.sign(noncanonical.as_bytes()).as_ref());
        let error =
            VerifiedManifest::verify(noncanonical.as_bytes(), &signature, &signer.public_key())
                .err()
                .expect("noncanonical manifest was accepted");
        assert!(error.to_string().contains("not canonical"), "{error:#}");
    }
    Ok(())
}

#[cfg(windows)]
#[test]
fn windows_junction_roots_and_ancestors_are_rejected() -> anyhow::Result<()> {
    use std::{os::windows::process::CommandExt, process::Command};
    let temporary = tempfile::tempdir()?;
    let original = temporary.path().join("original");
    let alias = temporary.path().join("alias");
    fs::create_dir_all(original.join("nested"))?;
    let quote =
        |path: &std::path::Path| format!("'{}'", path.to_string_lossy().replace('\'', "''"));
    let script = format!(
        "New-Item -ItemType Junction -Path {} -Target {} -ErrorAction Stop | Out-Null",
        quote(&alias),
        quote(&original)
    );
    let result = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ])
        .creation_flags(0x08000000)
        .output()?;
    anyhow::ensure!(
        result.status.success(),
        "could not create task-owned junction fixture: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let (signer, _) = Signer::generate()?;
    let (bytes, signature) = signer.sign(&fixture())?;
    let verified = VerifiedManifest::verify(&bytes, &signature, &signer.public_key())?;
    for path in [&alias, &alias.join("nested")] {
        let error = verified
            .verify_tree(path)
            .expect_err("junction package root was accepted");
        assert!(
            error.to_string().contains("reparse")
                || error.to_string().contains("links are forbidden"),
            "{error:#}"
        );
    }
    // Remove the junction itself; never recursively traverse its target.
    fs::remove_dir(alias)?;
    Ok(())
}

#[test]
fn exact_signed_tree_detects_missing_added_changed_and_linked_content() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let manifest = fixture();
    let (signer, _) = Signer::generate()?;
    let (bytes, signature) = signer.sign(&manifest)?;
    let verified = VerifiedManifest::verify(&bytes, &signature, &signer.public_key())?;
    for name in manifest.files.keys() {
        let path = root.path().join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, b"fixture")?;
    }
    fs::write(root.path().join("manifest.json"), &bytes)?;
    fs::write(
        root.path().join("manifest.sig"),
        serde_json::to_vec(&signature)?,
    )?;
    fs::write(
        root.path().join("manifest.pub"),
        signer.public_key().to_hex(),
    )?;
    verified.verify_tree(root.path())?;
    wukong_worker_package::verify_package(
        root.path(),
        &signer.public_key(),
        &Compatibility {
            runtime_protocol: 2,
            controller_version: "1.0.0",
        },
    )?;
    fs::write(root.path().join("worker.exe"), b"changed")?;
    assert!(verified.verify_tree(root.path()).is_err());
    fs::write(root.path().join("worker.exe"), b"fixture")?;
    fs::write(root.path().join("unexpected.exe"), b"fixture")?;
    assert!(verified.verify_tree(root.path()).is_err());
    fs::remove_file(root.path().join("unexpected.exe"))?;
    fs::remove_file(root.path().join("payloads/python.bin"))?;
    assert!(verified.verify_tree(root.path()).is_err());
    fs::write(root.path().join("payloads/python.bin"), b"fixture")?;
    #[cfg(unix)]
    {
        fs::remove_file(root.path().join("worker.exe"))?;
        std::os::unix::fs::symlink("toolchain.lock.json", root.path().join("worker.exe"))?;
        assert!(verified.verify_tree(root.path()).is_err());
    }
    Ok(())
}

#[cfg(windows)]
#[test]
#[ignore = "requires WUKONG_WORKER_CLI pointing to the self-built native verifier"]
fn native_cli_checks_independent_trust_and_actual_file_bytes() -> anyhow::Result<()> {
    use std::{os::windows::process::CommandExt, process::Command};
    let binary = std::env::var_os("WUKONG_WORKER_CLI")
        .ok_or_else(|| anyhow::anyhow!("set WUKONG_WORKER_CLI to the self-built native worker"))?;
    let directory = tempfile::tempdir()?;
    let manifest = fixture();
    let (signer, _private) = Signer::generate()?;
    let (bytes, signature) = signer.sign(&manifest)?;
    for name in manifest.files.keys() {
        let path = directory.path().join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, b"fixture")?;
    }
    fs::write(directory.path().join("manifest.json"), bytes)?;
    fs::write(
        directory.path().join("manifest.sig"),
        serde_json::to_vec(&signature)?,
    )?;
    let invoke = |key: &str| {
        Command::new(&binary)
            .arg("verify-package")
            .arg(directory.path())
            .args([key, "1.0.0"])
            .creation_flags(0x08000000)
            .output()
    };
    let valid = invoke(&signer.public_key().to_hex())?;
    anyhow::ensure!(
        valid.status.success(),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&valid.stdout)?;
    assert_eq!(result["scope"], "signature_and_file_integrity");
    assert_eq!(result["signer_sha256"], signer.public_key().fingerprint());
    let (stranger, _) = Signer::generate()?;
    assert!(!invoke(&stranger.public_key().to_hex())?.status.success());
    fs::write(directory.path().join("worker.exe"), b"changed")?;
    let changed = invoke(&signer.public_key().to_hex())?;
    assert!(!changed.status.success());
    assert!(String::from_utf8_lossy(&changed.stderr).contains("hash mismatch"));
    Ok(())
}
