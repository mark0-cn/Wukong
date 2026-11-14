//! Shared Windows descriptor ownership and exact ACL checks. No role policy.
use anyhow::{Result, ensure};
use std::{
    ffi::c_void,
    mem::size_of,
    ptr::{addr_of, null_mut},
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, Authorization::ConvertSidToStringSidW,
        CONTAINER_INHERIT_ACE, GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
        GetSecurityDescriptorOwner, INHERIT_ONLY_ACE, OBJECT_INHERIT_ACE, PSECURITY_DESCRIPTOR,
        PSID, SE_DACL_PROTECTED,
    },
};

pub(crate) struct Local(pub(crate) *mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

pub fn check(value: i32) -> Result<()> {
    if value == 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}
pub(crate) fn sid_text(sid: PSID) -> Result<String> {
    ensure!(!sid.is_null(), "native identity has no SID");
    let mut text = null_mut();
    check(unsafe { ConvertSidToStringSidW(sid, &mut text) })?;
    let _memory = Local(text.cast());
    let mut length = 0;
    while length < 256 && unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    ensure!(length < 256, "unbounded native SID");
    Ok(String::from_utf16(unsafe {
        std::slice::from_raw_parts(text, length)
    })?)
}
/// Exact owner/principal/mask checks shared by files and the owned SCM object.
/// Service objects have no directory inheritance; file callers require the
/// protected-DACL bit in addition to rejecting inherited/conditional ACEs.
pub(crate) fn verify_descriptor(
    descriptor: PSECURITY_DESCRIPTOR,
    owner: &str,
    grants: &std::collections::BTreeMap<String, u32>,
    protected: bool,
) -> Result<()> {
    verify_descriptor_with_children(descriptor, owner, grants, protected, &Default::default())
}

/// Inherit-only grants are separate from rights effective on this object.
/// Ordinary file/service policies pass no such entries and continue rejecting
/// them. Resource roots declare their exact descendant-only deletion grants.
pub(crate) fn verify_descriptor_with_children(
    descriptor: PSECURITY_DESCRIPTOR,
    owner: &str,
    grants: &std::collections::BTreeMap<String, u32>,
    protected: bool,
    children: &std::collections::BTreeMap<String, u32>,
) -> Result<()> {
    let mut actual_owner = null_mut();
    let mut defaulted = 0;
    check(unsafe { GetSecurityDescriptorOwner(descriptor, &mut actual_owner, &mut defaulted) })?;
    ensure!(sid_text(actual_owner)? == owner, "unexpected native owner");
    let mut acl = null_mut();
    let mut present = 0;
    check(unsafe {
        GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted)
    })?;
    ensure!(
        present != 0 && !acl.is_null(),
        "native private object has a null ACL"
    );
    let (mut control, mut revision) = (0, 0);
    check(unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) })?;
    ensure!(
        (!protected || control & SE_DACL_PROTECTED != 0)
            && unsafe { (*acl).AceCount as usize } == grants.len() + children.len(),
        "native ACL is unprotected or has unexpected principals"
    );
    let mut actual = std::collections::BTreeMap::new();
    let mut actual_children = std::collections::BTreeMap::new();
    for index in 0..unsafe { (*acl).AceCount } {
        let mut ace = null_mut();
        check(unsafe { GetAce(acl, index as u32, &mut ace) })?;
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        ensure!(
            header.AceType == 0 && header.AceSize as usize >= size_of::<ACCESS_ALLOWED_ACE>(),
            "invalid native ACL entry"
        );
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        let sid = sid_text(addr_of!(allowed.SidStart).cast_mut().cast())?;
        if header.AceFlags & INHERIT_ONLY_ACE as u8 != 0 {
            ensure!(
                header.AceFlags
                    == (INHERIT_ONLY_ACE | OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) as u8,
                "invalid native inherit-only ACL flags"
            );
            ensure!(
                actual_children.insert(sid, allowed.Mask).is_none(),
                "duplicate native descendant ACL principal"
            );
        } else {
            ensure!(
                header.AceFlags & 0x18 == 0,
                "invalid native ACL inheritance"
            );
            ensure!(
                actual.insert(sid, allowed.Mask).is_none(),
                "duplicate native ACL principal"
            );
        }
    }
    ensure!(
        &actual == grants && &actual_children == children,
        "native ACL permissions differ from policy"
    );
    Ok(())
}
