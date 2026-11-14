//! Initial process/thread object authority, independently of account defaults.
//! A process can alter its own DACL through its pseudo-handle; this is initial
//! admission policy, not a claim of immutable or adversarial session isolation.
use crate::{
    persona::Persona,
    security::{self, SecurityDescriptor},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    mem::size_of,
    ptr::{addr_of, null_mut},
};
use windows_sys::Win32::{
    Foundation::HANDLE,
    Security::{
        ACE_HEADER,
        Authorization::{SE_KERNEL_OBJECT, SetSecurityInfo},
        GetAce, GetSecurityDescriptorSacl, LABEL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        SECURITY_ATTRIBUTES, SYSTEM_MANDATORY_LABEL_ACE,
    },
    System::Threading::PROCESS_ALL_ACCESS,
};

pub(crate) struct Policy {
    descriptor: SecurityDescriptor,
    grants: BTreeMap<String, u32>,
}

/// Windows assigns process/thread labels during creation. Apply this explicit
/// label through the held suspended-child handle, then read it back, while the
/// existing owner/DACL policy remains unchanged.
pub(crate) fn seal_low_integrity(handle: HANDLE) -> Result<()> {
    let descriptor = SecurityDescriptor::from_sddl("S:(ML;;NW;;;LW)")?;
    let (mut sacl, mut present, mut defaulted) = (null_mut(), 0, 0);
    security::check(unsafe {
        GetSecurityDescriptorSacl(
            descriptor.attributes().lpSecurityDescriptor,
            &mut present,
            &mut sacl,
            &mut defaulted,
        )
    })?;
    ensure!(
        present != 0 && !sacl.is_null(),
        "missing low-integrity label"
    );
    let status = unsafe {
        SetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            LABEL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            sacl,
        )
    };
    ensure!(
        status == 0,
        "cannot seal owned suspended object label: {status}"
    );
    verify_integrity_label(
        SecurityDescriptor::from_object(handle, SE_KERNEL_OBJECT)?
            .attributes()
            .lpSecurityDescriptor,
    )
}
impl Policy {
    pub fn owned(persona: &Persona, debugger: Option<&Persona>) -> Result<Self> {
        let own = persona.sid_text()?;
        let peer = debugger
            .map(|peer| -> Result<_> { Ok((peer.sid_text()?, peer.logon_sid())) })
            .transpose()?;
        Self::new(
            &own,
            persona.logon_sid(),
            peer.as_ref().map(|(app, logon)| (app.as_str(), *logon)),
        )
    }
    fn new(app: &str, logon: &str, peer: Option<(&str, &str)>) -> Result<Self> {
        security::validate_app_sid(app)?;
        security::validate_logon_sid(logon)?;
        let mut grants = BTreeMap::from([
            ("S-1-5-18".into(), PROCESS_ALL_ACCESS),
            (app.into(), PROCESS_ALL_ACCESS),
            (logon.into(), PROCESS_ALL_ACCESS),
        ]);
        if let Some((peer_app, peer_logon)) = peer {
            security::validate_app_sid(peer_app)?;
            security::validate_logon_sid(peer_logon)?;
            ensure!(
                app != peer_app && logon != peer_logon,
                "debugger and target must have independent resource identities"
            );
            grants.insert(peer_app.into(), PROCESS_ALL_ACCESS);
            grants.insert(peer_logon.into(), PROCESS_ALL_ACCESS);
        }
        // DebugActiveProcess needs full target process access. Windows 10+
        // process/thread all-access masks are equal; the unit gate checks this.
        let entries = grants
            .iter()
            .map(|(sid, mask)| format!("(A;;0x{mask:08x};;;{sid})"))
            .collect::<String>();
        let descriptor =
            SecurityDescriptor::from_sddl(&format!("O:SYG:SYD:P{entries}S:(ML;;NW;;;LW)"))?;
        let result = Self { descriptor, grants };
        result.verify_descriptor(result.attributes().lpSecurityDescriptor)?;
        Ok(result)
    }
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        self.descriptor.attributes()
    }
    pub fn verify(&self, handle: HANDLE) -> Result<()> {
        self.verify_descriptor(
            SecurityDescriptor::from_object(handle, SE_KERNEL_OBJECT)?
                .attributes()
                .lpSecurityDescriptor,
        )
    }
    pub fn seal(&self, handle: HANDLE) -> Result<()> {
        security::verify_descriptor(
            SecurityDescriptor::from_object(handle, SE_KERNEL_OBJECT)?
                .attributes()
                .lpSecurityDescriptor,
            "S-1-5-18",
            &self.grants,
            false,
        )?;
        seal_low_integrity(handle)?;
        self.verify(handle)
    }
    fn verify_descriptor(&self, descriptor: PSECURITY_DESCRIPTOR) -> Result<()> {
        security::verify_descriptor(descriptor, "S-1-5-18", &self.grants, false)?;
        verify_integrity_label(descriptor)
    }
}

pub(crate) fn verify_integrity_label(descriptor: PSECURITY_DESCRIPTOR) -> Result<()> {
    let (mut sacl, mut present, mut defaulted) = (null_mut(), 0, 0);
    security::check(unsafe {
        GetSecurityDescriptorSacl(descriptor, &mut present, &mut sacl, &mut defaulted)
    })
    .context("inspect resource object DACL/label")?;
    ensure!(
        present != 0 && !sacl.is_null() && unsafe { (*sacl).AceCount } == 1,
        "resource object integrity label is ambiguous"
    );
    let mut ace = null_mut();
    security::check(unsafe { GetAce(sacl, 0, &mut ace) })?;
    let header = unsafe { &*ace.cast::<ACE_HEADER>() };
    ensure!(
        header.AceType == 0x11
            && header.AceFlags == 0
            && header.AceSize as usize >= size_of::<SYSTEM_MANDATORY_LABEL_ACE>(),
        "resource object integrity ACE differs"
    );
    let label = unsafe { &*ace.cast::<SYSTEM_MANDATORY_LABEL_ACE>() };
    let sid = security::sid_text(addr_of!(label.SidStart).cast_mut().cast())?;
    ensure!(
        label.Mask == 1 && sid == "S-1-16-4096",
        "resource object is not low/no-write-up (mask={},sid={sid})",
        label.Mask
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::Handle;
    use windows_sys::Win32::{
        Security::TOKEN_QUERY,
        System::Threading::{GetCurrentProcess, OpenProcessToken, THREAD_ALL_ACCESS},
    };
    const APP_A: &str = "S-1-15-2-101-102-103-104-105-106-107";
    const APP_B: &str = "S-1-15-2-201-202-203-204-205-206-207";
    const APP_C: &str = "S-1-15-2-301-302-303-304-305-306-307";

    fn allowed(policy: &Policy, original: &Handle, restriction: &str) -> Result<bool> {
        security::test_authz::allows(
            &policy.descriptor,
            original,
            restriction,
            PROCESS_ALL_ACCESS,
        )
    }

    #[test]
    fn initial_resource_rights_require_the_specific_restriction_or_explicit_debugger_peer()
    -> Result<()> {
        // Real local Authz checks with restricted in-memory SID contexts only.
        // This does not create a kernel AppContainer or qualify actual isolation.
        // CreateRestrictedToken rejects package SIDs; do not counterfeit a lowbox token.
        assert_eq!(PROCESS_ALL_ACCESS, THREAD_ALL_ACCESS);
        let mut token = null_mut();
        security::check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
        let token = Handle(token);
        let logon = security::token_logon_sid(&token)?;
        let own = Policy::new(APP_A, &logon, None)
            .context("construct inert private process descriptor")?;
        assert_eq!(own.grants.len(), 3);
        assert!(allowed(&own, &token, APP_A)?);
        assert!(!allowed(&own, &token, APP_B)?);
        assert!(!allowed(&own, &token, &security::current_user_sid()?)?);
        let peer = Policy::new(APP_A, &logon, Some((APP_B, "S-1-5-5-200-300")))?;
        assert_eq!(peer.grants.len(), 5);
        assert!(allowed(&peer, &token, APP_A)?);
        assert!(allowed(&peer, &token, APP_B)?);
        assert!(!allowed(&peer, &token, APP_C)?);
        assert!(Policy::new(APP_A, &logon, Some((APP_A, "S-1-5-5-200-300"))).is_err());
        assert!(Policy::new(APP_A, &logon, Some((APP_B, &logon))).is_err());
        assert!(Policy::new("S-1-15-2-1", &logon, None).is_err());
        Ok(())
    }
    #[test]
    fn broadened_initial_descriptors_and_wrong_labels_fail_admission() -> Result<()> {
        let policy = Policy::new(APP_A, "S-1-5-5-100-200", None)?;
        let entries = policy
            .grants
            .iter()
            .map(|(sid, mask)| format!("(A;;0x{mask:08x};;;{sid})"))
            .collect::<String>();
        for sddl in [
            format!("O:SYG:SYD:P{entries}(A;;0x001fffff;;;BU)S:(ML;;NW;;;LW)"),
            format!("O:BAG:SYD:P{entries}S:(ML;;NW;;;LW)"),
            format!("O:SYG:SYD:P{entries}S:(ML;;NW;;;ME)"),
        ] {
            let descriptor = SecurityDescriptor::from_sddl(&sddl)?;
            assert!(
                policy
                    .verify_descriptor(descriptor.attributes().lpSecurityDescriptor)
                    .is_err()
            );
        }
        Ok(())
    }
}
