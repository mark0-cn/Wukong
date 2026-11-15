//! Hold the directory's authority before granting a sandbox access. The handle
//! is noninheritable and denies delete sharing for its entire lifetime.
//! Directory data-write sharing is not an immutability boundary. An undeletable
//! private guard keeps the root nonempty, preventing a junction on that root.
use super::MetadataExt;

use super::{GetAce, GetSecurityDescriptorDacl, Local, Result, addr_of, fs, sid_text};

use super::{
    ACE_HEADER, AclPolicy, Context, DACL_SECURITY_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, OWNER_SECURITY_INFORMATION, Path, PathBuf,
    SE_FILE_OBJECT, SecurityDescriptor, check, ensure, no_links, null_mut, validate_app_sid,
    validate_logon_sid, verify_descriptor, verify_descriptor_with_children,
};

use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use windows_sys::Win32::{
    Security::{
        Authorization::{GetSecurityInfo, SetSecurityInfo},
        GetSecurityDescriptorSacl, LABEL_SECURITY_INFORMATION, SYSTEM_MANDATORY_LABEL_ACE,
    },
    Storage::FileSystem::{
        DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_WRITE, FILE_LIST_DIRECTORY,
        FILE_SHARE_READ, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
    },
};

pub(crate) struct ResourceDirectory {
    path: PathBuf,
    handle: fs::File,
    original: AclPolicy,
    guard: std::sync::Mutex<Option<fs::File>>,
}
const ROOT_GUARD: &str = ".wukong-root-guard";
impl ResourceDirectory {
    /// Reclaim a root from its protected scratch intent, after its named Job
    /// has independently been drained. Never accept an RPC-supplied OS path.
    pub(crate) fn recover(path: &Path, app: &str, logon: &str) -> Result<()> {
        security_system()?;
        validate_app_sid(app)?;
        validate_logon_sid(logon)?;
        no_links(path).context("verify recovery scratch ancestors")?;
        let handle = fs::OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY | READ_CONTROL | WRITE_DAC | WRITE_OWNER)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .context("hold original recovery scratch root")?;
        ensure!(
            handle.metadata()?.is_dir() && handle.metadata()?.file_attributes() & 0x400 == 0,
            "recovery scratch root is redirected"
        );
        let original = AclPolicy::private_vm();
        let mut sandbox = original.clone();
        for sid in [app, logon] {
            sandbox.grants.insert(
                sid.into(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE,
            );
        }
        let children = [(app.into(), DELETE), (logon.into(), DELETE)].into();
        let descriptor = SecurityDescriptor::from_object(handle.as_raw_handle(), SE_FILE_OBJECT)
            .context("read recovery scratch root policy")?;
        let raw = descriptor.attributes().lpSecurityDescriptor;
        ensure!(
            verify_descriptor(raw, &original.owner, &original.grants, true).is_ok()
                || verify_descriptor_with_children(
                    raw,
                    &original.owner,
                    &sandbox.grants,
                    true,
                    &children
                )
                .is_ok(),
            "recovery scratch root policy differs from its protected intent"
        );
        let guard_path = path.join(ROOT_GUARD);
        let guard = if guard_path.try_exists()? {
            original
                .verify(&guard_path, false)
                .context("verify original recovery scratch guard")?;
            Some(
                fs::OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                    .open(&guard_path)
                    .context("hold original recovery scratch guard")?,
            )
        } else {
            None
        };
        let directory = Self {
            path: path.into(),
            handle,
            original,
            guard: std::sync::Mutex::new(guard),
        };
        directory
            .reclaim()
            .context("reclaim original scratch root")?;
        drop(directory);
        crate::isolation::resource_cleanup::remove_reclaimed(path)
            .context("remove reclaimed scratch tree")
    }
    pub fn create(path: &Path, app: &str, logon: &str) -> Result<Self> {
        security_system()?;
        Self::with_policy(path, app, logon, AclPolicy::private_vm())
    }
    fn with_policy(path: &Path, app: &str, logon: &str, mut policy: AclPolicy) -> Result<Self> {
        validate_app_sid(app)?;
        validate_logon_sid(logon)?;
        policy.create_directory(path)?;
        let path = path.canonicalize()?;
        let handle = fs::OpenOptions::new()
            // A security-metadata-only handle does not establish data/delete
            // sharing. Directory list access makes the held share reservation real.
            .access_mode(FILE_LIST_DIRECTORY | READ_CONTROL | WRITE_DAC | WRITE_OWNER)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)?;
        let original = policy.clone();
        // Create before granting sandbox access. The protected file ACL and its
        // unshared handle deny deletion independently of directory write sharing.
        let guard = policy.create_file(&path.join(ROOT_GUARD))?;
        policy.grants.insert(
            logon.into(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE,
        );
        policy.grants.insert(
            app.into(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE,
        );
        // DELETE applies only to descendants. Neither the root nor its private,
        // protected guard gains DELETE/DELETE_CHILD through these entries.
        let children: std::collections::BTreeMap<String, u32> =
            [(app.into(), DELETE), (logon.into(), DELETE)]
                .into_iter()
                .collect();
        let mut sddl = policy.sddl(true);
        for (sid, mask) in &children {
            sddl.push_str(&format!("(A;OICIIO;0x{mask:08x};;;{sid})"));
        }
        let descriptor = SecurityDescriptor::from_sddl(&format!("{sddl}S:(ML;OICI;NW;;;LW)"))?;
        let (mut dacl, mut sacl) = (null_mut(), null_mut());
        let (mut present, mut defaulted) = (0, 0);
        check(unsafe {
            GetSecurityDescriptorDacl(descriptor.0.0, &mut present, &mut dacl, &mut defaulted)
        })?;
        check(unsafe {
            GetSecurityDescriptorSacl(descriptor.0.0, &mut present, &mut sacl, &mut defaulted)
        })?;
        let status = unsafe {
            SetSecurityInfo(
                handle.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION
                    | LABEL_SECURITY_INFORMATION
                    | windows_sys::Win32::Security::PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                sacl,
            )
        };
        ensure!(
            status == 0,
            "cannot install sandbox directory policy: {status}"
        );
        let result = Self {
            path,
            handle,
            original,
            guard: std::sync::Mutex::new(Some(guard)),
        };
        result.verify(&policy, &children)?;
        Ok(result)
    }
    fn verify(
        &self,
        policy: &AclPolicy,
        children: &std::collections::BTreeMap<String, u32>,
    ) -> Result<()> {
        let mut actual = null_mut();
        let status = unsafe {
            GetSecurityInfo(
                self.handle.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut actual,
            )
        };
        ensure!(
            status == 0,
            "cannot inspect held sandbox directory policy: {status}"
        );
        let actual = Local(actual);
        verify_descriptor_with_children(actual.0, &policy.owner, &policy.grants, true, children)?;
        let (mut sacl, mut present, mut defaulted) = (null_mut(), 0, 0);
        check(unsafe {
            GetSecurityDescriptorSacl(actual.0, &mut present, &mut sacl, &mut defaulted)
        })?;
        ensure!(
            present != 0 && !sacl.is_null() && unsafe { (*sacl).AceCount } == 1,
            "ambiguous sandbox directory integrity label"
        );
        let mut ace = null_mut();
        check(unsafe { GetAce(sacl, 0, &mut ace) })?;
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        ensure!(
            header.AceType == 0x11
                && header.AceSize as usize >= size_of::<SYSTEM_MANDATORY_LABEL_ACE>(),
            "invalid sandbox integrity ACE"
        );
        let label = unsafe { &*ace.cast::<SYSTEM_MANDATORY_LABEL_ACE>() };
        ensure!(
            label.Mask == 1
                && sid_text(addr_of!(label.SidStart).cast_mut().cast())? == "S-1-16-4096",
            "sandbox label is not low/no-write-up"
        );
        Ok(())
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Call only after the owned Job is empty. Reclaim the root through the
    /// original handle, so cleanup never depends on a sandbox-controlled ACL.
    pub fn reclaim(&self) -> Result<()> {
        // Reclaim this object only. Never propagate ACLs through an untrusted
        // child tree; the handle-based cleaner seals each non-reparse object.
        let descriptor = self.original.descriptor(false)?;
        let (mut present, mut defaulted) = (0, 0);
        let mut dacl = null_mut();
        check(unsafe {
            GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted)
        })?;
        let status = unsafe {
            SetSecurityInfo(
                self.handle.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION
                    | windows_sys::Win32::Security::PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                null_mut(),
            )
        };
        ensure!(status == 0, "cannot reclaim sandbox directory: {status}");
        let mut actual = null_mut();
        let status = unsafe {
            GetSecurityInfo(
                self.handle.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut actual,
            )
        };
        ensure!(status == 0, "cannot inspect reclaimed directory: {status}");
        let actual = Local(actual);
        verify_descriptor(actual.0, &self.original.owner, &self.original.grants, true)?;
        ensure!(
            self.handle.metadata()?.file_attributes() & 0x400 == 0,
            "held sandbox root became a reparse point"
        );
        let mut guard = self
            .guard
            .lock()
            .map_err(|_| anyhow::anyhow!("resource guard lock poisoned"))?;
        if let Some(file) = guard.take() {
            drop(file);
            fs::remove_file(self.path.join(ROOT_GUARD))?;
        }
        Ok(())
    }
}
// Dropping handles alone must not remove the private guard or change the DACL:
// an unsuccessful Job drain cannot prove the sandbox stopped. Only an explicit
// reclaim after quiescence removes it; failed resources remain for repair.
fn security_system() -> Result<()> {
    super::require_system()
}

/// Fixed lowbox probe, with no caller-selected destination. The well-formed
/// mount-point request points only at this same task-owned directory.
pub(crate) fn probe_root(path: &Path) -> Result<()> {
    use windows_sys::Win32::{
        Storage::FileSystem::{DELETE, FILE_SHARE_DELETE, FILE_SHARE_WRITE, FILE_WRITE_DATA},
        System::IO::DeviceIoControl,
    };
    let options = || {
        let mut options = fs::OpenOptions::new();
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        options
    };
    ensure!(
        matches!(options().access_mode(DELETE).open(path.join(ROOT_GUARD)),
        Err(error) if matches!(error.raw_os_error(), Some(5 | 32))),
        "sandbox root guard deletion was not denied"
    );
    match options().access_mode(FILE_WRITE_DATA).open(path) {
        Err(error) if matches!(error.raw_os_error(), Some(5 | 32)) => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(directory) => {
            let text = path.to_string_lossy();
            let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
            let substitute: Vec<u16> = format!(r"\??\{text}").encode_utf16().collect();
            ensure!(
                substitute.len() < 16000,
                "root probe path exceeds reparse limit"
            );
            let length = (substitute.len() * 2) as u16;
            let mut buffer = Vec::new();
            buffer.extend(0xa0000003u32.to_le_bytes());
            buffer.extend((length + 12).to_le_bytes());
            buffer.extend(0u16.to_le_bytes());
            for value in [0, length, length + 2, 0] {
                buffer.extend(value.to_le_bytes());
            }
            for value in substitute {
                buffer.extend(value.to_le_bytes());
            }
            buffer.extend([0u8; 4]);
            let mut returned = 0;
            let result = unsafe {
                DeviceIoControl(
                    directory.as_raw_handle(),
                    0x900a4,
                    buffer.as_ptr().cast(),
                    buffer.len() as u32,
                    null_mut(),
                    0,
                    &mut returned,
                    null_mut(),
                )
            };
            let error = std::io::Error::last_os_error();
            ensure!(
                result == 0 && matches!(error.raw_os_error(), Some(5 | 32 | 145 | 1314)),
                "sandbox root reparse mutation was not denied: {error}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Handle, test_authz, token_logon_sid, wide};
    use super::*;
    use windows_sys::Win32::{
        Security::{PSID, TOKEN_QUERY},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    fn inert_app_sid(name: &str) -> Result<String> {
        let name = wide(std::ffi::OsStr::new(name))?;
        let mut app = null_mut();
        let hr = unsafe {
            windows_sys::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName(
                name.as_ptr(),
                &mut app,
            )
        };
        ensure!(hr >= 0, "cannot derive an inert SID");
        struct Free(PSID);
        impl Drop for Free {
            fn drop(&mut self) {
                unsafe {
                    windows_sys::Win32::Security::FreeSid(self.0);
                }
            }
        }
        let app = Free(app);
        sid_text(app.0)
    }
    #[test]
    #[ignore = "dedicated Windows VM: actual AppContainer filesystem ACL inheritance"]
    fn inherited_delete_permits_owned_children_but_not_root_guard_or_other_resources() -> Result<()>
    {
        use windows_sys::Win32::Storage::FileSystem::FILE_DELETE_CHILD;
        let app = inert_app_sid("Wukong-inert-deletion-test")?;
        const OTHER: &str = "S-1-15-2-201-202-203-204-205-206-207";
        let temporary = tempfile::tempdir()?;
        let mut original = null_mut();
        check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut original) })?;
        let token = Handle(original);
        let logon = token_logon_sid(&token)?;
        let descriptor = |path: &Path| -> Result<SecurityDescriptor> {
            let file = fs::OpenOptions::new()
                .access_mode(READ_CONTROL)
                .share_mode(FILE_SHARE_READ)
                .open(path)?;
            SecurityDescriptor::from_object(file.as_raw_handle(), SE_FILE_OBJECT)
        };
        // The former read/write/execute grant permits data I/O but cannot
        // delete an inherited file. Test actual NTFS inheritance through Authz.
        let mut legacy = AclPolicy::current_user()?;
        for sid in [app.as_str(), logon.as_str()] {
            legacy.grants.insert(
                sid.into(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE,
            );
        }
        let old_root = temporary.path().join("legacy");
        AclPolicy::current_user()?.create_directory(&old_root)?;
        legacy.apply(&old_root, true)?;
        let old_child = old_root.join("probe.bin");
        fs::write(&old_child, b"old")?;
        let old = descriptor(&old_child)?;
        assert!(test_authz::allows(&old, &token, &app, FILE_GENERIC_READ)?);
        assert!(!test_authz::allows(&old, &token, &app, DELETE)?);

        let directory = ResourceDirectory::with_policy(
            &temporary.path().join("resource"),
            &app,
            &logon,
            AclPolicy::current_user()?,
        )?;
        let child = directory.path().join("probe.bin");
        fs::write(&child, b"owned")?;
        let child_descriptor = descriptor(&child)?;
        assert!(test_authz::allows(&child_descriptor, &token, &app, DELETE)?);
        assert!(!test_authz::allows(
            &child_descriptor,
            &token,
            OTHER,
            DELETE
        )?);
        let root =
            SecurityDescriptor::from_object(directory.handle.as_raw_handle(), SE_FILE_OBJECT)?;
        assert!(!test_authz::allows(&root, &token, &app, DELETE)?);
        assert!(!test_authz::allows(&root, &token, &app, FILE_DELETE_CHILD)?);
        let guard = {
            let held = directory.guard.lock().unwrap();
            SecurityDescriptor::from_object(held.as_ref().unwrap().as_raw_handle(), SE_FILE_OBJECT)?
        };
        assert!(!test_authz::allows(&guard, &token, &app, DELETE)?);
        probe_root(directory.path())?;
        directory.reclaim()?;
        assert!(!test_authz::allows(
            &descriptor(&child)?,
            &token,
            &app,
            DELETE
        )?);
        fs::remove_file(child)?;
        let path = directory.path().to_owned();
        drop(directory);
        fs::remove_dir(path)?;
        Ok(())
    }

    #[test]
    fn root_verification_requires_exact_inherit_only_delete_entries() -> Result<()> {
        let mut policy = AclPolicy::private_vm();
        let app = "S-1-15-2-101-102-103-104-105-106-107";
        let logon = "S-1-5-5-200-300";
        for sid in [app, logon] {
            policy.grants.insert(
                sid.into(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE,
            );
        }
        let children = [(app.into(), DELETE), (logon.into(), DELETE)]
            .into_iter()
            .collect();
        let entries = format!("(A;OICIIO;0x00010000;;;{app})(A;OICIIO;0x00010000;;;{logon})");
        let sddl = format!("{}{entries}", policy.sddl(true));
        let descriptor = SecurityDescriptor::from_sddl(&sddl)?;
        verify_descriptor_with_children(
            descriptor.0.0,
            &policy.owner,
            &policy.grants,
            true,
            &children,
        )?;
        assert!(verify_descriptor(descriptor.0.0, &policy.owner, &policy.grants, true).is_err());
        for changed in [
            sddl.replace("OICIIO", "OICI"),
            sddl.replace("OICIIO", "OIIO"),
            sddl.replace("0x00010000", "0x00000040"),
            sddl.replace("D:P", "D:"),
            format!("{sddl}(A;OICIIO;0x00010000;;;{app})"),
        ] {
            let changed = SecurityDescriptor::from_sddl(&changed)?;
            assert!(
                verify_descriptor_with_children(
                    changed.0.0,
                    &policy.owner,
                    &policy.grants,
                    true,
                    &children
                )
                .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn shared_write_denial_qualifies_the_guarded_root_probe() -> Result<()> {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_WRITE, FILE_WRITE_DATA,
        };
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().join("plain-root");
        fs::create_dir(&root)?;
        let guard = AclPolicy::current_user()?.create_file(&root.join(ROOT_GUARD))?;
        let held = fs::OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY | READ_CONTROL | WRITE_DAC | WRITE_OWNER)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&root)?;
        let write = fs::OpenOptions::new()
            .access_mode(FILE_WRITE_DATA)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&root);
        assert!(matches!(write, Err(error) if error.raw_os_error()==Some(32)));
        probe_root(&root)?;
        drop(guard);
        drop(held);
        Ok(())
    }
    #[test]
    fn directory_list_access_establishes_the_delete_share_reservation() -> Result<()> {
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_SHARE_DELETE, FILE_SHARE_WRITE,
        };
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().join("control");
        fs::create_dir(&root)?;
        let authority = |access| {
            fs::OpenOptions::new()
                .access_mode(access)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&root)
        };
        let deletion = || {
            fs::OpenOptions::new()
                .access_mode(DELETE)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&root)
        };
        let metadata = authority(READ_CONTROL | WRITE_DAC | WRITE_OWNER)?;
        // Regression control: share flags on a metadata-only handle do not
        // prevent this open, even though FILE_SHARE_DELETE was omitted.
        drop(deletion()?);
        drop(metadata);
        let held = authority(FILE_LIST_DIRECTORY | READ_CONTROL | WRITE_DAC | WRITE_OWNER)?;
        assert!(matches!(deletion(), Err(error) if error.raw_os_error()==Some(32)));
        drop(held);
        drop(deletion()?);
        Ok(())
    }
    #[test]
    fn held_resource_directory_has_exact_principals_and_low_label_without_profile_creation()
    -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut token = null_mut();
        check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
        let token = Handle(token);
        let logon = token_logon_sid(&token)?;
        let app = inert_app_sid("Wukong-inert-directory-test")?;
        let directory = ResourceDirectory::with_policy(
            &root.path().join("scratch"),
            &app,
            &logon,
            AclPolicy::current_user()?,
        )?;
        assert!(directory.path().is_dir());
        assert!(validate_app_sid("S-1-15-2-1").is_err());
        assert!(validate_logon_sid("S-1-5-18").is_err());
        // Directory writes may be admitted on Windows. Delete sharing and the
        // nonempty, unshared guard are the distinct protections actually tested.
        assert!(
            fs::OpenOptions::new()
                .access_mode(windows_sys::Win32::Storage::FileSystem::DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(directory.path())
                .is_err()
        );
        probe_root(directory.path())?;
        directory.reclaim()?;
        let scratch = directory.path().to_owned();
        drop(directory);
        fs::remove_dir(scratch)?;
        Ok(())
    }
}
