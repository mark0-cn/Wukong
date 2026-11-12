//! Read-only SDK path observations shared by the System host and debugger.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs::File, os::windows::io::AsRawHandle};
use windows_sys::Win32::{
    Globalization::{CSTR_EQUAL, CompareStringOrdinal},
    Storage::FileSystem::{
        FILE_NAME_OPENED, GetFinalPathNameByHandleW, VOLUME_NAME_DOS, VOLUME_NAME_NT,
    },
};
pub const MAX_CONTEXT_BYTES: usize = 8192;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryIdentity {
    pub dos: String,
    pub native: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathContext {
    pub schema: u32,
    pub engine: DirectoryIdentity,
    pub symbols: DirectoryIdentity,
}
impl DirectoryIdentity {
    pub fn observe(file: &File, requested: &str) -> Result<Self> {
        let requested = requested.replace('/', "\\");
        let requested = requested.strip_prefix(r"\\?\").unwrap_or(&requested);
        let dos = opened_name(file, VOLUME_NAME_DOS)?;
        let dos = dos.strip_prefix(r"\\?\").unwrap_or(&dos);
        ensure!(
            same_name(dos, requested)?,
            "opened debugger drive identity differs (requested={requested}, actual={dos})"
        );
        Ok(Self {
            dos: dos.into(),
            native: opened_name(file, VOLUME_NAME_NT)?,
        })
    }
}
pub fn opened_name(file: &File, volume: u32) -> Result<String> {
    let mut buffer = vec![0u16; 32768];
    let count = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            FILE_NAME_OPENED | volume,
        )
    } as usize;
    if count == 0 {
        return Err(std::io::Error::last_os_error()).context("query opened debugger path name");
    }
    ensure!(
        count < buffer.len(),
        "opened debugger path exceeds the name bound"
    );
    String::from_utf16(&buffer[..count]).context("opened debugger path is not Unicode")
}

pub fn same_name(a: &str, b: &str) -> Result<bool> {
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    ensure!(
        a.len() <= 32767 && b.len() <= 32767,
        "debugger identity exceeds the name bound"
    );
    let result =
        unsafe { CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1) };
    if result == 0 {
        return Err(std::io::Error::last_os_error())
            .context("compare opened debugger path identity");
    }
    Ok(result == CSTR_EQUAL)
}
