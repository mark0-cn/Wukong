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
    // SAFETY: `text` is NUL-terminated by `security::wide`; the SID is allocated into `sid`
    // and handed to the `Sid` guard below, which frees it exactly once.
    security::check(unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut sid) })
        .context("parse inert resource restriction")?;
    struct Sid(*mut std::ffi::c_void);
    impl Drop for Sid {
        fn drop(&mut self) {
            // SAFETY: `Sid` only ever wraps a successful `ConvertStringSidToSidW` allocation
            // and is the sole owner, so this LocalFree runs exactly once.
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
                // SAFETY: the null guard proves `self.0` is a handle created by a successful
                // `AuthzInitializeResourceManager`, and `Manager` is its sole owner.
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
                // SAFETY: the null guard proves `self.0` is a live context handle from
                // `AuthzInitializeContextFromToken`/`AuthzAddSidsToContext`; `Context` owns it.
                unsafe {
                    AuthzFreeContext(self.0);
                }
            }
        }
    }
    let mut manager = Manager(null_mut());
    // SAFETY: `manager.0` receives an opaque handle owned by the `Manager` guard above; the
    // audit callback and context are all null/None, as `AUTHZ_RM_FLAG_NO_AUDIT` allows.
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
    // SAFETY: `original.0` is the fixture's live token owned by its `Handle`, `manager.0` is
    // the live resource manager above, and `initial.0` receives the new owned context.
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
    // SAFETY: `initial.0` is the live context above and `restriction` borrows the `Sid` guard's
    // SID for the call duration; `restricted.0` then receives the new owned context.
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
    // SAFETY: `restricted.0` is the live context above, `descriptor` owns the SD passed via
    // `attributes()`, and `reply` points at the two still-live locals it fills in.
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
