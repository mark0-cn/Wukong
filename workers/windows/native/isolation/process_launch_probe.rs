//! Bounded observations for the fixed signed self-test before its first instruction.
//! Never selects a caller's program/PID or changes the sealed token or access policy.
use super::{GetCurrentProcess, Handle, OpenProcessToken, Result, security};

use super::{
    BTreeMap, HANDLE, OsString, OwnedProcess, Path, SecurityDescriptor, ensure, null, null_mut,
};

use serde_json::{Value, json};
use std::{collections::BTreeSet, os::windows::ffi::OsStringExt, path::PathBuf};
use windows_sys::Win32::{
    Foundation::{HMODULE, INVALID_HANDLE_VALUE},
    Security::{Authorization::SE_WINDOW_OBJECT, TOKEN_DUPLICATE, TOKEN_QUERY},
    Storage::FileSystem::{
        CreateFileW, FILE_EXECUTE, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES,
        FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE,
        OPEN_EXISTING, SYNCHRONIZE,
    },
    System::ProcessStatus::{K32EnumProcessModules, K32GetModuleFileNameExW},
};

fn modules() -> Result<Vec<PathBuf>> {
    let mut handles: [HMODULE; 256] = [null_mut(); 256];
    let mut needed = 0;
    security::check(unsafe {
        K32EnumProcessModules(
            GetCurrentProcess(),
            handles.as_mut_ptr(),
            size_of_val(&handles) as u32,
            &mut needed,
        )
    })?;
    ensure!(
        needed as usize <= size_of_val(&handles)
            && (needed as usize).is_multiple_of(size_of::<HMODULE>()),
        "unbounded self-test module list"
    );
    handles[..needed as usize / size_of::<HMODULE>()]
        .iter()
        .map(|handle| {
            let mut path = vec![0u16; 32768];
            let count = unsafe {
                K32GetModuleFileNameExW(
                    GetCurrentProcess(),
                    *handle,
                    path.as_mut_ptr(),
                    path.len() as u32,
                )
            } as usize;
            ensure!(
                count > 0 && count < path.len(),
                "invalid self-test module path"
            );
            Ok(PathBuf::from(OsString::from_wide(&path[..count])))
        })
        .collect()
}

fn access(path: &Path, directory: bool) -> Result<Option<i32>> {
    let name = security::wide(path.as_os_str())?;
    let rights = if directory {
        FILE_TRAVERSE | SYNCHRONIZE
    } else {
        FILE_READ_DATA | FILE_EXECUTE | FILE_READ_ATTRIBUTES | SYNCHRONIZE
    };
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            rights,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            if directory {
                FILE_FLAG_BACKUP_SEMANTICS
            } else {
                0
            },
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Ok(Some(
            std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
        ));
    }
    let _handle = Handle(handle);
    Ok(None)
}

fn label(handle: HANDLE) -> Value {
    let result = SecurityDescriptor::from_object(handle, SE_WINDOW_OBJECT).and_then(|descriptor| {
        crate::isolation::process_access::verify_integrity_label(
            descriptor.attributes().lpSecurityDescriptor,
        )
    });
    match result {
        Ok(()) => json!({"low_no_write_up":true}),
        Err(error) => json!({"error":format!("{error:#}").chars().take(192).collect::<String>()}),
    }
}

pub(super) fn inspect(
    process: &OwnedProcess,
    directory: &Path,
    environment: &BTreeMap<String, OsString>,
) -> Result<Value> {
    security::require_system()?;
    let files = modules()?;
    ensure!(
        files.contains(&std::env::current_exe()?),
        "self-test module is missing"
    );
    let mut paths = BTreeSet::new();
    for file in &files {
        paths.insert((file.clone(), false));
        for parent in file.ancestors().skip(1) {
            paths.insert((parent.to_owned(), true));
        }
    }
    for path in std::iter::once(directory).chain(
        ["APPDATA", "LOCALAPPDATA", "USERPROFILE"]
            .into_iter()
            .filter_map(|key| environment.get(key).map(Path::new)),
    ) {
        for parent in path.ancestors() {
            paths.insert((parent.to_owned(), true));
        }
    }
    ensure!(
        paths.len() <= 512,
        "unbounded self-test startup access list"
    );
    let mut token = null_mut();
    security::check(unsafe {
        OpenProcessToken(process.process.0, TOKEN_QUERY | TOKEN_DUPLICATE, &mut token)
    })?;
    let token = Handle(token);
    let mut denied = 0;
    let mut observations = Vec::new();
    crate::isolation::persona::as_user(&token, || {
        for (path, directory) in &paths {
            if let Some(error) = access(path, *directory)? {
                denied += 1;
                if observations.len() < 8 {
                    observations.push(json!({"path":path.to_string_lossy().chars().take(200).collect::<String>(),
                        "operation":if *directory { "traverse" } else { "read_execute" }, "win32_error":error}));
                }
            }
        }
        Ok(())
    })?;
    Ok(
        json!({"scope":"sealed_selftest_startup_observation", "checked":paths.len(),
        "parent_modules":files.len(), "denied":denied, "denials":observations,
        "denials_truncated":denied > observations.len(),
        "station_label":process._desktop.as_ref().map(|desktop| label(desktop.station)),
        "desktop_label":process._desktop.as_ref().map(|desktop| label(desktop.desktop)),
        "execution_fallback":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_modules_can_be_opened_without_parsing_their_images() -> Result<()> {
        let files = modules()?;
        assert!(files.contains(&std::env::current_exe()?));
        for file in files {
            assert_eq!(access(&file, false)?, None, "{}", file.display());
        }
        let temporary = tempfile::tempdir()?;
        assert_eq!(access(temporary.path(), true)?, None);
        assert_eq!(
            access(&temporary.path().join("absent.dll"), false)?,
            Some(2)
        );
        Ok(())
    }
}
