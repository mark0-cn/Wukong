//! Local debugger paths need final-object access, not metadata access to every
//! ancestor. Let Windows reject reparses during the actual open instead.
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    mem::size_of,
    os::windows::{fs::MetadataExt, io::FromRawHandle},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{FILE_OPEN, FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile},
    },
    Win32::{
        Foundation::{
            OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE, RtlNtStatusToDosError, UNICODE_STRING,
        },
        Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, QueryDosDeviceW, SYNCHRONIZE,
        },
        System::IO::IO_STATUS_BLOCK,
    },
};

pub(super) fn local_path(path: &Path) -> Result<PathBuf> {
    let name = path
        .to_str()
        .context("DbgEng paths must be Unicode")?
        .replace('/', "\\");
    let drive = name.strip_prefix(r"\\?\").unwrap_or(&name);
    ensure!(
        drive.as_bytes().get(1) == Some(&b':')
            && drive.as_bytes()[0].is_ascii_alphabetic()
            && !drive.contains([';', '\0'])
            && !drive
                .split('\\')
                .skip(1)
                .any(|part| part.ends_with('.') || part.ends_with(' ') || part.contains(':')),
        "DbgEng requires an unambiguous local drive path"
    );
    let path = PathBuf::from(drive);
    ensure!(
        path.is_absolute()
            && !path.components().any(|component| matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )),
        "DbgEng requires an absolute local path without relative aliases"
    );

    // A DOS drive letter is itself an Object Manager link. Passing it directly
    // with OBJ_DONT_REPARSE rejects even ordinary files on Windows 10.
    // Query the current mapping through the SDK, accepting only a local disk
    // volume, then have NtCreateFile refuse every filesystem reparse in one open.
    let device_name: Vec<u16> = drive[..2].encode_utf16().chain(Some(0)).collect();
    let mut mapping = vec![0u16; 32768];
    let count = unsafe {
        QueryDosDeviceW(
            device_name.as_ptr(),
            mapping.as_mut_ptr(),
            mapping.len() as u32,
        )
    } as usize;
    if count == 0 {
        return Err(std::io::Error::last_os_error())
            .context("resolve local debugger drive mapping");
    }
    ensure!(
        count <= mapping.len(),
        "invalid debugger drive mapping length"
    );
    let end = mapping[..count]
        .iter()
        .position(|value| *value == 0)
        .context("unterminated debugger drive mapping")?;
    // QueryDosDevice returns the current mapping first; subsequent strings are
    // undeleted prior mappings, not alternatives to try after a rejected open.
    let mapping = String::from_utf16(&mapping[..end])?;
    ensure!(
        mapping
            .strip_prefix(r"\Device\HarddiskVolume")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())),
        "DbgEng requires a local disk-volume mapping without device aliases"
    );
    let mut encoded: Vec<u16> = format!("{mapping}{}", &drive[2..])
        .encode_utf16()
        .chain(Some(0))
        .collect();
    ensure!(
        encoded.len() <= 32767,
        "DbgEng path exceeds the NT name limit"
    );
    let object_name = UNICODE_STRING {
        Length: ((encoded.len() - 1) * 2) as u16,
        MaximumLength: (encoded.len() * 2) as u16,
        Buffer: encoded.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        ObjectName: &object_name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        ..Default::default()
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = null_mut();
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            &attributes,
            &mut status,
            null(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    if result < 0 {
        return Err(std::io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ))
        .with_context(|| {
            format!(
                "open local debugger path without reparsing {} (NTSTATUS {result:#010x})",
                path.display()
            )
        });
    }
    ensure!(
        !handle.is_null(),
        "Windows returned no debugger path handle"
    );
    // The SDK opened an existing object with normal access checks; this handle
    // never carries an inheritable flag or security-management rights.
    let file = unsafe { File::from_raw_handle(handle) };
    ensure!(
        file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "DbgEng paths cannot contain links or reparse points"
    );
    // DbgEng treats a \\?\ drive path as network access when that is disabled.
    // Preserve the verified drive spelling for its local-only loader.
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_paths_preserve_existing_files_and_reject_networks_and_aliases() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("known 中文 fixture.bin");
        std::fs::write(&path, b"inert-known-fixture")?;
        assert_eq!(local_path(&path)?, path);
        assert_eq!(
            local_path(Path::new(&path.to_string_lossy().replace('\\', "/")))?,
            path
        );
        assert_eq!(
            local_path(Path::new(&format!(r"\\?\{}", path.display())))?,
            path
        );
        let missing = directory.path().join("missing.bin");
        assert!(local_path(&missing).is_err());
        assert!(!missing.exists());
        for invalid in [
            r"\\server\share\file",
            r"C:\file:stream",
            r"relative\file",
            r"C:\dir.\file",
            r"C:\dir\..\file",
        ] {
            assert!(local_path(Path::new(invalid)).is_err());
        }
        Ok(())
    }

    #[test]
    fn local_paths_reject_final_and_intermediate_junctions() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let target = directory.path().join("plain");
        std::fs::create_dir(&target)?;
        std::fs::write(target.join("known.bin"), b"inert-known-fixture")?;
        let link = directory.path().join("linked");
        junction::create(&target, &link)?;
        assert!(local_path(&target).is_ok());
        assert!(local_path(&link).is_err());
        assert!(local_path(&link.join("known.bin")).is_err());
        junction::delete(link)?;
        Ok(())
    }
}
