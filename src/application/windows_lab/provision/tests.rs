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
    assert_eq!(paths.launcher, BOOT_LAUNCHER);
    assert_eq!(paths.task_file, BOOT_TASK_FILE);
    assert_eq!(paths.task_guid, BOOT_TASK_GUID);
    assert_eq!(paths.service, format!("WukongLabProvision-{id}"));
    // Retired local-GPO locations stay addressable so `clean-guest` can remove
    // artifacts of earlier attempts.
    assert_eq!(paths.startup_entry, format!("wukong-provision-{id}.cmd"));
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
        GuestPaths::guestfish(&paths.launcher).unwrap(),
        "/ProgramData/WukongLab/provision/launch.ps1"
    );
    assert_eq!(
        GuestPaths::guestfish(&paths.task_file).unwrap(),
        "/Windows/System32/Tasks/WukongLabProvision"
    );
    assert!(GuestPaths::guestfish(r"D:\outside").is_err());
    assert!(GuestPaths::for_provision("short").is_err());

    let launcher = boot_launcher(&id, &paths.wrapper).unwrap();
    let text = String::from_utf8(launcher.clone()).unwrap();
    assert!(text.contains(&format!("& '{}'", paths.wrapper)), "{text}");
    assert!(!text.contains("Startup"));
    assert_eq!(
        launcher_operation_id(&launcher).as_deref(),
        Some(id.as_str())
    );
    assert!(boot_launcher(&id, r"D:\rogue\provision.ps1").is_err());
    assert!(launcher_operation_id(b"no operation identity").is_none());
}

/// The offline disk adapter converts exactly once, at its boundary. Requests
/// therefore must carry absolute Windows guest paths, not libguestfs paths.
#[test]
fn provision_requests_carry_windows_guest_paths_that_convert_once() {
    let id = "e".repeat(32);
    let paths = GuestPaths::for_provision(&id).unwrap();
    let inspection = DiskInspectionRequest::for_provision(&paths);
    assert_eq!(inspection.system_hive, GUEST_SYSTEM_HIVE);
    assert_eq!(inspection.software_hive, GUEST_SOFTWARE_HIVE);
    assert_eq!(inspection.program_data, GUEST_PROGRAM_DATA);
    assert_eq!(inspection.provision_root, GUEST_PROVISION_ROOT);
    assert_eq!(inspection.tasks_dir, GUEST_TASKS_DIR);
    assert_eq!(inspection.task_file, paths.task_file);
    assert_eq!(inspection.launcher, paths.launcher);
    assert_eq!(inspection.task_cache_root, TASK_CACHE_ROOT);
    assert_eq!(inspection.task_name, BOOT_TASK_NAME);
    assert_eq!(inspection.task_guid, BOOT_TASK_GUID);
    assert_eq!(inspection.scripts_ini, GUEST_SCRIPTS_INI);
    assert_eq!(inspection.scripts_ini_backup, paths.scripts_ini_backup);
    assert_eq!(inspection.gpt_ini, GUEST_GPT_INI);
    assert_eq!(inspection.gpt_ini_backup, paths.gpt_ini_backup);
    assert_eq!(
        GuestPaths::guestfish(&inspection.system_hive).unwrap(),
        "/Windows/System32/config/SYSTEM"
    );
    assert_eq!(
        GuestPaths::guestfish(&inspection.software_hive).unwrap(),
        "/Windows/System32/config/SOFTWARE"
    );
    assert_eq!(
        GuestPaths::guestfish(&inspection.tasks_dir).unwrap(),
        "/Windows/System32/Tasks"
    );
    assert_eq!(
        GuestPaths::guestfish(&inspection.launcher).unwrap(),
        "/ProgramData/WukongLab/provision/launch.ps1"
    );
    assert_eq!(
        GuestPaths::guestfish(GUEST_SYSTEM_CONFIG).unwrap(),
        "/Windows/System32/config"
    );

    let observation = GuestObservationRequest::for_provision(&paths);
    assert_eq!(observation.launcher, BOOT_LAUNCHER);
    assert_eq!(observation.task_file, BOOT_TASK_FILE);
    assert_eq!(observation.software_hive, GUEST_SOFTWARE_HIVE);
    assert_eq!(observation.task_name, BOOT_TASK_NAME);
    assert_eq!(observation.task_guid, BOOT_TASK_GUID);
    assert_eq!(
        GuestPaths::guestfish(&observation.task_file).unwrap(),
        "/Windows/System32/Tasks/WukongLabProvision"
    );
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

/// Encode a parsed TaskCache value exactly as `hivexsh lsval` prints it.
fn hivexsh_encoded(value: &TaskCacheValue) -> String {
    match value {
        TaskCacheValue::String(text) => format!("\"{}\"", text.replace('\\', "\\\\")),
        TaskCacheValue::Dword(number) => format!("dword:{number:08x}"),
        TaskCacheValue::Binary(bytes) => format!(
            "hex(3):{}",
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

#[test]
fn canonical_boot_task_fixtures_are_pinned_and_complete() {
    validate_boot_task_fixture().unwrap();
    assert_eq!(BOOT_TASK_XML.len(), 3160);
    assert_eq!(BOOT_TASK_TREE_SD.len(), 124);
    assert_eq!(BOOT_TASK_HASH.len(), 32);
    assert_eq!(BOOT_TASK_TRIGGERS.len(), 288);
    assert_eq!(BOOT_TASK_ACTIONS.len(), 400);
    assert_eq!(BOOT_TASK_DYNAMIC_INFO.len(), 36);
    assert_eq!(sha256(BOOT_TASK_XML), BOOT_TASK_XML_SHA256);
    assert_eq!(sha256(BOOT_TASK_TREE_SD), BOOT_TASK_TREE_SD_SHA256);
    assert_eq!(sha256(BOOT_TASK_HASH), BOOT_TASK_HASH_SHA256);
    assert_eq!(sha256(BOOT_TASK_TRIGGERS), BOOT_TASK_TRIGGERS_SHA256);
    assert_eq!(sha256(BOOT_TASK_ACTIONS), BOOT_TASK_ACTIONS_SHA256);
    assert_eq!(
        sha256(BOOT_TASK_DYNAMIC_INFO),
        BOOT_TASK_DYNAMIC_INFO_SHA256
    );

    let body = &BOOT_TASK_XML[2..];
    let units = body
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let text = String::from_utf16(&units).unwrap();
    for needle in [
        "<BootTrigger>",
        "<UserId>S-1-5-18</UserId>",
        "<Delay>PT1M</Delay>",
        "<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>",
        "WukongLabProvision",
        BOOT_LAUNCHER,
    ] {
        assert!(
            text.contains(needle),
            "canonical boot task file lost {needle}"
        );
    }
    assert!(!text.contains("GroupPolicy"));
    assert!(!text.contains("scripts.ini"));

    let injection = boot_task_injection().unwrap();
    assert_eq!(injection.file.windows_path, BOOT_TASK_FILE);
    assert_eq!(injection.file.bytes, BOOT_TASK_XML);
    assert_eq!(injection.file.sha256, BOOT_TASK_XML_SHA256);
    assert_eq!(injection.registry.guid, BOOT_TASK_GUID);
    assert_eq!(injection.registry.tree_name, BOOT_TASK_NAME);
    assert_eq!(injection.registry.index, 1);
    assert_eq!(injection.registry.values.len(), 8);
    assert_eq!(injection.registry.expected_tree_values().len(), 3);
    // Only well-known SIDs may appear in the security descriptor; it is
    // machine independent so it can be replayed into any guest.
    assert!(BOOT_TASK_TREE_SD.starts_with(&[0x01, 0x00, 0x04]));
}

#[test]
fn boot_task_hive_scripts_create_and_remove_only_the_fixed_registration() {
    let registry = boot_task_registry();
    let write = registry.write_script();
    assert!(write.contains(&format!("add {BOOT_TASK_NAME}")));
    assert!(write.contains(&format!("add {BOOT_TASK_GUID}")));
    assert!(write.contains(&format!("string:{BOOT_TASK_GUID}")));
    assert!(write.contains("dword:0x00010002"));
    assert!(write.contains("string:\\WukongLabProvision"));
    assert!(write.contains(&format!("string:{BOOT_TASK_SDDL}")));
    assert!(write.ends_with("commit\n"));
    assert!(write.contains("cd \\Microsoft\\Windows NT\\CurrentVersion\\Schedule\\TaskCache"));
    // Every binary literal in the script must decode to the pinned fixture bytes.
    let hive_write_literal = |bytes: &[u8]| {
        format!(
            "hex:3:{}",
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    for (name, value) in &registry.values {
        if let TaskCacheValue::Binary(bytes) = value {
            let literal = hive_write_literal(bytes);
            assert!(
                write.contains(&format!("{name}\n{literal}")),
                "{name} literal missing"
            );
        }
    }
    assert!(write.contains(&format!("SD\n{}", hive_write_literal(BOOT_TASK_TREE_SD))));

    let verify = registry.verify_script();
    assert!(verify.contains(&registry.tree_path()));
    assert!(verify.contains(&registry.tasks_path()));
    assert_eq!(registry.remove_script(false, false, false), None);
    let remove = registry.remove_script(true, true, true).unwrap();
    for path in [
        registry.tree_path(),
        registry.tasks_path(),
        registry.boot_path(),
    ] {
        assert!(remove.contains(&format!("cd {path}\ndel\n")), "{remove}");
    }
    assert!(remove.ends_with("commit\n"));
    let boot_only = registry.remove_script(false, false, true).unwrap();
    assert_eq!(
        boot_only,
        format!("cd {}\ndel\ncommit\n", registry.boot_path())
    );
}

#[test]
fn task_cache_values_parse_and_reject_tampering() {
    let registry = boot_task_registry();
    let mut canonical = String::new();
    for (name, value) in registry
        .expected_tree_values()
        .iter()
        .chain(registry.values.iter())
    {
        canonical.push_str(&format!("\"{name}\"={}\n", hivexsh_encoded(value)));
    }
    verify_boot_task_values(&canonical).unwrap();
    let mut tampered = canonical.clone();
    tampered.push_str("\"Extra\"=dword:00000001\n");
    assert!(verify_boot_task_values(&tampered).is_err());
    let tampered = canonical.replace("dword:00010002", "dword:00010003");
    assert!(verify_boot_task_values(&tampered).is_err());
    let tampered = canonical.replace("D:P(A;;FA;;;BA)", "D:(A;;FA;;;BA)");
    assert!(verify_boot_task_values(&tampered).is_err());

    let parsed =
        parse_task_cache_values("\"Path\"=\"\\\\WukongLabProvision\"\n\"Index\"=dword:00000001\n")
            .unwrap();
    assert_eq!(
        parsed[0].1,
        TaskCacheValue::String(r"\WukongLabProvision".into())
    );
    assert_eq!(parsed[1].1, TaskCacheValue::Dword(1));
    assert!(parse_task_cache_values("not a value line").is_err());
    assert!(parse_task_cache_values("\"Hash\"=hex(3):zz").is_err());
}

#[test]
fn boot_task_cache_state_detects_partial_and_complete_registrations() {
    let tree = vec![BOOT_TASK_NAME.to_string()];
    let tasks = vec![BOOT_TASK_GUID.to_string()];
    let boot = vec![BOOT_TASK_GUID.to_string()];
    let complete = boot_task_cache_state(&tree, &tasks, &boot);
    assert!(complete.is_registered() && complete.is_complete());
    let partial = boot_task_cache_state(&tree, &[], &[]);
    assert!(partial.is_registered() && !partial.is_complete());
    assert!(!boot_task_cache_state(&[], &[], &[]).is_registered());
    // The registry compares task names case-insensitively.
    assert!(boot_task_cache_state(&["wukonglabprovision".into()], &[], &[]).tree);
}
fn hivexsh_deleted_key(script: &str) -> Option<&str> {
    let lines: Vec<&str> = script.lines().collect();
    let [cd, del, commit, quit] = lines.as_slice() else {
        return None;
    };
    let service_key = cd.strip_prefix("cd ")?;
    (*del == "del" && *commit == "commit" && *quit == "quit").then_some(service_key)
}

#[test]
fn legacy_service_removal_uses_the_bare_hivexsh_del_form() {
    let service = "WukongLabProvision-abc";
    let expected_key = "\\ControlSet001\\Services\\WukongLabProvision-abc";
    let script = hive_remove_service_script(1, service).unwrap();
    // hivexsh `del` takes no arguments and deletes the current node, so the
    // script must navigate into the exact service key first.
    assert_eq!(
        script,
        format!("cd {expected_key}\ndel\ncommit\nquit\n"),
        "the emitted script must use hivexsh's argument-free del form"
    );
    assert_eq!(hivexsh_deleted_key(&script), Some(expected_key));
    assert_eq!(
        hivexsh_deleted_key(&hive_remove_service_script(12, "svc-1").unwrap()),
        Some("\\ControlSet012\\Services\\svc-1"),
        "the control set is still formatted from the discovered value"
    );
    // The previous `cd Services` + `del <service>` form is rejected by hivexsh
    // 1.46.2/1.48.2 before it deletes anything, so the script grammar must
    // reject it instead of silently regressing.
    let old_form = "cd \\ControlSet001\\Services\ndel WukongLabProvision-abc\ncommit\nquit\n";
    assert_eq!(
        hivexsh_deleted_key(old_form),
        None,
        "a del argument (the broken form) must fail the script grammar"
    );
    assert!(hive_remove_service_script(0, "x").is_err());
    assert!(hive_remove_service_script(1, "bad/name").is_err());
    assert!(validate_service_name("bad name").is_err());
}

fn cleanup_inspection(_paths: &GuestPaths) -> DiskInspection {
    DiskInspection {
        windows_volume: true,
        system_hive_present: true,
        software_hive_present: true,
        program_data_present: true,
        control_set: Some(1),
        existing_service: None,
        provision_root_entries: Vec::new(),
        task_file_present: false,
        boot_task: BootTaskCacheState::default(),
        launcher: None,
        policy: GuestPolicyFiles::default(),
    }
}

#[test]
fn cleanup_plan_restores_policy_byte_exactly_and_removes_created_task_artifacts() {
    let paths = GuestPaths::for_provision(&"c".repeat(32)).unwrap();
    // The recovered backup was 53 bytes; any restore must reproduce these exact
    // bytes and must never append a newline.
    let original_scripts = b"[Startup]\r\n0CmdLine=enable-admin.cmd\r\n0Parameters=\r\n".to_vec();
    assert_eq!(original_scripts.len(), 52);
    let injected_scripts = format!(
        "[Startup]\r\n0CmdLine=enable-admin.cmd\r\n0Parameters=\r\n1CmdLine={}\r\n1Parameters=\r\n",
        paths.startup_entry
    )
    .into_bytes();
    let mut inspection = cleanup_inspection(&paths);
    inspection.policy.scripts_ini = Some(injected_scripts);
    inspection.policy.scripts_ini_backup = Some(original_scripts.clone());
    inspection.policy.startup_command_lines =
        vec!["enable-admin.cmd".into(), paths.startup_entry.clone()];
    inspection.policy.startup_directory_entries = vec![paths.startup_entry.clone()];
    inspection.policy.gpt_ini = Some(b"[General]\r\nVersion=65539\r\n".to_vec());
    inspection.policy.gpt_ini_backup = Some(b"previous gpt.ini".to_vec());
    inspection.policy.group_policy_entries =
        vec![format!("gpt.ini.wukong-lab-{}.bak", paths.provision_id())];
    let plan = plan_guest_cleanup(&paths, &inspection, true).unwrap();
    let restored = plan
        .files
        .iter()
        .find(|file| file.windows_path == paths.scripts_ini)
        .expect("scripts.ini is restored");
    assert_eq!(
        restored.bytes, original_scripts,
        "the previous bytes are written verbatim, with no added newline"
    );
    assert_ne!(
        restored.bytes.len(),
        b"[Startup]\r\n0CmdLine=enable-admin.cmd\r\n0Parameters=\r\n\n".len(),
        "restore must not append a newline"
    );
    let restored_gpt = plan
        .files
        .iter()
        .find(|file| file.windows_path == paths.gpt_ini)
        .expect("gpt.ini is restored");
    assert_eq!(restored_gpt.bytes, b"previous gpt.ini");
    assert!(plan.files_to_remove.contains(&paths.scripts_ini_backup));
    assert!(plan.files_to_remove.contains(&paths.gpt_ini_backup));
    assert!(plan.files_to_remove.contains(&paths.startup_script));
    assert!(
        plan.actions
            .iter()
            .any(|action| action.contains("verbatim"))
    );

    // The fixed scheduled-task artifacts are removed as part of the same plan.
    inspection.boot_task = BootTaskCacheState {
        tree: true,
        tasks: true,
        boot: true,
    };
    inspection.task_file_present = true;
    inspection.launcher = Some(boot_launcher(paths.provision_id(), &paths.wrapper).unwrap());
    inspection.existing_service = Some(paths.service.clone());
    inspection.provision_root_entries = vec![paths.provision_id().into()];
    let plan = plan_guest_cleanup(&paths, &inspection, true).unwrap();
    assert!(plan.boot_task);
    assert!(plan.files_to_remove.contains(&paths.task_file));
    assert!(plan.files_to_remove.contains(&paths.launcher));
    assert!(plan.directories_to_remove.contains(&paths.root));
    assert_eq!(plan.service_key.as_deref(), Some(paths.service.as_str()));
    let boot_action = plan
        .actions
        .iter()
        .position(|action| action.contains("scheduled-task registration"))
        .unwrap();
    let launcher_action = plan
        .actions
        .iter()
        .position(|action| action.contains("boot launcher"))
        .unwrap();
    assert!(boot_action < launcher_action);
}

#[test]
fn cleanup_plan_deletes_a_cli_created_gpt_ini_only_for_the_last_operation() {
    let paths = GuestPaths::for_provision(&"f".repeat(32)).unwrap();
    let mut inspection = cleanup_inspection(&paths);
    inspection.policy.gpt_ini = Some(LEGACY_CREATED_GPT_INI.to_vec());
    let plan = plan_guest_cleanup(&paths, &inspection, true).unwrap();
    assert!(plan.files_to_remove.contains(&paths.gpt_ini));
    assert!(
        plan.actions
            .iter()
            .any(|action| action.contains("CLI-created"))
    );
    let plan = plan_guest_cleanup(&paths, &inspection, false).unwrap();
    assert!(
        !plan.files_to_remove.contains(&paths.gpt_ini),
        "a created gpt.ini is only attributable to the last recorded operation"
    );
    inspection.policy.gpt_ini = Some(b"[General]\r\nVersion=65537\r\n".to_vec());
    let plan = plan_guest_cleanup(&paths, &inspection, true).unwrap();
    assert!(
        !plan.files_to_remove.contains(&paths.gpt_ini),
        "only the exact created document may be deleted"
    );
}

#[test]
fn cleanup_plan_leaves_another_operations_boot_task_artifacts_alone() {
    let paths = GuestPaths::for_provision(&"c".repeat(32)).unwrap();
    let other = "d".repeat(32);
    let other_wrapper = format!(r"{GUEST_PROVISION_ROOT}\{other}\provision.ps1");
    let mut inspection = cleanup_inspection(&paths);
    inspection.boot_task = BootTaskCacheState {
        tree: true,
        tasks: true,
        boot: true,
    };
    inspection.task_file_present = true;
    inspection.launcher = Some(boot_launcher(&other, &other_wrapper).unwrap());
    inspection.provision_root_entries = vec![other];
    let plan = plan_guest_cleanup(&paths, &inspection, true).unwrap();
    assert!(!plan.boot_task);
    assert!(!plan.files_to_remove.contains(&paths.task_file));
    assert!(!plan.files_to_remove.contains(&paths.launcher));
    assert!(plan.is_empty());
}

#[test]
fn leftover_detection_covers_every_injected_artifact() {
    let paths = GuestPaths::for_provision(&"c".repeat(32)).unwrap();
    let mut inspection = cleanup_inspection(&paths);
    assert!(inspection.leftover_operation_ids().is_empty());
    assert!(!inspection.has_boot_task_leftovers());
    inspection.task_file_present = true;
    assert!(inspection.has_boot_task_leftovers());
    inspection.boot_task = BootTaskCacheState {
        tree: true,
        tasks: true,
        boot: true,
    };
    inspection.launcher = Some(boot_launcher(paths.provision_id(), &paths.wrapper).unwrap());
    inspection.existing_service = Some(paths.service.clone());
    inspection.provision_root_entries = vec![paths.provision_id().into()];
    inspection.policy.startup_command_lines = vec![paths.startup_entry.clone()];
    inspection.policy.group_policy_entries =
        vec![format!("gpt.ini.wukong-lab-{}.bak", paths.provision_id())];
    let ids = inspection.leftover_operation_ids();
    assert_eq!(ids, vec![paths.provision_id().to_owned()]);
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
    let restored = resolve_guest_disk("nvme0:0.fileName = \"base-000001.vmdk\"\n", &vmx).unwrap();
    assert_eq!(
        restored.path,
        fs::canonicalize(directory.path().join("base-000001.vmdk")).unwrap()
    );
}

#[test]
fn iso_attachment_uses_a_dedicated_cd_device() {
    let directory = tempfile::tempdir().unwrap();
    let iso = &directory.path().join("offline.iso");
    let vmx = "config.version = \"8\"\nsata0.present = \"TRUE\"\nsata0:0.fileName = \"disk.vmdk\"\nsata0:1.deviceType = \"cdrom-image\"\nsata0:1.present = \"FALSE\"\nsata0:1.fileName = \"\"\n";
    let edit = attach_offline_iso(vmx, iso).unwrap();
    assert_eq!(edit.device, "sata0:1");
    assert!(edit.text.contains("sata0:1.present = \"TRUE\""));
    assert!(
        edit.text
            .contains(&format!("sata0:1.fileName = \"{}\"", iso.display()))
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
fn guest_receipt_binds_media_identity_and_scheduled_task_cleanup() {
    let media = fixture::media();
    let provision_id = "b".repeat(32);
    let expectation = ReceiptExpectation {
        provision_id: provision_id.clone(),
        media: media.identity.clone(),
    };
    let receipt = fixture::guest_receipt(&provision_id, &media.connection);
    let connection = verify_guest_receipt(&receipt, &expectation).unwrap();
    assert_eq!(connection, media.connection);

    let mut current: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
    current["connection"]["candidate_addresses"] = serde_json::json!(["invalid"]);
    assert!(verify_guest_receipt(&serde_json::to_vec(&current).unwrap(), &expectation).is_err());
    let mut old: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
    old["connection"]["receipt_version"] = serde_json::json!(1);
    assert!(verify_guest_receipt(&serde_json::to_vec(&old).unwrap(), &expectation).is_err());

    for field in ["boot_task_removed", "launcher_deleted", "cleanup_scheduled"] {
        let mut value: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
        value[field] = serde_json::json!(false);
        let error =
            verify_guest_receipt(&serde_json::to_vec(&value).unwrap(), &expectation).unwrap_err();
        assert!(
            format!("{error:#}").contains("scheduled")
                || format!("{error:#}").contains("launcher")
                || format!("{error:#}").contains("cleanup"),
            "{error:#}"
        );
    }
    let mut value: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
    value["installed"] = serde_json::json!(false);
    assert!(verify_guest_receipt(&serde_json::to_vec(&value).unwrap(), &expectation).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
    value["error"] = serde_json::json!("media missing");
    assert!(verify_guest_receipt(&serde_json::to_vec(&value).unwrap(), &expectation).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
    value["connection"] = serde_json::Value::Null;
    assert!(verify_guest_receipt(&serde_json::to_vec(&value).unwrap(), &expectation).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
    value["provision_id"] = serde_json::json!("c".repeat(32));
    assert!(verify_guest_receipt(&serde_json::to_vec(&value).unwrap(), &expectation).is_err());
}

#[test]
fn guest_receipt_pins_the_installer_generated_host_key() {
    let media = fixture::media();
    // The enrollment/media key is the controller's SSH client identity. The
    // worker installer generates its own sshd host key, so a successful install
    // can never report the media key; the previous equality check rejected every
    // correct guest receipt.
    assert_ne!(
        media.connection.host_key,
        media.identity.ssh_public_key.public().unwrap()
    );
    assert_eq!(
        media.connection.host_fingerprint,
        media.connection.host_key.fingerprint().unwrap()
    );
    let provision_id = "d".repeat(32);
    let expectation = ReceiptExpectation {
        provision_id: provision_id.clone(),
        media: media.identity.clone(),
    };
    let receipt = fixture::guest_receipt(&provision_id, &media.connection);
    let connection = verify_guest_receipt(&receipt, &expectation).unwrap();
    assert_eq!(connection.host_key, media.connection.host_key);

    let reject = |mutate: &dyn Fn(&mut serde_json::Value)| {
        let mut value: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
        mutate(&mut value["connection"]);
        let error =
            verify_guest_receipt(&serde_json::to_vec(&value).unwrap(), &expectation).unwrap_err();
        format!("{error:#}")
    };

    let error = reject(&|connection| {
        connection["host_fingerprint"] = serde_json::json!("SHA256:wrong");
    });
    assert!(error.contains("fingerprint"), "{error}");

    let error = reject(&|connection| {
        connection["host_fingerprint"] = serde_json::json!("");
    });
    assert!(error.contains("fingerprint"), "{error}");

    let error = reject(&|connection| {
        connection["host_key"]["key"] = serde_json::json!("");
    });
    assert!(error.contains("host key"), "{error}");

    let error = reject(&|connection| {
        connection["host_key"]["algorithm"] = serde_json::json!("ssh-rsa");
    });
    assert!(
        error.contains("Ed25519") || error.contains("public key"),
        "{error}"
    );

    let error = reject(&|connection| {
        connection.as_object_mut().unwrap().remove("host_key");
    });
    assert!(
        error.contains("host_key") || error.contains("connection"),
        "{error}"
    );
}

#[test]
fn provision_plan_exposes_only_verified_identities() {
    let media = fixture::media();
    let provision_id = "b".repeat(32);
    let paths = GuestPaths::for_provision(&provision_id).unwrap();
    let plan: serde_json::Value =
        serde_json::from_slice(&provision_plan(&provision_id, &media.identity, &paths).unwrap())
            .unwrap();
    assert_eq!(plan["schema"], 3);
    assert_eq!(plan["provision_id"], provision_id);
    assert_eq!(plan["installation_id"], media.identity.installation_id);
    assert_eq!(plan["launcher"], BOOT_LAUNCHER);
    assert_eq!(plan["task_file"], BOOT_TASK_FILE);
    assert_eq!(plan["task_name"], BOOT_TASK_NAME);
    assert_eq!(plan["task_guid"], BOOT_TASK_GUID);
    for legacy in [
        "scripts_ini",
        "gpt_ini",
        "startup_script",
        "scripts_ini_backup",
    ] {
        assert!(
            plan.get(legacy).is_none(),
            "legacy plan field {legacy} remained"
        );
    }
}

#[test]
fn wrapper_removes_the_boot_task_and_launcher_before_shutdown() {
    let wrapper = PROVISION_WRAPPER;
    let receipt = wrapper
        .find("# Record the installation outcome")
        .expect("wrapper writes the receipt before cleanup");
    let delete = wrapper
        .find("# Unregister the boot trigger")
        .expect("wrapper unregisters the boot task first");
    let launcher = wrapper
        .find("# Delete the fixed per-operation boot launcher")
        .expect("wrapper removes the launcher");
    let shutdown = wrapper
        .find("shutdown.exe /s")
        .expect("wrapper always shuts down");
    assert!(
        receipt < delete,
        "the receipt is written before the boot task is removed"
    );
    assert!(delete < shutdown, "cleanup runs before shutdown");
    assert!(
        launcher < shutdown,
        "the launcher is removed before shutdown"
    );
    assert!(wrapper.contains("boot_task_removed"));
    assert!(wrapper.contains("launcher_deleted"));
    assert!(wrapper.contains("Remove-BootTask"));
    // The retired mechanisms must not reappear in the guest wrapper.
    assert!(!wrapper.contains("scripts.ini"));
    assert!(!wrapper.contains("gpt.ini"));
    assert!(!wrapper.contains("GroupPolicy"));
    assert!(!wrapper.contains("Startup\\"));
    assert!(!wrapper.contains("New-Service") && !wrapper.contains("sc.exe create"));
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
