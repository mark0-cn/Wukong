//! Public offline-provisioning planning surface: configuration validation, VMX
//! disk/CD planning and finalize-spec parsing. No VMware, WSL or VM is used.
use std::path::PathBuf;
use wukong::application::windows_lab::BASELINE_SNAPSHOT;
use wukong::application::windows_lab::provision::{
    SnapshotPlan, attach_offline_iso, plan_snapshot, read_finalize_spec, resolve_guest_disk,
};
use wukong::config::AppConfig;

#[test]
fn offline_tooling_configuration_appears_in_the_registered_vm() {
    let directory = tempfile::tempdir().unwrap();
    let configuration = directory.path().join("wukong.toml");
    let vmx = directory.path().join("lab.vmx");
    std::fs::write(&vmx, b"config").unwrap();
    std::fs::write(
        &configuration,
        format!(
            r#"
[windows_lab.vms.lab]
vmx = "{}"
vmrun = "{}"
[windows_lab.vms.lab.offline]
wsl_executable = "{}"
wsl_distribution = "Ubuntu"
guestfish = "/usr/bin/guestfish"
hivexsh = "/usr/bin/hivexsh"
mkisofs = "{}"
timeout_seconds = 600
"#,
            vmx.display().to_string().replace('\\', "/"),
            directory
                .path()
                .join("vmrun.exe")
                .display()
                .to_string()
                .replace('\\', "/"),
            directory
                .path()
                .join("wsl.exe")
                .display()
                .to_string()
                .replace('\\', "/"),
            directory
                .path()
                .join("mkisofs.exe")
                .display()
                .to_string()
                .replace('\\', "/"),
        ),
    )
    .unwrap();
    let config = AppConfig::load(Some(&configuration)).unwrap();
    let offline = config.windows_lab.vms["lab"].offline.as_ref().unwrap();
    assert_eq!(offline.wsl_distribution, "Ubuntu");
    assert_eq!(offline.timeout_seconds, 600);
}

#[test]
fn provisioning_plans_a_dedicated_cd_device_from_the_vmx() {
    let directory = tempfile::tempdir().unwrap();
    let vmx = directory.path().join("lab.vmx");
    let disk = directory.path().join("Windows 10 x64.vmdk");
    let iso = directory.path().join("offline.iso");
    std::fs::write(&disk, b"fixture disk").unwrap();
    let vmx_text = "config.version = \"8\"\nsata0.present = \"TRUE\"\nnvme0.present = \"TRUE\"\nnvme0:0.fileName = \"Windows 10 x64.vmdk\"\nnvme0:0.present = \"TRUE\"\n";
    std::fs::write(&vmx, vmx_text).unwrap();
    let guest = resolve_guest_disk(vmx_text, &vmx).unwrap();
    assert_eq!(guest.device, "nvme0:0");
    assert_eq!(guest.path, std::fs::canonicalize(&disk).unwrap());
    let edit = attach_offline_iso(vmx_text, &iso).unwrap();
    assert_eq!(edit.device, "sata0:0");
    assert!(edit.text.contains("sata0:0.fileName"));
    assert!(edit.text.contains("sata0:0.deviceType = \"cdrom-image\""));
}

#[test]
fn snapshot_planning_replaces_only_the_fixed_base() {
    assert_eq!(
        plan_snapshot(&[], BASELINE_SNAPSHOT, false).unwrap(),
        SnapshotPlan::Pristine
    );
    let baseline = vec![BASELINE_SNAPSHOT.to_string()];
    assert!(plan_snapshot(&baseline, BASELINE_SNAPSHOT, false).is_err());
    assert_eq!(
        plan_snapshot(&baseline, BASELINE_SNAPSHOT, true).unwrap(),
        SnapshotPlan::ReplaceBaseline
    );
}

#[test]
fn finalize_spec_requires_the_disposable_vm_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("spec.json");
    std::fs::write(
        &path,
        br#"{"schema":1,"installation_id":"958ef753fc7646598935ce8f79c53940","environment_id":"windows-vm","management_peer":"192.168.199.1","ssh_port":22268}"#,
    )
    .unwrap();
    let spec = read_finalize_spec(&path).unwrap();
    assert_eq!(spec.generation, 1);
    assert_eq!(spec.execution_boundary, "disposable_vm");
    assert!(spec.ssh_public_key.is_none());

    std::fs::write(
        &path,
        br#"{"schema":1,"installation_id":"958ef753fc7646598935ce8f79c53940","environment_id":"windows-vm","management_peer":"192.168.199.1","ssh_port":22268,"execution_boundary":"shared_host"}"#,
    )
    .unwrap();
    assert!(read_finalize_spec(&path).is_err());
    assert!(read_finalize_spec(PathBuf::from("relative.json").as_path()).is_err());
}
