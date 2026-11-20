//! Explicit status-only SCM access for the installation's noninteractive relay.
//! This module never changes the SCM manager or another installation's service.
use crate::{
    isolation::security::{self, SecurityDescriptor},
    rpc::pipe_transport,
};
use anyhow::{Result, ensure};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    mem::size_of,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Security::{
        Authorization::{SE_SERVICE, SetSecurityInfo},
        DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
        OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    },
    Storage::FileSystem::READ_CONTROL,
    System::Services::*,
};

struct Service(SC_HANDLE);
impl Drop for Service {
    fn drop(&mut self) {
        // SAFETY: Service wraps an OpenSCManagerW/OpenServiceW result (null only on failure,
        // which CloseServiceHandle rejects), and closes that handle exactly once here.
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

fn grants(management: &str) -> Result<BTreeMap<String, u32>> {
    security::validate_account_sid(management)?;
    Ok([
        ("S-1-5-18".into(), SERVICE_ALL_ACCESS),
        ("S-1-5-32-544".into(), SERVICE_ALL_ACCESS),
        (management.into(), SERVICE_QUERY_STATUS),
    ]
    .into_iter()
    .collect())
}
fn descriptor(management: &str) -> Result<SecurityDescriptor> {
    let entries = grants(management)?
        .into_iter()
        .map(|(sid, mask)| format!("(A;;0x{mask:08x};;;{sid})"))
        .collect::<String>();
    SecurityDescriptor::from_sddl(&format!("O:BAG:BAD:P{entries}"))
}
fn verify(handle: SC_HANDLE, management: &str) -> Result<()> {
    let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut size = 0;
    // SAFETY: handle is the caller's live service handle; a null buffer with zero length is
    // the documented size query, and size is a writable out parameter.
    unsafe {
        QueryServiceObjectSecurity(handle, information, null_mut(), 0, &mut size);
    }
    ensure!(
        size > 0 && size <= 4096,
        "invalid owned service descriptor size"
    );
    let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    // SAFETY: buffer is a live, usize-aligned allocation of at least size bytes from the
    // preceding query; handle is still live and size is updated to the bytes written.
    security::check(unsafe {
        QueryServiceObjectSecurity(
            handle,
            information,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    })?;
    security::verify_descriptor(
        buffer.as_mut_ptr().cast(),
        "S-1-5-32-544",
        &grants(management)?,
        false,
    )
}

/// Called only for the newly created, stopped service, before its first start.
pub(crate) fn apply(handle: SC_HANDLE, management: &str) -> Result<()> {
    let descriptor = descriptor(management)?;
    let raw = descriptor.attributes().lpSecurityDescriptor;
    let (mut owner, mut acl) = (null_mut(), null_mut());
    let (mut defaulted, mut present) = (0, 0);
    // SAFETY: raw points at the live SecurityDescriptor; owner and defaulted are writable
    // out parameters, and owner stays inside that descriptor while used.
    security::check(unsafe { GetSecurityDescriptorOwner(raw, &mut owner, &mut defaulted) })?;
    // SAFETY: raw is the same live descriptor; present, acl and defaulted are writable
    // locals, and acl points into that descriptor while it is still alive.
    security::check(unsafe {
        GetSecurityDescriptorDacl(raw, &mut present, &mut acl, &mut defaulted)
    })?;
    ensure!(present != 0 && !acl.is_null(), "missing owned service ACL");
    // SAFETY: handle is the caller's live service handle; owner and acl point into the live
    // descriptor above, and only that service's owner/DACL is written.
    let status = unsafe {
        SetSecurityInfo(
            handle,
            SE_SERVICE,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            owner,
            null_mut(),
            acl,
            null(),
        )
    };
    ensure!(
        status == 0,
        "cannot set owned service status-only access: {status}"
    );
    verify(handle, management)
}

pub(crate) fn verify_registered(installation: &str, management: &str) -> Result<()> {
    security::require_system()?;
    // SAFETY: null database and machine names select the local SCM; the returned handle is
    // checked non-null before use and owned by Service, which closes it on drop.
    let manager = Service(unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) });
    ensure!(
        !manager.0.is_null(),
        "cannot inspect owned service manager: {}",
        std::io::Error::last_os_error()
    );
    let name = security::wide(OsStr::new(&pipe_transport::service_name(installation)?))?;
    // SAFETY: manager.0 is the live SCM handle checked above; name is a NUL-terminated wide
    // service name alive through the call, and the returned handle is owned by Service.
    let handle = Service(unsafe { OpenServiceW(manager.0, name.as_ptr(), READ_CONTROL) });
    ensure!(
        !handle.0.is_null(),
        "cannot inspect owned service access: {}",
        std::io::Error::last_os_error()
    );
    verify(handle.0, management)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isolation::security::Handle;
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            AccessCheck, Authorization::ConvertStringSidToSidW, CreateRestrictedToken,
            DuplicateToken, GENERIC_MAPPING, SID_AND_ATTRIBUTES, SecurityImpersonation,
            TOKEN_DUPLICATE, TOKEN_QUERY,
        },
        Storage::FileSystem::{WRITE_DAC, WRITE_OWNER},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    struct Sid(*mut std::ffi::c_void);
    impl Drop for Sid {
        fn drop(&mut self) {
            // SAFETY: Sid owns the LocalAlloc buffer returned by ConvertStringSidToSidW in sid()
            // and releases it exactly once.
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    fn sid(value: &str) -> Result<Sid> {
        let value = security::wide(OsStr::new(value))?;
        let mut sid = null_mut();
        // SAFETY: value is a NUL-terminated wide SID alive for the call; sid is a writable out
        // pointer whose LocalAlloc result is owned by the returned Sid.
        security::check(unsafe { ConvertStringSidToSidW(value.as_ptr(), &mut sid) })?;
        Ok(Sid(sid))
    }
    #[test]
    fn management_service_access_allows_only_status_with_real_restricted_access_checks()
    -> Result<()> {
        // In-memory object and current-user token only: no service/account,
        // global policy or impersonation is created on the development host.
        let management = security::current_user_sid()?;
        let descriptor = descriptor(&management)?;
        security::verify_descriptor(
            descriptor.attributes().lpSecurityDescriptor,
            "S-1-5-32-544",
            &grants(&management)?,
            false,
        )?;
        let management_sid = sid(&management)?;
        let administrators = sid("S-1-5-32-544")?;
        let restriction = SID_AND_ATTRIBUTES {
            Sid: management_sid.0,
            Attributes: 0,
        };
        let disabled = SID_AND_ATTRIBUTES {
            Sid: administrators.0,
            Attributes: 0,
        };
        let mut original = null_mut();
        // SAFETY: GetCurrentProcess is this process's non-closable pseudo-handle; original is a
        // writable out pointer, and the returned token handle is owned by Handle.
        security::check(unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE,
                &mut original,
            )
        })?;
        let original = Handle(original);
        let mut restricted = null_mut();
        // SAFETY: original.0 is the live token; disabled/restriction are the declared one-element
        // SID_AND_ATTRIBUTES arrays backed by live Sid allocations, and restricted is writable.
        security::check(unsafe {
            CreateRestrictedToken(
                original.0,
                // Test ordinary-account ACL rights, without an administrator's
                // enabled SeTakeOwnershipPrivilege bypassing WRITE_OWNER.
                windows_sys::Win32::Security::DISABLE_MAX_PRIVILEGE,
                1,
                &disabled,
                0,
                null(),
                1,
                &restriction,
                &mut restricted,
            )
        })?;
        let restricted = Handle(restricted);
        let mut token = null_mut();
        // SAFETY: restricted.0 is the live restricted token; token is a writable out pointer,
        // and the duplicate it receives is owned by Handle.
        security::check(unsafe {
            DuplicateToken(restricted.0, SecurityImpersonation, &mut token)
        })?;
        let token = Handle(token);
        let mapping = GENERIC_MAPPING {
            GenericRead: SERVICE_QUERY_STATUS,
            GenericWrite: SERVICE_CHANGE_CONFIG,
            GenericExecute: SERVICE_START | SERVICE_STOP,
            GenericAll: SERVICE_ALL_ACCESS,
        };
        for desired in [
            SERVICE_QUERY_STATUS,
            SERVICE_START,
            SERVICE_STOP,
            SERVICE_CHANGE_CONFIG,
            READ_CONTROL,
            WRITE_DAC,
            WRITE_OWNER,
        ] {
            let mut privileges = [0u64; 1024];
            let mut size = std::mem::size_of_val(&privileges) as u32;
            let (mut granted, mut allowed) = (0, 0);
            // SAFETY: descriptor is the live SDDL descriptor and token.0 the live token;
            // privileges is a usize-aligned size-byte buffer and all four out values are writable.
            security::check(unsafe {
                AccessCheck(
                    descriptor.attributes().lpSecurityDescriptor,
                    token.0,
                    desired,
                    &mapping,
                    privileges.as_mut_ptr().cast(),
                    &mut size,
                    &mut granted,
                    &mut allowed,
                )
            })?;
            assert_eq!(
                allowed != 0,
                desired == SERVICE_QUERY_STATUS,
                "unexpected service access {desired:#x}"
            );
        }
        Ok(())
    }
}
