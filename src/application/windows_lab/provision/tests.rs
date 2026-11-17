//! Pure offline-provisioning helpers: VMX planning, hive scripts, media and
//! receipt verification. These tests never touch VMware or a guest disk.
use super::fixture;
use super::*;

fn snapshot_names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

#[test]
fn provisioning_requires_a_snapshot_free_vm() {
    assert_eq!(
        plan_snapshot(&[], "baseline", false).unwrap(),
        SnapshotPlan::Pristine
    );
    // `--replace-baseline` without an existing snapshot is a documented no-op:
    // the pristine flow creates the baseline after the guest receipt verifies.
    assert_eq!(
        plan_snapshot(&[], "baseline", true).unwrap(),
        SnapshotPlan::Pristine
    );
    assert_eq!(
        plan_snapshot(&snapshot_names(&["baseline"]), "baseline", true).unwrap(),
        SnapshotPlan::ReplaceBaseline
    );
    for (snapshots, replace) in [
        (snapshot_names(&["baseline"]), false),
        (snapshot_names(&["baseline", "lab-1"]), true),
        (snapshot_names(&["lab-1"]), false),
        (snapshot_names(&["other"]), true),
    ] {
        let error = plan_snapshot(&snapshots, "baseline", replace).unwrap_err();
        assert!(format!("{error:#}").contains("snapshot"), "{error:#}");
    }
}

#[test]
fn guest_paths_are_fixed_and_translate_to_libguestfs() {
    let id = "a".repeat(32);
    let paths = GuestPaths::for_provision(&id).unwrap();
    assert_eq!(
        paths.wrapper,
        format!(r"C:\ProgramData\WukongLab\provision\{id}\provision.ps1")
    );
    assert_eq!(paths.service, format!("WukongLabProvision-{id}"));
    assert_eq!(
        GuestPaths::guestfish(&paths.plan).unwrap(),
        format!("/ProgramData/WukongLab/provision/{id}/provision-plan.json")
    );
    assert!(GuestPaths::guestfish(r"D:\outside").is_err());
    assert!(GuestPaths::for_provision("short").is_err());
    let image = service_image_path(&paths.wrapper).unwrap();
    assert!(image.starts_with(POWERSHELL));
    assert!(image.ends_with(&format!("-File {}", paths.wrapper)));
    assert!(service_image_path(r"D:\rogue\provision.ps1").is_err());
}

/// The offline disk adapter converts exactly once, at its boundary. Requests
/// therefore must carry absolute Windows guest paths, not libguestfs paths.
#[test]
fn provision_requests_carry_windows_guest_paths_that_convert_once() {
    let paths = GuestPaths::for_provision(&"e".repeat(32)).unwrap();
    let inspection = DiskInspectionRequest::new();
    assert_eq!(inspection.system_hive, GUEST_SYSTEM_HIVE);
    assert_eq!(inspection.program_data, GUEST_PROGRAM_DATA);
    assert_eq!(inspection.provision_root, GUEST_PROVISION_BASE);
    assert_eq!(
        GuestPaths::guestfish(&inspection.system_hive).unwrap(),
        "/Windows/System32/config/SYSTEM"
    );
    assert_eq!(
        GuestPaths::guestfish(&inspection.program_data).unwrap(),
        "/ProgramData"
    );
    assert_eq!(
        GuestPaths::guestfish(&inspection.provision_root).unwrap(),
        "/ProgramData/WukongLab"
    );
    assert_eq!(
        GuestPaths::guestfish(GUEST_SYSTEM_CONFIG).unwrap(),
        "/Windows/System32/config"
    );

    // Hive patch and observation requests reach the adapter exactly like the
    // inspection request: every guest path is absolute `C:\` form.
    let patch = HivePatch {
        system_hive: GUEST_SYSTEM_HIVE.into(),
        control_set: 1,
        script: String::new(),
        verify_script: String::new(),
        host_backup: None,
    };
    assert_eq!(
        GuestPaths::guestfish(&patch.system_hive).unwrap(),
        "/Windows/System32/config/SYSTEM"
    );
    let observation = GuestObservationRequest {
        system_hive: GUEST_SYSTEM_HIVE.into(),
        control_set: 1,
        service_name: paths.service.clone(),
        wrapper: paths.wrapper.clone(),
        plan: paths.plan.clone(),
        receipt: paths.receipt.clone(),
    };
    let id = "e".repeat(32);
    for (windows, guest) in [
        (
            observation.system_hive,
            "/Windows/System32/config/SYSTEM".to_owned(),
        ),
        (
            observation.wrapper,
            format!("/ProgramData/WukongLab/provision/{id}/provision.ps1"),
        ),
        (
            observation.plan,
            format!("/ProgramData/WukongLab/provision/{id}/provision-plan.json"),
        ),
        (
            observation.receipt,
            format!("/ProgramData/WukongLab/provision/{id}/receipt.json"),
        ),
    ] {
        assert_eq!(GuestPaths::guestfish(&windows).unwrap(), guest);
        assert_eq!(GuestPaths::windows(&guest).unwrap(), windows);
    }
}

#[test]
fn guest_path_conversion_matrix_round_trips_and_rejects_escapes() {
    for windows in [
        r"C:\Windows\System32\config\SYSTEM",
        r"C:\ProgramData\WukongLab",
        r"C:\Program Files\Wukong Worker\setup.exe",
    ] {
        let guest = GuestPaths::guestfish(windows).unwrap();
        assert!(guest.starts_with('/'));
        assert_eq!(GuestPaths::windows(&guest).unwrap(), windows);
    }
    assert_eq!(
        GuestPaths::guestfish(r"C:\Windows\System32\config\SYSTEM").unwrap(),
        "/Windows/System32/config/SYSTEM"
    );
    assert_eq!(
        GuestPaths::windows("/Windows/System32/config/SYSTEM").unwrap(),
        r"C:\Windows\System32\config\SYSTEM"
    );

    // Relative paths, other drives and UNC paths never reach libguestfs.
    for windows in [
        r"Windows\System32",
        "relative",
        r"D:\Windows",
        r"\\server\share\SYSTEM",
        r"\\?\C:\Windows\SYSTEM",
        r"C:relative\SYSTEM",
    ] {
        assert!(
            GuestPaths::guestfish(windows).is_err(),
            "guestfish accepted {windows}"
        );
    }
    // The inverse conversion has the same contract: only one absolute POSIX
    // path form is accepted, and nothing may escape the guest volume.
    for guest in [
        "relative",
        "C:\\Windows",
        r"\Windows",
        "//server/share/SYSTEM",
        r"/Windows\System32",
    ] {
        assert!(
            GuestPaths::windows(guest).is_err(),
            "windows accepted {guest}"
        );
    }
    // Traversal, empty components and separator/stream aliases are rejected in
    // both directions.
    for windows in [
        r"C:\Windows\..\..\SYSTEM",
        r"C:\Windows\.\SYSTEM",
        r"C:\Windows\\System32\SYSTEM",
        r"C:\Windows/System32/SYSTEM",
        r"C:\Windows\System32\SYSTEM:stream",
        r"C:\",
    ] {
        let error = GuestPaths::guestfish(windows).unwrap_err();
        assert!(
            format!("{error:#}").contains("guest path"),
            "{windows}: {error:#}"
        );
    }
    for guest in [
        "/Windows/../SYSTEM",
        "/Windows/./SYSTEM",
        "/Windows//System32",
        "/Windows/System32/",
        "/.",
        "/",
    ] {
        assert!(
            GuestPaths::windows(guest).is_err(),
            "windows accepted {guest}"
        );
    }
}

#[test]
fn one_shot_hive_edit_is_fixed_and_verifiable() {
    let script = hive_patch_script(1, "WukongLabProvision-abc", "C:\\x\\provision.ps1").unwrap();
    assert!(script.contains("cd \\ControlSet001\\Services\nadd WukongLabProvision-abc\n"));
    assert!(script.contains("setval 6\nType\ndword:16\nStart\ndword:00000002\n"));
    assert!(script.contains("ObjectName\nstring:LocalSystem\n"));
    assert!(script.contains("commit\nquit\n"));
    assert!(hive_patch_script(0, "x", "C:\\x").is_err());
    assert!(hive_patch_script(1, "bad/name", "C:\\x").is_err());
    assert!(hive_patch_script(1, "ok", "\"quoted\"").is_err());

    let verification = "cd \\ControlSet001\\Services\\WukongLabProvision-abc\nlsval\nquit\n";
    let service = "WukongLabProvision-abc";
    assert_eq!(hive_verify_script(1, service).unwrap(), verification);
    let output = "\"Type\"=dword:00000010\n\"Start\"=dword:00000002\n\"ErrorControl\"=dword:00000001\n\"ImagePath\"=str(2):\"C:\\\\x\\\\provision.ps1\"\n\"ObjectName\"=\"LocalSystem\"\n";
    verify_hive_output(output, "C:\\x\\provision.ps1").unwrap();
    assert!(verify_hive_output(output, "C:\\other").is_err());
    assert!(verify_hive_output("\"Type\"=dword:00000010", "C:\\x").is_err());
}

#[test]
fn guest_disk_is_resolved_from_the_registered_vmx() {
    let directory = tempfile::tempdir().unwrap();
    let vmx = directory.path().join("vm.vmx");
    let disk = directory.path().join("Windows 10 x64.vmdk");
    fs::write(&disk, b"fixture disk").unwrap();
    fs::write(&vmx, "config").unwrap();
    for value in [
        "\"Windows 10 x64.vmdk\"".to_string(),
        format!("\"{}\"", disk.to_string_lossy()),
    ] {
        let resolved = resolve_guest_disk(
            &format!("nvme0:0.fileName = {value}\nnvme0:0.present = \"TRUE\"\n"),
            &vmx,
        )
        .unwrap();
        assert_eq!(resolved.device, "nvme0:0");
        assert_eq!(resolved.path, fs::canonicalize(&disk).unwrap());
    }
    assert!(resolve_guest_disk("sata0:0.present = \"TRUE\"\n", &vmx).is_err());
    assert!(resolve_guest_disk("nvme0:0.fileName = \"../escape.vmdk\"\n", &vmx).is_err());
    fs::write(directory.path().join("base-000001.vmdk"), b"delta").unwrap();
    assert!(
        resolve_guest_disk("nvme0:0.fileName = \"base-000001.vmdk\"\n", &vmx)
            .unwrap_err()
            .to_string()
            .contains("delta")
    );
}

#[test]
fn iso_attachment_uses_a_dedicated_cd_device() {
    let iso = Path::new(r"D:\vm\lab\offline.iso");
    let vmx = "config.version = \"8\"\nsata0.present = \"TRUE\"\nsata0:0.fileName = \"disk.vmdk\"\nsata0:1.deviceType = \"cdrom-image\"\nsata0:1.present = \"FALSE\"\nsata0:1.fileName = \"\"\n";
    let edit = attach_offline_iso(vmx, iso).unwrap();
    assert_eq!(edit.device, "sata0:1");
    assert!(edit.text.contains("sata0:1.present = \"TRUE\""));
    assert!(
        edit.text
            .contains("sata0:1.fileName = \"D:\\vm\\lab\\offline.iso\"")
    );
    assert!(edit.text.contains("sata0:1.deviceType = \"cdrom-image\""));
    assert!(edit.text.contains("sata0:1.startConnected = \"TRUE\""));
    assert!(edit.text.contains("sata0:0.fileName = \"disk.vmdk\""));

    let edit = attach_offline_iso(
        "sata0.present = \"TRUE\"\nsata0:0.fileName = \"disk.vmdk\"\n",
        iso,
    )
    .unwrap();
    assert_eq!(edit.device, "sata0:1");
    assert!(edit.text.ends_with("sata0:1.startConnected = \"TRUE\"\n"));

    let crlf = "sata0.present = \"TRUE\"\r\nsata0:0.fileName = \"disk.vmdk\"\r\n";
    let edit = attach_offline_iso(crlf, iso).unwrap();
    assert!(edit.text.contains("\r\n"));
    assert!(!edit.text.contains("\n\n"));

    assert!(attach_offline_iso("config.version = \"8\"\n", iso).is_err());
    assert!(attach_offline_iso("sata0.present = \"TRUE\"\n", Path::new("relative.iso")).is_err());
}

#[test]
fn finalized_media_is_re_verified_byte_by_byte() {
    let fixture = fixture::media();
    let identity = verify_media(&fixture.root).unwrap();
    assert_eq!(identity, fixture.identity);

    fs::write(fixture.root.join("WukongWorkerSetup.exe"), b"tampered").unwrap();
    let error = verify_media(&fixture.root).unwrap_err();
    assert!(
        format!("{error:#}").contains("setup executable differs"),
        "{error:#}"
    );
}

#[test]
fn media_verification_rejects_missing_records_and_links() {
    let empty = tempfile::tempdir().unwrap();
    assert!(verify_media(empty.path()).is_err());

    let fixture = fixture::media();
    fs::write(fixture.root.join("payload/manifest.json"), b"{}").unwrap();
    assert!(verify_media(&fixture.root).is_err());

    let fixture = fixture::media();
    fs::write(fixture.root.join("unexpected.bin"), b"x").unwrap();
    assert!(verify_media(&fixture.root).is_err());

    let fixture = fixture::media();
    std::fs::remove_dir_all(fixture.root.join("payload")).unwrap();
    assert!(verify_media(&fixture.root).is_err());
}

#[cfg(windows)]
#[test]
fn media_verification_rejects_reparse_points() {
    let fixture = fixture::media();
    let target = tempfile::tempdir().unwrap();
    std::fs::remove_dir_all(fixture.root.join("payload")).unwrap();
    junction::create(target.path(), fixture.root.join("payload")).unwrap();
    let error = verify_media(&fixture.root).unwrap_err();
    assert!(
        format!("{error:#}").contains("link") || format!("{error:#}").contains("reparse"),
        "{error:#}"
    );
}

#[test]
fn finalize_spec_validation_matches_the_media_contract() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("spec.json");
    let valid = serde_json::json!({
        "schema": 1,
        "installation_id": "958ef753fc7646598935ce8f79c53940",
        "environment_id": "windows-vm",
        "generation": 64,
        "management_peer": "192.168.199.1",
        "ssh_port": 22268,
        "execution_boundary": "disposable_vm",
    });
    fs::write(&path, serde_json::to_vec(&valid).unwrap()).unwrap();
    let spec = read_finalize_spec(&path).unwrap();
    assert_eq!(spec.generation, 64);
    assert!(spec.ssh_public_key.is_none());

    let mut invalid = valid.clone();
    invalid["installation_id"] = serde_json::json!("A".repeat(32));
    fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(read_finalize_spec(&path).is_err());

    let mut invalid = valid.clone();
    invalid["management_peer"] = serde_json::json!("192.168.199.01");
    fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(read_finalize_spec(&path).is_err());

    let mut invalid = valid;
    invalid["execution_boundary"] = serde_json::json!("shared_host");
    fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(read_finalize_spec(&path).is_err());

    fs::write(&path, b"{\"schema\":1,\"unknown\":true}").unwrap();
    assert!(read_finalize_spec(&path).is_err());
}

#[test]
fn guest_receipt_binds_media_identity_and_cleanup() {
    let fixture = fixture::media();
    let provision_id = "b".repeat(32);
    let receipt = fixture::guest_receipt(&provision_id, &fixture.connection);
    let expectation = ReceiptExpectation {
        provision_id: provision_id.clone(),
        media: fixture.identity.clone(),
    };
    let connection = verify_guest_receipt(&receipt, &expectation).unwrap();
    assert_eq!(connection, fixture.connection);

    let mut tampered: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
    tampered["provision_id"] = serde_json::json!("c".repeat(32));
    assert!(verify_guest_receipt(&serde_json::to_vec(&tampered).unwrap(), &expectation).is_err());

    let mut tampered: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
    tampered["connection"]["installation_id"] = serde_json::json!("0".repeat(32));
    assert!(verify_guest_receipt(&serde_json::to_vec(&tampered).unwrap(), &expectation).is_err());

    let mut tampered: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
    tampered["connection"]["host_fingerprint"] = serde_json::json!("SHA256:wrong");
    assert!(verify_guest_receipt(&serde_json::to_vec(&tampered).unwrap(), &expectation).is_err());

    let mut tampered: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
    tampered["cleanup_scheduled"] = serde_json::json!(false);
    assert!(verify_guest_receipt(&serde_json::to_vec(&tampered).unwrap(), &expectation).is_err());

    let mut tampered: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
    tampered["exit_code"] = serde_json::json!(5);
    assert!(verify_guest_receipt(&serde_json::to_vec(&tampered).unwrap(), &expectation).is_err());

    assert!(verify_guest_receipt(&vec![b'x'; 1024 * 1024 + 1], &expectation).is_err());
}

#[test]
fn provision_plan_exposes_only_verified_identities() {
    let fixture = fixture::media();
    let paths = GuestPaths::for_provision(&"d".repeat(32)).unwrap();
    let plan = provision_plan(&"d".repeat(32), &fixture.identity, &paths).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&plan).unwrap();
    assert_eq!(value["installation_id"], fixture.identity.installation_id);
    assert_eq!(value["manifest_sha256"], fixture.identity.manifest_sha256);
    assert_eq!(value["media_label"], MEDIA_LABEL);
    assert_eq!(value["service_name"], paths.service);
    assert!(PROVISION_WRAPPER.contains("install-reviewed"));
    assert!(PROVISION_WRAPPER.contains("Get-FileHash"));
    assert!(!PROVISION_WRAPPER.contains("REPLACE"));
}

#[test]
fn host_key_pin_path_is_derived_from_the_installation() {
    let fixture = fixture::media();
    let state = Path::new(r"D:\controller\state");
    let pin = known_hosts_path(
        state,
        &fixture.identity.installation_id,
        &fixture.connection.host_key,
    )
    .unwrap();
    assert_eq!(pin.parent().unwrap(), state.join("environments"));
    assert!(
        pin.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("ssh-host-")
    );
    assert!(known_hosts_path(state, "short", &fixture.connection.host_key).is_err());
}

#[test]
fn streamed_hashing_is_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("blob.bin");
    fs::write(&path, vec![9_u8; 3 * 1024 * 1024]).unwrap();
    let (hash, size) = sha256_file(&path).unwrap();
    assert_eq!(size, 3 * 1024 * 1024);
    assert_eq!(hash, sha256(&fs::read(&path).unwrap()));
}
