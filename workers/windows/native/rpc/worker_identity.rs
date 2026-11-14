//! OS boot binding belongs to the Windows worker, independently of Python.
use anyhow::{Context, Result, ensure};
use std::{ffi::c_void, mem, ptr};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use wukong_worker_protocol::{PROTOCOL, WorkerIdentity};

pub(crate) fn identity(installation: &str, package: &str) -> Result<WorkerIdentity> {
    // Resolve the native API dynamically, as required by Microsoft's contract:
    // https://learn.microsoft.com/windows/win32/api/winternl/nf-winternl-ntquerysysteminformation
    let name = crate::isolation::security::wide(std::ffi::OsStr::new("ntdll.dll"))?;
    let library = unsafe { GetModuleHandleW(name.as_ptr()) };
    ensure!(!library.is_null(), "Windows native runtime is unavailable");
    let address = unsafe { GetProcAddress(library, c"NtQuerySystemInformation".as_ptr().cast()) }
        .context("Windows native boot query is unavailable")?;
    type Query = unsafe extern "system" fn(i32, *mut c_void, u32, *mut u32) -> i32;
    let query: Query = unsafe { mem::transmute(address) };
    let mut environment = [0u8; 32];
    let mut returned = 0;
    let status = unsafe { query(90, environment.as_mut_ptr().cast(), 32, &mut returned) };
    ensure!(
        status >= 0,
        "native boot query failed: 0x{:08x}",
        status as u32
    );
    ensure!(
        matches!(returned, 24 | 32),
        "unsupported native boot environment length"
    );
    let mut shared = [0u8; 0x2c8];
    // Fixed read-only Windows user mapping; only copy the bounded known prefix.
    unsafe {
        ptr::copy_nonoverlapping(
            0x7ffe0000usize as *const u8,
            shared.as_mut_ptr(),
            shared.len(),
        );
    }
    let boot_id = decode(&environment[..returned as usize], &shared)?;
    let value = WorkerIdentity {
        protocol: PROTOCOL,
        installation_id: installation.into(),
        boot_id,
        package_sha256: package.into(),
    };
    value.validate()?;
    Ok(value)
}

fn decode(environment: &[u8], shared: &[u8]) -> Result<String> {
    ensure!(
        matches!(environment.len(), 24 | 32) && shared.len() == 0x2c8,
        "unsupported native boot information length"
    );
    let word = |offset| u32::from_le_bytes(shared[offset..offset + 4].try_into().unwrap());
    let architecture = u16::from_le_bytes(shared[0x26a..0x26c].try_into().unwrap());
    ensure!(
        word(0x26c) == 10 && word(0x260) & 0x0fffffff >= 10240 && matches!(architecture, 0 | 9),
        "unsupported Windows shared boot layout"
    );
    let entry = uuid::Uuid::from_bytes_le(environment[..16].try_into().unwrap());
    let firmware = u32::from_le_bytes(environment[16..20].try_into().unwrap());
    ensure!(
        !entry.is_nil() && matches!(firmware, 1 | 2),
        "native boot-entry identity unavailable"
    );
    Ok(format!("windows:{}:{}", entry.simple(), word(0x2c4)))
}

pub(crate) fn validate(identity: &WorkerIdentity) -> Result<()> {
    identity.validate()?;
    crate::rpc::pipe_transport::service_name(&identity.installation_id)?;
    let (entry, sequence) = identity
        .boot_id
        .strip_prefix("windows:")
        .and_then(|boot| boot.split_once(':'))
        .context("invalid native boot binding")?;
    ensure!(
        entry.len() == 32
            && entry
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            && entry.bytes().any(|b| b != b'0')
            && sequence.parse::<u32>()?.to_string() == sequence,
        "invalid native boot binding"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boot_binding_is_stable_across_queries_and_retains_sequence() -> Result<()> {
        let one = identity(&"a".repeat(32), &"b".repeat(64))?;
        assert_eq!(one, identity(&"a".repeat(32), &"b".repeat(64))?);
        assert_eq!(one.boot_id.split(':').count(), 3);
        assert!(decode(&[0; 32], &[0; 0x2c8]).is_err());
        Ok(())
    }
}
