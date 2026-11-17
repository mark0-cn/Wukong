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
        paths.startup_entry,
        format!("wukong-provision-{id}.cmd"),
        "the machine startup entry is the proven .cmd launcher layout"
    );
    assert_eq!(
        paths.startup_script,
        format!(
            r"C:\Windows\System32\GroupPolicy\Machine\Scripts\Startup\wukong-provision-{id}.cmd"
        )
    );
    assert_eq!(paths.scripts_ini, GUEST_SCRIPTS_INI);
    assert_eq!(
        paths.scripts_ini_backup,
        format!(r"C:\Windows\System32\GroupPolicy\Machine\Scripts\scripts.ini.wukong-lab-{id}.bak")
    );
    assert_eq!(paths.gpt_ini, GUEST_GPT_INI);
    assert_eq!(
        paths.gpt_ini_backup,
        format!(r"C:\Windows\System32\GroupPolicy\gpt.ini.wukong-lab-{id}.bak")
    );
    assert_eq!(paths.provision_id(), id);
    assert_eq!(
        GuestPaths::guestfish(&paths.plan).unwrap(),
        format!("/ProgramData/WukongLab/provision/{id}/provision-plan.json")
    );
    assert_eq!(
        GuestPaths::guestfish(&paths.startup_script).unwrap(),
        format!("/Windows/System32/GroupPolicy/Machine/Scripts/Startup/wukong-provision-{id}.cmd")
    );
    assert!(GuestPaths::guestfish(r"D:\outside").is_err());
    assert!(GuestPaths::for_provision("short").is_err());
    let launcher = String::from_utf8(startup_launcher(&id, &paths.wrapper).unwrap()).unwrap();
    assert!(launcher.starts_with("@echo off\r\n"));
    assert!(
        launcher.contains(&format!(
            "start \"\" /b \"{POWERSHELL}\" -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{}\"",
            paths.wrapper
        )),
        "{launcher}"
    );
    assert!(startup_launcher(&id, r"D:\rogue\provision.ps1").is_err());
}

/// The offline disk adapter converts exactly once, at its boundary. Requests
/// therefore must carry absolute Windows guest paths, not libguestfs paths.
#[test]
fn provision_requests_carry_windows_guest_paths_that_convert_once() {
    let id = "e".repeat(32);
    let paths = GuestPaths::for_provision(&id).unwrap();
    let inspection = DiskInspectionRequest::for_provision(&paths);
    assert_eq!(inspection.system_hive, GUEST_SYSTEM_HIVE);
    assert_eq!(inspection.program_data, GUEST_PROGRAM_DATA);
    assert_eq!(inspection.provision_root, GUEST_PROVISION_ROOT);
    assert_eq!(inspection.scripts_ini, GUEST_SCRIPTS_INI);
    assert_eq!(inspection.scripts_ini_backup, paths.scripts_ini_backup);
    assert_eq!(inspection.gpt_ini, GUEST_GPT_INI);
    assert_eq!(inspection.gpt_ini_backup, paths.gpt_ini_backup);
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
        "/ProgramData/WukongLab/provision"
    );
    assert_eq!(
        GuestPaths::guestfish(GUEST_SYSTEM_CONFIG).unwrap(),
        "/Windows/System32/config"
    );

    // Observation and legacy service removal requests reach the adapter exactly
    // like the inspection request: every guest path is absolute `C:\` form.
    let removal = ServiceRemoval {
        system_hive: GUEST_SYSTEM_HIVE.into(),
        service: paths.service.clone(),
    };
    assert_eq!(
        GuestPaths::guestfish(&removal.system_hive).unwrap(),
        "/Windows/System32/config/SYSTEM"
    );
    let observation = GuestObservationRequest::for_provision(&paths);
    for (windows, guest) in [
        (
            observation.wrapper.clone(),
            format!("/ProgramData/WukongLab/provision/{id}/provision.ps1"),
        ),
        (
            observation.plan.clone(),
            format!("/ProgramData/WukongLab/provision/{id}/provision-plan.json"),
        ),
        (
            observation.receipt.clone(),
            format!("/ProgramData/WukongLab/provision/{id}/receipt.json"),
        ),
        (
            observation.startup_script.clone(),
            format!(
                "/Windows/System32/GroupPolicy/Machine/Scripts/Startup/wukong-provision-{id}.cmd"
            ),
        ),
        (
            observation.scripts_ini.clone(),
            "/Windows/System32/GroupPolicy/Machine/Scripts/scripts.ini".to_owned(),
        ),
        (
            observation.scripts_ini_backup.clone(),
            format!(
                "/Windows/System32/GroupPolicy/Machine/Scripts/scripts.ini.wukong-lab-{id}.bak"
            ),
        ),
        (
            observation.gpt_ini_backup.clone(),
            format!("/Windows/System32/GroupPolicy/gpt.ini.wukong-lab-{id}.bak"),
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
fn machine_startup_policy_injection_preserves_existing_content() {
    let paths = GuestPaths::for_provision(&"b".repeat(32)).unwrap();
    let entry = paths.startup_entry.clone();
    // A fresh guest has no scripts.ini: create a minimal ASCII [Startup] section.
    let fresh = inject_startup_script(None, &entry).unwrap();
    let fresh_text = String::from_utf8(fresh.clone()).unwrap();
    assert_eq!(
        fresh_text,
        format!("[Startup]\r\n0CmdLine={entry}\r\n0Parameters=\r\n")
    );
    assert_eq!(
        startup_command_lines(&fresh).unwrap(),
        vec![entry.clone()],
        "the injected entry is readable again"
    );

    // The recovered working layout was ANSI `[Startup] 0CmdLine=...`; unrelated
    // entries and sections are preserved and our entry uses the next index.
    let existing = b"[Startup]\r\n0CmdLine=enable-admin.cmd\r\n0Parameters=\r\n\r\n[Shutdown]\r\n0CmdLine=cleanup.cmd\r\n0Parameters=\r\n";
    let injected = inject_startup_script(Some(existing), &entry).unwrap();
    let text = String::from_utf8(injected.clone()).unwrap();
    assert!(text.contains("0CmdLine=enable-admin.cmd\r\n0Parameters=\r\n"));
    assert!(text.contains(&format!("1CmdLine={entry}\r\n1Parameters=\r\n")));
    assert!(text.contains("[Shutdown]\r\n0CmdLine=cleanup.cmd\r\n0Parameters=\r\n"));
    assert_eq!(
        startup_command_lines(&injected).unwrap(),
        vec!["enable-admin.cmd".to_owned(), entry.clone()]
    );

    // UTF-16LE policy files keep their encoding and BOM.
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend(
        "[Startup]\r\n0CmdLine=enable-admin.cmd\r\n0Parameters=\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes),
    );
    let injected = inject_startup_script(Some(&utf16), &entry).unwrap();
    assert_eq!(&injected[..2], &[0xff, 0xfe]);
    assert!(startup_command_lines(&injected).unwrap().contains(&entry));

    // An already-injected provisioning entry is refused instead of stacked.
    assert!(inject_startup_script(Some(&injected), &entry).is_err());
    // Unsupported encodings and malformed entries fail closed.
    assert!(inject_startup_script(Some(&[0xff, 0x00, 0x01]), &entry).is_err());
    assert!(inject_startup_script(None, "wukong-provision-short.cmd").is_err());

    // Removal restores a pre-existing unrelated startup entry and drops the
    // [Startup] section only when this entry was the only content. The UTF-16LE
    // encoding of the removed document is preserved.
    let removed = remove_startup_script(&injected, &entry).unwrap().unwrap();
    assert_eq!(&removed[..2], &[0xff, 0xfe]);
    let entries = startup_command_lines(&removed).unwrap();
    assert_eq!(entries, vec!["enable-admin.cmd".to_owned()]);
    assert!(remove_startup_script(&fresh, &entry).unwrap().is_none());
}

#[test]
fn local_gpo_version_bump_preserves_the_user_half() {
    let created = bump_gpt_version(None).unwrap();
    assert_eq!(
        String::from_utf8(created).unwrap(),
        "[General]\r\nVersion=1\r\n"
    );
    // 65539 is 0x00010003: user version 1, computer version 3.
    let bumped = bump_gpt_version(Some(
        b"[General]\r\nVersion=65539\r\ngPCMachineExtensionNames=x\r\n",
    ))
    .unwrap();
    let text = String::from_utf8(bumped).unwrap();
    assert!(text.contains("Version=65540\r\n"), "{text}");
    assert!(text.contains("gPCMachineExtensionNames=x\r\n"), "{text}");

    // No [General] section and no Version: the section is appended, not guessed.
    let appended =
        String::from_utf8(bump_gpt_version(Some(b"[Other]\r\nA=1\r\n")).unwrap()).unwrap();
    assert!(appended.contains("[General]\r\nVersion=1\r\n"));
    assert!(appended.contains("[Other]\r\nA=1\r\n"));
    assert!(bump_gpt_version(Some(b"[General]\r\nVersion=nope\r\n")).is_err());
}

#[test]
fn legacy_service_removal_is_fixed_and_validated() {
    let script = hive_remove_service_script(1, "WukongLabProvision-abc").unwrap();
    assert_eq!(
        script,
        "cd \\ControlSet001\\Services\ndel WukongLabProvision-abc\ncommit\nquit\n"
    );
    assert!(hive_remove_service_script(0, "x").is_err());
    assert!(hive_remove_service_script(1, "bad/name").is_err());
    assert!(validate_service_name("bad name").is_err());
}

#[test]
fn cleanup_plan_removes_only_the_named_operation() {
    let paths = GuestPaths::for_provision(&"c".repeat(32)).unwrap();
    let other = "d".repeat(32);
    let mut policy = GuestPolicyFiles {
        scripts_ini: Some(
            format!(
                "[Startup]\r\n0CmdLine=other.cmd\r\n0Parameters=\r\n1CmdLine={}\r\n1Parameters=\r\n",
                paths.startup_entry
            )
            .into_bytes(),
        ),
        startup_command_lines: vec!["other.cmd".into(), paths.startup_entry.clone()],
        startup_directory_entries: vec![paths.startup_entry.clone()],
        ..Default::default()
    };
    let inspection = DiskInspection {
        windows_volume: true,
        system_hive_present: true,
        program_data_present: true,
        control_set: Some(1),
        existing_service: Some(paths.service.clone()),
        provision_root_entries: vec![paths.provision_id().to_owned(), other.clone()],
        policy: policy.clone(),
    };
    let plan = plan_guest_cleanup(&paths, &inspection).unwrap();
    assert_eq!(plan.files.len(), 1, "scripts.ini is rewritten in place");
    let rewritten = String::from_utf8(plan.files[0].bytes.clone()).unwrap();
    assert!(rewritten.contains("0CmdLine=other.cmd"));
    assert!(!rewritten.contains(&paths.startup_entry));
    assert_eq!(plan.files_to_remove, vec![paths.startup_script.clone()]);
    assert_eq!(plan.directories_to_remove, vec![paths.root.clone()]);
    assert_eq!(plan.service_key, Some(paths.service.clone()));
    assert!(
        !plan.actions.iter().any(|action| action.contains(&other)),
        "unrelated operation ids are never named: {plan:?}"
    );

    // A backup (when the previous scripts.ini existed) is restored verbatim and
    // both backup files are removed.
    policy.scripts_ini = Some(b"[Startup]\r\n0CmdLine=other.cmd\r\n0Parameters=\r\n".to_vec());
    policy.scripts_ini_backup = Some(b"previous scripts.ini".to_vec());
    policy.gpt_ini_backup = Some(b"previous gpt.ini".to_vec());
    let inspection = DiskInspection {
        existing_service: None,
        provision_root_entries: Vec::new(),
        policy,
        ..inspection
    };
    let plan = plan_guest_cleanup(&paths, &inspection).unwrap();
    assert_eq!(plan.files.len(), 2);
    assert!(plan.files.iter().any(
        |file| file.windows_path == paths.scripts_ini && file.bytes == b"previous scripts.ini"
    ));
    assert!(
        plan.files
            .iter()
            .any(|file| file.windows_path == paths.gpt_ini && file.bytes == b"previous gpt.ini")
    );
    assert!(plan.files_to_remove.contains(&paths.scripts_ini_backup));
    assert!(plan.files_to_remove.contains(&paths.gpt_ini_backup));
    assert!(plan.directories_to_remove.is_empty());
    assert!(plan.service_key.is_none());

    // A pristine disk produces no actions at all (idempotent cleanup).
    let pristine = DiskInspection {
        policy: GuestPolicyFiles::default(),
        existing_service: None,
        provision_root_entries: Vec::new(),
        ..inspection
    };
    assert!(plan_guest_cleanup(&paths, &pristine).unwrap().is_empty());
}

#[test]
fn leftover_detection_covers_every_injected_artifact() {
    let id = "f".repeat(32);
    let policy = GuestPolicyFiles {
        startup_command_lines: vec![format!("wukong-provision-{id}.cmd")],
        startup_directory_entries: vec![format!("wukong-provision-{id}.cmd")],
        scripts_directory_entries: vec![format!("scripts.ini.wukong-lab-{id}.bak")],
        group_policy_entries: vec![format!("gpt.ini.wukong-lab-{id}.bak")],
        ..Default::default()
    };
    assert_eq!(policy.leftover_operation_ids(), vec![id.clone()]);
    let inspection = DiskInspection {
        windows_volume: true,
        system_hive_present: true,
        program_data_present: true,
        control_set: Some(1),
        existing_service: Some(format!("WukongLabProvision-{id}")),
        provision_root_entries: vec![id.clone()],
        policy: GuestPolicyFiles::default(),
    };
    assert_eq!(inspection.leftover_operation_ids(), vec![id.clone()]);
    let pristine = DiskInspection {
        existing_service: None,
        provision_root_entries: Vec::new(),
        policy: GuestPolicyFiles::default(),
        ..inspection
    };
    assert!(pristine.leftover_operation_ids().is_empty());
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
    tampered["startup_cleanup"] = serde_json::json!("failed:busy");
    assert!(verify_guest_receipt(&serde_json::to_vec(&tampered).unwrap(), &expectation).is_err());

    let mut tampered: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
    tampered["startup_script_deleted"] = serde_json::json!(false);
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
    assert_eq!(value["schema"], 2);
    assert_eq!(value["installation_id"], fixture.identity.installation_id);
    assert_eq!(value["manifest_sha256"], fixture.identity.manifest_sha256);
    assert_eq!(value["media_label"], MEDIA_LABEL);
    assert_eq!(value["startup_script"], paths.startup_script);
    assert_eq!(value["scripts_ini"], GUEST_SCRIPTS_INI);
    assert_eq!(value["scripts_ini_backup"], paths.scripts_ini_backup);
    assert_eq!(value["gpt_ini"], GUEST_GPT_INI);
    assert_eq!(value["gpt_ini_backup"], paths.gpt_ini_backup);
    assert!(value.get("service_name").is_none());
    assert!(PROVISION_WRAPPER.contains("install-reviewed"));
    assert!(PROVISION_WRAPPER.contains("Get-FileHash"));
    assert!(PROVISION_WRAPPER.contains("startup_cleanup"));
    assert!(PROVISION_WRAPPER.contains("Remove-StartupEntry"));
    assert!(PROVISION_WRAPPER.contains("shutdown.exe /s /t 10 /f"));
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
