//! Exact sandbox token policy shared by suspended-child admission, the fixed
//! resource probe.
use anyhow::{Result, ensure};
use std::{mem::size_of, ptr::null};
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_PRIVILEGES, TokenPrivileges};

pub(crate) fn traverse_privilege() -> Result<windows_sys::Win32::Foundation::LUID> {
    use windows_sys::Win32::Security::{LookupPrivilegeValueW, SE_CHANGE_NOTIFY_NAME};
    let mut luid = windows_sys::Win32::Foundation::LUID::default();
    // SAFETY: `null()` selects the local system as the name source and
    // `SE_CHANGE_NOTIFY_NAME` is a NUL-terminated static; `luid` is a valid out-parameter.
    windows::core::BOOL(unsafe { LookupPrivilegeValueW(null(), SE_CHANGE_NOTIFY_NAME, &mut luid) })
        .ok()?;
    Ok(luid)
}
pub(crate) fn same_privilege(
    a: &windows_sys::Win32::Foundation::LUID,
    b: &windows_sys::Win32::Foundation::LUID,
) -> bool {
    a.LowPart == b.LowPart && a.HighPart == b.HighPart
}
/// Verify the exact OS privilege identity, not just its count. This privilege
/// bypasses ancestor traversal; final object read/write/execute ACLs still apply.
pub(crate) fn verify_privileges(token: windows_sys::Win32::Foundation::HANDLE) -> Result<()> {
    use windows_sys::Win32::Security::SE_PRIVILEGE_ENABLED;
    let mut privileges = TOKEN_PRIVILEGES::default();
    let mut bytes = 0;
    // SAFETY: `privileges` is a TOKEN_PRIVILEGES buffer of exactly `size_of` bytes; a larger
    // token privilege list makes the call fail with ERROR_INSUFFICIENT_BUFFER instead.
    windows::core::BOOL(unsafe {
        GetTokenInformation(
            token,
            TokenPrivileges,
            (&mut privileges as *mut TOKEN_PRIVILEGES).cast(),
            size_of::<TOKEN_PRIVILEGES>() as u32,
            &mut bytes,
        )
    })
    .ok()?;
    ensure!(
        privileges.PrivilegeCount == 1
            && same_privilege(&privileges.Privileges[0].Luid, &traverse_privilege()?)
            && privileges.Privileges[0].Attributes & SE_PRIVILEGE_ENABLED != 0,
        "sandbox token must retain only enabled SeChangeNotifyPrivilege"
    );
    Ok(())
}

/// Read back guest token restrictions; this does not verify hypervisor isolation.
pub(crate) fn verify_appcontainer_policy(
    token: windows_sys::Win32::Foundation::HANDLE,
) -> Result<()> {
    use windows_sys::Win32::Security::{TOKEN_GROUPS, TokenCapabilities, TokenIsAppContainer};
    let mut app = 0u32;
    let mut returned = 0;
    // SAFETY: `app` is a u32 out-buffer of exactly the passed size, and the following
    // `ensure!` requires the API to have written exactly that many bytes.
    windows::core::BOOL(unsafe {
        GetTokenInformation(
            token,
            TokenIsAppContainer,
            (&mut app as *mut u32).cast(),
            size_of::<u32>() as u32,
            &mut returned,
        )
    })
    .ok()?;
    ensure!(
        returned as usize == size_of::<u32>() && app == 1,
        "guest controls require an AppContainer token"
    );
    // A zero-capability token fits the SDK's fixed structure. Larger responses
    // fail at the SDK call rather than allocating an attacker-sized token list.
    let mut groups = TOKEN_GROUPS::default();
    // SAFETY: `groups` is a fixed TOKEN_GROUPS buffer of exactly the passed size, and the
    // `ensure!` below rejects any nonzero group count before the flexible array is read.
    windows::core::BOOL(unsafe {
        GetTokenInformation(
            token,
            TokenCapabilities,
            (&mut groups as *mut TOKEN_GROUPS).cast(),
            size_of::<TOKEN_GROUPS>() as u32,
            &mut returned,
        )
    })
    .ok()?;
    ensure!(
        returned >= 4 && groups.GroupCount == 0,
        "guest AppContainer cannot have capabilities"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_ordinary_token_cannot_satisfy_the_appcontainer_policy() -> Result<()> {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            Security::TOKEN_QUERY,
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        };
        let mut token = std::ptr::null_mut();
        // SAFETY: `GetCurrentProcess` returns a pseudo-handle (never closed); on success
        // `token` is a real owned handle, closed by the explicit `CloseHandle` below.
        windows::core::BOOL(unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
        })
        .ok()?;
        let result = verify_appcontainer_policy(token);
        // SAFETY: `token` was opened successfully above and is closed exactly once, here,
        // after its last use; the `.ok()?` above returned early on failure.
        unsafe { CloseHandle(token) };
        assert!(result.unwrap_err().to_string().contains("AppContainer"));
        Ok(())
    }
}
