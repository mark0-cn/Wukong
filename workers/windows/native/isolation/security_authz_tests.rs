//! Shared inert Authz fixture. Never creates an account, token lowbox or profile.
use super::{Handle, SecurityDescriptor};
use crate::isolation::security;
use anyhow::{Context as _, Result};
use std::{
    ffi::OsStr,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        Authorization::{
            AUTHZ_ACCESS_REPLY, AUTHZ_ACCESS_REQUEST, AUTHZ_CLIENT_CONTEXT_HANDLE,
            AUTHZ_RESOURCE_MANAGER_HANDLE, AUTHZ_RM_FLAG_NO_AUDIT, AuthzAccessCheck,
            AuthzAddSidsToContext, AuthzFreeContext, AuthzFreeResourceManager,
            AuthzInitializeContextFromToken, AuthzInitializeResourceManager,
            ConvertStringSidToSidW,
        },
        SID_AND_ATTRIBUTES,
    },
};
pub(crate) fn allows(
    descriptor: &SecurityDescriptor,
    original: &Handle,
    restriction: &str,
    desired: u32,
) -> Result<bool> {
    let text = security::wide(OsStr::new(restriction))?;
    let mut sid = null_mut();
    security::check(unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut sid) })
        .context("parse inert resource restriction")?;
    struct Sid(*mut std::ffi::c_void);
    impl Drop for Sid {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    let sid = Sid(sid);
    let restriction = SID_AND_ATTRIBUTES {
        Sid: sid.0,
        Attributes: 4,
    };
    struct Manager(AUTHZ_RESOURCE_MANAGER_HANDLE);
    impl Drop for Manager {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    AuthzFreeResourceManager(self.0);
                }
            }
        }
    }
    struct Context(AUTHZ_CLIENT_CONTEXT_HANDLE);
    impl Drop for Context {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    AuthzFreeContext(self.0);
                }
            }
        }
    }
    let mut manager = Manager(null_mut());
    security::check(unsafe {
        AuthzInitializeResourceManager(
            AUTHZ_RM_FLAG_NO_AUDIT,
            None,
            None,
            None,
            null(),
            &mut manager.0,
        )
    })
    .context("create inert authorization manager")?;
    let mut initial = Context(null_mut());
    security::check(unsafe {
        AuthzInitializeContextFromToken(
            0,
            original.0,
            manager.0,
            null(),
            Default::default(),
            null(),
            &mut initial.0,
        )
    })
    .context("copy fixture token into authorization context")?;
    let mut restricted = Context(null_mut());
    security::check(unsafe {
        AuthzAddSidsToContext(initial.0, null(), 0, &restriction, 1, &mut restricted.0)
    })
    .context("add inert restricted authorization identity")?;
    let request = AUTHZ_ACCESS_REQUEST {
        DesiredAccess: desired,
        ..Default::default()
    };
    let (mut granted, mut error) = (0, 0);
    let mut reply = AUTHZ_ACCESS_REPLY {
        ResultListLength: 1,
        GrantedAccessMask: &mut granted,
        Error: &mut error,
        ..Default::default()
    };
    security::check(unsafe {
        AuthzAccessCheck(
            0,
            restricted.0,
            &request,
            null_mut(),
            descriptor.attributes().lpSecurityDescriptor,
            null(),
            0,
            &mut reply,
            null_mut(),
        )
    })
    .context("check inert object authorization")?;
    Ok(error == 0 && granted == desired)
}
