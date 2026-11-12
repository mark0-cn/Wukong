//! Hold the directory's authority before granting a sandbox access. The handle
//! is noninheritable and denies write/delete sharing for its entire lifetime.
use super::*;
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use windows_sys::Win32::{
    Security::{
        Authorization::{GetSecurityInfo, SetSecurityInfo},
        GetSecurityDescriptorSacl, LABEL_SECURITY_INFORMATION, SYSTEM_MANDATORY_LABEL_ACE,
    },
    Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_WRITE, FILE_SHARE_READ, READ_CONTROL, WRITE_DAC,
        WRITE_OWNER,
    },
};

pub(crate) struct ResourceDirectory {
    path: PathBuf,
    handle: fs::File,
    original: AclPolicy,
}
impl ResourceDirectory {
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
            .access_mode(READ_CONTROL | WRITE_DAC | WRITE_OWNER)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)?;
        let original = policy.clone();
        policy.grants.insert(
            logon.into(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE,
        );
        policy.grants.insert(
            app.into(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE,
        );
        let descriptor =
            SecurityDescriptor::from_sddl(&format!("{}S:(ML;OICI;NW;;;LW)", policy.sddl(true)))?;
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
        };
        result.verify(&policy)?;
        Ok(result)
    }
    fn verify(&self, policy: &AclPolicy) -> Result<()> {
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
        verify_descriptor(actual.0, &policy.owner, &policy.grants, true)?;
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
        let descriptor = self.original.descriptor(true)?;
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
        verify_descriptor(actual.0, &self.original.owner, &self.original.grants, true)
    }
}
impl Drop for ResourceDirectory {
    fn drop(&mut self) {
        let _ = self.reclaim();
    }
}
fn security_system() -> Result<()> {
    super::require_system()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn held_resource_directory_has_exact_principals_and_low_label_without_profile_creation()
    -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut token = null_mut();
        check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
        let token = Handle(token);
        let logon = token_logon_sid(&token)?;
        let name = wide(std::ffi::OsStr::new("Wukong-inert-directory-test"))?;
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
        let app = sid_text(app.0)?;
        let directory = ResourceDirectory::with_policy(
            &root.path().join("scratch"),
            &app,
            &logon,
            AclPolicy::current_user()?,
        )?;
        assert!(directory.path().is_dir());
        assert!(validate_app_sid("S-1-15-2-1").is_err());
        assert!(validate_logon_sid("S-1-5-18").is_err());
        // Another directory write/delete handle cannot mutate the held root.
        assert!(
            fs::OpenOptions::new()
                .access_mode(windows_sys::Win32::Storage::FileSystem::FILE_WRITE_DATA)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(directory.path())
                .is_err()
        );
        Ok(())
    }
}
