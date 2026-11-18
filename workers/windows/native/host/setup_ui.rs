//! Native offline bootstrap review/elevation and install-result export. The elevated
//! process verifies the exact reviewed enrollment; no shell interpreter or downloaded
//! script is used. Saving the public result copy never changes installation success.
use crate::{
    host::installer::{self, Plan, Setup},
    host::network,
    isolation::processes,
    isolation::security,
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    mem::size_of,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
    ptr::null,
};
use windows_sys::Win32::{
    Foundation::CloseHandle,
    UI::{
        Controls::Dialogs::{
            CommDlgExtendedError, GetSaveFileNameW, OFN_EXPLORER, OFN_NOCHANGEDIR,
            OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
        },
        Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW},
        WindowsAndMessaging::{
            IDOK, IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_OKCANCEL, MB_YESNO,
            MessageBoxW, SW_SHOWNORMAL,
        },
    },
};
use wukong_worker_package::PublicKey;

const SAVE_DIALOG_TITLE: &str = "Save Wukong installation result";
const SAVE_DIALOG_FILTER: &str = "JSON (*.json)\u{0}*.json\u{0}All files (*.*)\u{0}*.*\u{0}";
const SAVE_DIALOG_PATH_UNITS: usize = 4096;
const EXPORT_BYTES_LIMIT: u64 = 64 * 1024;

fn message(text: &str, flags: u32) -> Result<i32> {
    let title = security::wide(OsStr::new("Wukong offline Windows worker"))?;
    let body = security::wide(OsStr::new(text))?;
    // SAFETY: body and title are NUL-terminated wide strings from security::wide and stay
    // alive through the call; a null owner window is allowed for MessageBoxW.
    Ok(unsafe { MessageBoxW(std::ptr::null_mut(), body.as_ptr(), title.as_ptr(), flags) })
}
/// Injectable save-dialog boundary so result export and its fallback are testable.
trait SaveDialog {
    /// Ask the user where to save `suggested_name`; `Ok(None)` means the user cancelled.
    fn choose(&self, suggested_name: &str) -> Result<Option<PathBuf>>;
}
struct SystemSaveDialog;
impl SaveDialog for SystemSaveDialog {
    fn choose(&self, suggested_name: &str) -> Result<Option<PathBuf>> {
        ensure!(
            suggested_name.len() < SAVE_DIALOG_PATH_UNITS / 2,
            "installation result file name is too long"
        );
        let name = suggested_name.encode_utf16().collect::<Vec<_>>();
        let mut file = vec![0u16; SAVE_DIALOG_PATH_UNITS];
        file[..name.len()].copy_from_slice(&name);
        let filter = SAVE_DIALOG_FILTER
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let title = security::wide(OsStr::new(SAVE_DIALOG_TITLE))?;
        let extension = security::wide(OsStr::new("json"))?;
        let mut information = OPENFILENAMEW {
            lStructSize: size_of::<OPENFILENAMEW>() as u32,
            lpstrFilter: filter.as_ptr(),
            nFilterIndex: 1,
            lpstrFile: file.as_mut_ptr(),
            nMaxFile: file.len() as u32,
            lpstrTitle: title.as_ptr(),
            lpstrDefExt: extension.as_ptr(),
            Flags: OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        // SAFETY: `information` is a fully initialized OPENFILENAMEW whose `lStructSize` is its
        // own type size; `filter`, `title`, `extension` and `file` are live NUL-terminated
        // buffers that outlive the call, and Windows only writes the chosen path into `file`.
        let chosen = unsafe { GetSaveFileNameW(&mut information) } != 0;
        if !chosen {
            // SAFETY: No pointers are passed; the common dialog implementation reports the
            // extended error of the immediately preceding call for this calling thread.
            let error = unsafe { CommDlgExtendedError() };
            // A zero extended error is the documented user-cancellation result.
            ensure!(
                error == 0,
                "the Windows save dialog is unavailable (common dialog error 0x{error:08x})"
            );
            return Ok(None);
        }
        let length = file
            .iter()
            .position(|unit| *unit == 0)
            .context("the Windows save dialog returned no path")?;
        ensure!(length > 0, "the Windows save dialog returned an empty path");
        Ok(Some(PathBuf::from(OsString::from_wide(&file[..length]))))
    }
}
/// Result of one public install-result export attempt.
#[derive(Debug)]
enum ExportResult {
    Saved { path: PathBuf, fallback: bool },
    Cancelled,
    Failed(anyhow::Error),
}
fn export_name(installation_id: &str) -> Result<String> {
    crate::rpc::pipe_transport::service_name(installation_id)?;
    Ok(format!("wukong-worker-{installation_id}.json"))
}
fn write_export(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut output = fs::File::create(path)
        .with_context(|| format!("cannot create the export file {}", path.display()))?;
    output.write_all(bytes)?;
    output.sync_all()?;
    Ok(())
}
fn export_to_fallback(
    bytes: &[u8],
    fallback: Option<&Path>,
    reason: anyhow::Error,
) -> ExportResult {
    let Some(path) = fallback else {
        return ExportResult::Failed(reason);
    };
    match write_export(path, bytes) {
        Ok(()) => ExportResult::Saved {
            path: path.to_owned(),
            fallback: true,
        },
        Err(error) => ExportResult::Failed(
            reason.context(format!("the fallback copy also failed: {error:#}")),
        ),
    }
}
/// Save `bytes` through the injected dialog. A failed dialog or a failed write to
/// the chosen path falls back to `fallback`; user cancellation writes nothing.
fn save_export_with(
    dialog: &impl SaveDialog,
    bytes: &[u8],
    suggested_name: &str,
    fallback: Option<PathBuf>,
) -> ExportResult {
    match dialog.choose(suggested_name) {
        Ok(Some(path)) => match write_export(&path, bytes) {
            Ok(()) => ExportResult::Saved {
                path,
                fallback: false,
            },
            Err(error) => export_to_fallback(
                bytes,
                fallback.as_deref(),
                error.context("cannot write the chosen installation result"),
            ),
        },
        Ok(None) => ExportResult::Cancelled,
        Err(error) => export_to_fallback(
            bytes,
            fallback.as_deref(),
            error.context("the save dialog is unavailable"),
        ),
    }
}
/// Preferred writable fallback: the current user's Desktop, then Public (its
/// Desktop when present). Returns `None` when the guest exposes no such location.
fn fallback_export_path(
    user_profile: Option<&Path>,
    public: Option<&Path>,
    name: &str,
) -> Option<PathBuf> {
    let user_desktop = user_profile
        .map(|profile| profile.join("Desktop"))
        .filter(|desktop| desktop.is_dir());
    if let Some(desktop) = user_desktop {
        return Some(desktop.join(name));
    }
    let public = public?;
    let public_desktop = public.join("Desktop");
    Some(if public_desktop.is_dir() {
        public_desktop.join(name)
    } else {
        public.join(name)
    })
}
fn fallback_export_path_from_environment(name: &str) -> Option<PathBuf> {
    let user_profile = std::env::var_os("USERPROFILE").map(PathBuf::from);
    let public = std::env::var_os("PUBLIC").map(PathBuf::from);
    fallback_export_path(user_profile.as_deref(), public.as_deref(), name)
}
fn serialize_receipt(receipt: &Value) -> Result<Vec<u8>> {
    let mut bytes =
        serde_json::to_vec(receipt).context("cannot serialize the installation result")?;
    bytes.push(b'\n');
    Ok(bytes)
}
/// Read the stored receipt verbatim so a copy always matches `connection.json`.
fn stored_receipt_bytes(base: &Path, installation_id: &str) -> Result<Option<Vec<u8>>> {
    let path = installer::installation_receipt_path(base, installation_id)?;
    match crate::artifacts::io::read_bytes(&path, EXPORT_BYTES_LIMIT) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(_) => Ok(None),
    }
}
fn receipt_bytes(installation_id: &str, receipt: &Value) -> Result<Vec<u8>> {
    let stored =
        installer::install_base().and_then(|base| stored_receipt_bytes(&base, installation_id));
    match stored {
        Ok(Some(bytes)) => Ok(bytes),
        _ => serialize_receipt(receipt),
    }
}
fn export_bytes(bytes: &[u8], installation_id: &str) -> Result<ExportResult> {
    let name = export_name(installation_id)?;
    let fallback = fallback_export_path_from_environment(&name);
    Ok(save_export_with(&SystemSaveDialog, bytes, &name, fallback))
}
fn save_install_result(installation_id: &str, receipt: &Value) -> Result<ExportResult> {
    let bytes = receipt_bytes(installation_id, receipt)?;
    export_bytes(&bytes, installation_id)
}
fn report_export(outcome: ExportResult) -> Result<()> {
    match outcome {
        ExportResult::Saved {
            path,
            fallback: true,
        } => message(
            &format!(
                "The Windows save dialog was unavailable or the selected location could not be written.\n\nThe installation result was copied to the fallback location:\n{}\n\nThe protected installation record remains in ProgramData.",
                path.display()
            ),
            MB_OK | MB_ICONINFORMATION,
        )
        .map(|_| ()),
        ExportResult::Saved {
            path,
            fallback: false,
        } => message(
            &format!("Installation result saved:\n{}", path.display()),
            MB_OK | MB_ICONINFORMATION,
        )
        .map(|_| ()),
        ExportResult::Cancelled => Ok(()),
        ExportResult::Failed(error) => message(
            &format!(
                "The installation result copy could not be saved.\n\n{error:#}\n\nThe installation itself is complete; start setup again without arguments to save a refreshed result."
            ),
            MB_OK | MB_ICONERROR,
        )
        .map(|_| ()),
    }
}
/// Re-started setup on an already installed guest. Never reinstalls; optionally
/// exports a refreshed public result with the current guest address candidates.
fn review_existing(installed: &[installer::InstalledReceipt]) -> Result<()> {
    if installed.len() > 1 {
        let installations = installed
            .iter()
            .map(|found| found.installation_id.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        return message(
            &format!(
                "This guest already contains more than one Wukong worker installation. Setup did not reinstall or modify any of them.\n\n{installations}\n\nReinstall or repair a worker only from its offline media on a disposable VM."
            ),
            MB_OK | MB_ICONINFORMATION,
        )
        .map(|_| ());
    }
    let found = installed
        .first()
        .context("installed receipt disappeared during review")?;
    let receipt = &found.receipt;
    let summary = format!(
        "Wukong Windows worker is already installed; this setup will not reinstall it.\n\nInstallation: {}\nEnvironment: {}\nSSH user: {}\nSSH port: {}\nHost fingerprint:\n{}\n\nSave a refreshed installation result with the current guest IPv4 addresses (保存刷新后的安装结果)?",
        found.installation_id,
        receipt["environment_id"].as_str().unwrap_or(""),
        receipt["ssh_user"].as_str().unwrap_or(""),
        receipt["ssh_port"],
        receipt["host_fingerprint"].as_str().unwrap_or("")
    );
    if message(&summary, MB_YESNO | MB_ICONINFORMATION)? != IDYES {
        return Ok(());
    }
    let candidates = network::receipt_candidates(&network::SystemAdapters);
    let refreshed = installer::refreshed_receipt(receipt, candidates)?;
    let bytes = serialize_receipt(&refreshed)?;
    report_export(export_bytes(&bytes, &found.installation_id)?)
}
pub fn review() -> Result<()> {
    // A no-argument start must never reinstall. An existing complete installation
    // only offers a refreshed public export; repair stays on the reviewed media path.
    let installed = installer::installed_receipts(&installer::install_base()?)?;
    if !installed.is_empty() {
        return review_existing(&installed);
    }
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
        "Install this offline worker in a dedicated, disposable Windows VM. Targets may affect the entire guest. Restrict host access and external networking at the hypervisor; keep controller credentials outside the VM.\n\nEnvironment: {}\nInstallation: {}\nVersion: {}\nSSH port: {}\nAllowed controller address: {}\nController signing fingerprint:\n{}\n\nCompare this fingerprint with the controller's prepared bundle before continuing. All tools and dependencies are included; this installer downloads nothing.\n\nAdministrator elevation is required. Guest startup checks do not verify hypervisor isolation. Discard or reset the VM after untrusted work.",
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
    // SAFETY: information is a fully initialized SHELLEXECUTEINFOW with cbSize set, and
    // verb, file and parameters reference live locals for the duration of the call.
    security::check(unsafe { ShellExecuteExW(&mut information) })?;
    if !information.hProcess.is_null() {
        // SAFETY: ShellExecuteExW succeeded with SEE_MASK_NOCLOSEPROCESS and hProcess is
        // non-null here, so this closes the one process handle the API transferred to us.
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
            let summary = format!(
                "Worker service started and offline dependency checks passed.\n\nSSH user: {}\nSSH port: {}\nHost fingerprint:\n{}\n\nImport connection.json from the installation into the controller and independently compare this host fingerprint. The execution boundary is the disposable VM. Hypervisor isolation is externally configured and has not been verified by this worker.\n\nSave a copy of the installation result (保存安装结果)?",
                value["ssh_user"].as_str().unwrap_or(""),
                value["ssh_port"],
                value["host_fingerprint"].as_str().unwrap_or("")
            );
            // Post-install dialogs and the public copy are best effort: a save failure
            // must never turn a completed installation into a failed setup run.
            let save = message(&summary, MB_YESNO | MB_ICONINFORMATION)
                .map(|answer| answer == IDYES)
                .unwrap_or(false);
            if save {
                let export = match value["installation_id"].as_str() {
                    Some(installation_id) => {
                        save_install_result(installation_id, &value).and_then(report_export)
                    }
                    None => Err(anyhow::anyhow!(
                        "the installation result has no installation identity"
                    )),
                };
                if let Err(error) = export {
                    let _ = message(
                        &format!(
                            "The worker is installed, but the installation result copy could not be saved.\n\n{error:#}\n\nThe protected installation record remains available; start setup again without arguments for a refreshed export."
                        ),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
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

#[cfg(test)]
#[path = "setup_ui_tests.rs"]
mod tests;
