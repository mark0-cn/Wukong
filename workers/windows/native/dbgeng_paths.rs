#[path = "debugger_path_context.rs"]
mod context;
#[path = "security_descriptor.rs"]
mod descriptor;
use anyhow::{Context, Result, ensure};
pub(super) use context::PathContext;
use context::{opened_name, same_name};
use std::{
    fs::File,
    mem::size_of,
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::FromRawHandle,
    },
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
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            SYNCHRONIZE, VOLUME_NAME_NT,
        },
        System::IO::IO_STATUS_BLOCK,
    },
};

pub(super) fn read_context(input: &mut std::io::StdinLock<'_>) -> Result<PathContext> {
    use std::{
        io::{BufRead, Read},
        os::windows::io::AsRawHandle,
    };
    verify_context_authority(input.as_raw_handle())?;
    let mut line = Vec::new();
    let count = input
        .by_ref()
        .take((context::MAX_CONTEXT_BYTES + 1) as u64)
        .read_until(b'\n', &mut line)?;
    ensure!(
        count <= context::MAX_CONTEXT_BYTES && line.last() == Some(&b'\n'),
        "native debugger context exceeds its frame bound"
    );
    let context: PathContext = serde_json::from_slice(&line)?;
    ensure!(context.schema == 1, "unsupported native debugger context");
    Ok(context)
}

fn verify_context_authority(handle: std::os::windows::io::RawHandle) -> Result<()> {
    use windows_sys::Win32::{
        Security::{
            Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
            DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
        },
        Storage::FileSystem::{FILE_ALL_ACCESS, FILE_TYPE_DISK, GetFileType},
    };
    ensure!(
        unsafe { GetFileType(handle) } == FILE_TYPE_DISK,
        "native debugger context requires an inherited private file"
    );
    let mut actual = null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut actual,
        )
    };
    let actual = descriptor::Local(actual);
    ensure!(
        status == 0,
        "cannot inspect native debugger context authority: {status}"
    );
    let grants = [
        ("S-1-5-18".into(), FILE_ALL_ACCESS),
        ("S-1-5-32-544".into(), FILE_ALL_ACCESS),
    ]
    .into_iter()
    .collect();
    descriptor::verify_descriptor(actual.0, "S-1-5-32-544", &grants, true)
        .context("native debugger context is not System/administrator-private")?;
    Ok(())
}

pub(super) fn local_path(path: &Path, bootstrap: Option<&PathContext>) -> Result<PathBuf> {
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

    // AppContainers cannot query the DOS-device namespace. Resolve only an
    // already accessible object: FILE_NAME_OPENED avoids normalized-name
    // queries that require metadata access to closed ancestors. Reject a
    // different DOS identity, and retain the original drive-relative suffix
    // for the independent kernel no-reparse open below.
    let opened = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path)
        .context("open local debugger path attributes")?;
    ensure!(
        opened.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "DbgEng paths cannot contain links or reparse points"
    );
    let native =
        opened_name(&opened, VOLUME_NAME_NT).context("read opened debugger NT identity")?;
    if let Some(context) = bootstrap {
        let selected = if same_name(&context.engine.dos, drive)? {
            &context.engine
        } else if same_name(&context.symbols.dos, drive)? {
            &context.symbols
        } else {
            anyhow::bail!("private debugger context does not authorize the requested path");
        };
        ensure!(
            same_name(&native, &selected.native)?,
            "private debugger NT path identity differs"
        );
    } else {
        let identity = context::DirectoryIdentity::observe(&opened, drive)?;
        ensure!(
            same_name(&native, &identity.native)?,
            "opened debugger NT identity changed"
        );
    }
    let prefix = r"\Device\HarddiskVolume";
    let suffix = native
        .strip_prefix(prefix)
        .context("DbgEng requires a local disk-volume identity")?;
    let end = suffix.find('\\').unwrap_or(suffix.len());
    ensure!(
        end > 0 && suffix[..end].bytes().all(|b| b.is_ascii_digit()),
        "DbgEng requires a local disk-volume identity without device aliases"
    );
    let mapping = &native[..prefix.len() + end];
    ensure!(
        same_name(&native, &format!("{mapping}{}", &drive[2..]))?,
        "opened debugger NT identity differs from the original drive-relative path"
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
    use std::os::windows::io::AsRawHandle;
    #[test]
    fn native_context_rejects_ordinary_files_and_pipes_before_reading_a_header() -> Result<()> {
        let file = tempfile::tempfile()?;
        let error = verify_context_authority(file.as_raw_handle())
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("not System/administrator-private"));
        let (reader, _writer) = std::io::pipe()?;
        let error = verify_context_authority(reader.as_raw_handle())
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("requires an inherited private file"));
        Ok(())
    }

    #[test]
    fn native_context_binds_both_the_requested_and_actual_nt_path() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let path = temporary.path().join("known.bin");
        std::fs::write(&path, b"inert fixture")?;
        let file = File::open(&path)?;
        let mut context = PathContext {
            schema: 1,
            engine: context::DirectoryIdentity::observe(&file, path.to_str().unwrap())?,
            symbols: context::DirectoryIdentity::observe(&file, path.to_str().unwrap())?,
        };
        assert_eq!(local_path(&path, Some(&context))?, path);
        context.engine.native.push_str("-different-object");
        assert!(local_path(&path, Some(&context)).is_err());
        let other = temporary.path().join("other.bin");
        std::fs::write(&other, b"inert different fixture")?;
        assert!(local_path(&other, Some(&context)).is_err());
        Ok(())
    }
    #[test]
    fn local_paths_preserve_existing_files_and_reject_networks_and_aliases() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("known 中文 fixture.bin");
        std::fs::write(&path, b"inert-known-fixture")?;
        assert_eq!(local_path(&path, None)?, path);
        assert_eq!(
            local_path(Path::new(&path.to_string_lossy().replace('\\', "/")), None)?,
            path
        );
        assert_eq!(
            local_path(Path::new(&format!(r"\\?\{}", path.display())), None)?,
            path
        );
        let missing = directory.path().join("missing.bin");
        assert!(local_path(&missing, None).is_err());
        assert!(!missing.exists());
        for invalid in [
            r"\\server\share\file",
            r"C:\file:stream",
            r"relative\file",
            r"C:\dir.\file",
            r"C:\dir\..\file",
        ] {
            assert!(local_path(Path::new(invalid), None).is_err());
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
        assert!(local_path(&target, None).is_ok());
        assert!(local_path(&link, None).is_err());
        assert!(local_path(&link.join("known.bin"), None).is_err());
        junction::delete(link)?;
        Ok(())
    }
}
