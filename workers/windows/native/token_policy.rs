//! Exact sandbox token policy shared by suspended-child admission, the fixed
//! resource probe.
use anyhow::{Result, ensure};
use std::{mem::size_of, ptr::null};
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_PRIVILEGES, TokenPrivileges};

pub(crate) fn traverse_privilege() -> Result<windows_sys::Win32::Foundation::LUID> {
    use windows_sys::Win32::Security::{LookupPrivilegeValueW, SE_CHANGE_NOTIFY_NAME};
    let mut luid = windows_sys::Win32::Foundation::LUID::default();
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

/// Verify the OS network boundary independently of local path validation.
pub(crate) fn verify_network_isolation(
    token: windows_sys::Win32::Foundation::HANDLE,
) -> Result<()> {
    use windows_sys::Win32::Security::{TOKEN_GROUPS, TokenCapabilities, TokenIsAppContainer};
    let mut app = 0u32;
    let mut returned = 0;
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
        "network isolation requires an AppContainer token"
    );
    // A zero-capability token fits the SDK's fixed structure. Larger responses
    // fail at the SDK call rather than allocating an attacker-sized token list.
    let mut groups = TOKEN_GROUPS::default();
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
        "network-isolated AppContainer cannot have capabilities"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_ordinary_token_cannot_satisfy_the_resource_network_policy() -> Result<()> {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            Security::TOKEN_QUERY,
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        };
        let mut token = std::ptr::null_mut();
        windows::core::BOOL(unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
        })
        .ok()?;
        let result = verify_network_isolation(token);
        unsafe { CloseHandle(token) };
        assert!(result.unwrap_err().to_string().contains("AppContainer"));
        Ok(())
    }
}
