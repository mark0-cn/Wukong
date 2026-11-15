//! Native offline bootstrap review/elevation. The elevated process verifies the
//! exact reviewed enrollment; no shell interpreter or downloaded script is used.
use crate::{
    host::installer::{self, Plan, Setup},
    isolation::processes,
    isolation::security,
};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{ffi::OsStr, path::Path, ptr::null};
use windows_sys::Win32::{
    Foundation::CloseHandle,
    UI::{
        Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW},
        WindowsAndMessaging::{
            IDOK, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_OKCANCEL, MessageBoxW, SW_SHOWNORMAL,
        },
    },
};
use wukong_worker_package::PublicKey;

fn message(text: &str, flags: u32) -> Result<i32> {
    let title = security::wide(OsStr::new("Wukong offline Windows worker"))?;
    let body = security::wide(OsStr::new(text))?;
    Ok(unsafe { MessageBoxW(std::ptr::null_mut(), body.as_ptr(), title.as_ptr(), flags) })
}
pub fn review() -> Result<()> {
    let configuration = std::env::current_exe()?
        .parent()
        .context("setup has no directory")?
        .join("setup.json");
    let setup: Setup =
        serde_json::from_slice(&crate::artifacts::io::read_bytes(&configuration, 16384)?)?;
    let key = PublicKey::from_hex(&setup.controller_public_key)?;
    let plan = Plan::read(&configuration, &key)?;
    let enrollment = plan.enrollment.enrollment();
    let summary = format!(
        "Install this offline worker in the dedicated Windows test VM.\n\nEnvironment: {}\nInstallation: {}\nVersion: {}\nSSH port: {}\nAllowed controller address: {}\nController signing fingerprint:\n{}\n\nCompare this fingerprint with the controller's prepared bundle before continuing. All tools and dependencies are included; this installer downloads nothing.\n\nAdministrator elevation is required. Installation and production isolation still require VM acceptance.",
        enrollment.environment_id,
        enrollment.installation_id,
        plan.manifest.manifest().version,
        enrollment.ssh_port,
        enrollment.management_peer,
        key.fingerprint()
    );
    if message(&summary, MB_OKCANCEL | MB_ICONINFORMATION)? != IDOK {
        return Ok(());
    }
    let enrollment_hash = hex::encode(Sha256::digest(enrollment.canonical_bytes()?));
    if installer::elevated()? {
        return elevated(&configuration, &key, &enrollment_hash);
    }
    let executable = std::env::current_exe()?;
    let file = security::wide(executable.as_os_str())?;
    let verb = security::wide(OsStr::new("runas"))?;
    let parameters = processes::command_line(&[
        "install-elevated".into(),
        configuration.into_os_string(),
        key.to_hex().into(),
        enrollment_hash.into(),
    ])?;
    let mut information = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: parameters.as_ptr(),
        lpDirectory: null(),
        nShow: SW_SHOWNORMAL,
        ..Default::default()
    };
    security::check(unsafe { ShellExecuteExW(&mut information) })?;
    if !information.hProcess.is_null() {
        unsafe {
            CloseHandle(information.hProcess);
        }
    }
    Ok(())
}
/// Already-elevated dedicated-VM automation uses the same independently pinned
/// key and exact reviewed enrollment as the GUI. This never elevates itself.
pub fn install_reviewed(
    configuration: &Path,
    key: &PublicKey,
    enrollment_hash: &str,
) -> Result<serde_json::Value> {
    ensure!(
        installer::elevated()?,
        "administrator elevation is required for VM installation"
    );
    let plan = Plan::read(configuration, key)?;
    ensure!(
        hex::encode(Sha256::digest(
            plan.enrollment.enrollment().canonical_bytes()?
        )) == enrollment_hash,
        "offline enrollment changed after setup review"
    );
    installer::install(plan)
}
pub fn elevated(configuration: &Path, key: &PublicKey, enrollment_hash: &str) -> Result<()> {
    let result = install_reviewed(configuration, key, enrollment_hash);
    match result {
        Ok(value) => {
            message(
                &format!(
                    "Worker service started and offline dependency checks passed.\n\nSSH user: {}\nSSH port: {}\nHost fingerprint:\n{}\n\nImport connection.json from the installation into the controller and independently compare this host fingerprint. Production isolation acceptance remains pending.",
                    value["ssh_user"].as_str().unwrap_or(""),
                    value["ssh_port"],
                    value["host_fingerprint"].as_str().unwrap_or("")
                ),
                MB_OK | MB_ICONINFORMATION,
            )?;
            Ok(())
        }
        Err(error) => {
            message(
                &format!(
                    "Offline installation did not complete.\n\n{error:#}\n\nProtected installation records were preserved for repair. The installer does not overwrite existing accounts or services."
                ),
                MB_OK | MB_ICONERROR,
            )?;
            Err(error)
        }
    }
}
pub fn failure(error: &anyhow::Error) {
    let _ = message(
        &format!("Offline setup failed:\n\n{error:#}"),
        MB_OK | MB_ICONERROR,
    );
}
