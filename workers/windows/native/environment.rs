//! Fixed environments for trusted elevated tools. Never inherit controller
//! credentials or use the ordinary worker's writable temporary directory.
use crate::security::{self, AclPolicy};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    ptr::null_mut,
};
use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath};

pub(crate) fn program_data() -> Result<PathBuf> {
    let mut path = null_mut();
    let result = unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, null_mut(), &mut path) };
    ensure!(
        result >= 0 && !path.is_null(),
        "cannot resolve Windows ProgramData (HRESULT 0x{:08x})",
        result as u32
    );
    struct Memory(*mut u16);
    impl Drop for Memory {
        fn drop(&mut self) {
            unsafe {
                windows::Win32::System::Com::CoTaskMemFree(Some(self.0.cast()));
            }
        }
    }
    let path = Memory(path);
    let mut length = 0;
    while length < 32768 && unsafe { *path.0.add(length) } != 0 {
        length += 1;
    }
    ensure!(length < 32768, "invalid Windows ProgramData path");
    let result = PathBuf::from(String::from_utf16(unsafe {
        std::slice::from_raw_parts(path.0, length)
    })?);
    security::no_links(&result)?;
    Ok(result)
}

pub(crate) fn system(root: &Path) -> Result<BTreeMap<String, OsString>> {
    system_with_policy(root, &AclPolicy::private_vm())
}

fn system_with_policy(root: &Path, private: &AclPolicy) -> Result<BTreeMap<String, OsString>> {
    let system = PathBuf::from(std::env::var_os("SystemRoot").context("missing Windows root")?);
    security::no_links(&system)?;
    let temporary = root.join("metadata/system-tmp");
    private.verify(&temporary, true)?;
    let path = std::env::join_paths([system.join("System32"), system.clone()])?;
    Ok([
        ("SYSTEMROOT".into(), system.clone().into_os_string()),
        ("WINDIR".into(), system.clone().into_os_string()),
        ("PROGRAMDATA".into(), program_data()?.into_os_string()),
        (
            "COMSPEC".into(),
            system.join("System32/cmd.exe").into_os_string(),
        ),
        ("PATH".into(), path),
        ("TEMP".into(), temporary.clone().into_os_string()),
        ("TMP".into(), temporary.into_os_string()),
    ]
    .into_iter()
    .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_tools_use_only_fixed_paths_and_reject_a_shared_writable_temp() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let root = temporary.path();
        let private = AclPolicy::current_user()?;
        private.create_directory(&root.join("metadata"))?;
        let scratch = root.join("metadata/system-tmp");
        private.create_directory(&scratch)?;
        let environment = system_with_policy(root, &private)?;
        assert_eq!(
            environment.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "COMSPEC",
                "PATH",
                "PROGRAMDATA",
                "SYSTEMROOT",
                "TEMP",
                "TMP",
                "WINDIR"
            ]
        );
        assert_eq!(environment["TEMP"], scratch.as_os_str());
        assert_eq!(environment["TMP"], scratch.as_os_str());
        let system = Path::new(&environment["SYSTEMROOT"]);
        assert_eq!(
            environment["COMSPEC"],
            system.join("System32/cmd.exe").as_os_str()
        );
        assert_eq!(
            std::env::split_paths(&environment["PATH"]).collect::<Vec<_>>(),
            [system.join("System32"), system.to_owned()]
        );
        private
            .clone()
            .with_account("S-1-5-21-111-222-333-444", true)?
            .apply(&scratch, true)?;
        assert!(system_with_policy(root, &private).is_err());
        // Restore the fixture ACL before its ordinary-user temporary cleanup.
        private.apply(&scratch, true)?;
        Ok(())
    }
}
