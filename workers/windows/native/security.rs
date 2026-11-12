use anyhow::{Context, Result, ensure};
use std::{
    ffi::c_void,
    fs,
    mem::size_of,
    os::windows::{ffi::OsStrExt, fs::MetadataExt, io::FromRawHandle},
    path::{Path, PathBuf},
    ptr::{addr_of, null_mut},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            ConvertStringSidToSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
        },
        CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl,
        GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, GetTokenInformation,
        INHERIT_ONLY_ACE, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

pub(crate) struct Handle(pub(crate) HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

pub(crate) struct SecurityDescriptor(Local);
impl SecurityDescriptor {
    pub(crate) fn from_object(
        handle: HANDLE,
        kind: windows_sys::Win32::Security::Authorization::SE_OBJECT_TYPE,
    ) -> Result<Self> {
        use windows_sys::Win32::Security::{
            Authorization::GetSecurityInfo, LABEL_SECURITY_INFORMATION,
        };
        let mut descriptor = null_mut();
        let status = unsafe {
            GetSecurityInfo(
                handle,
                kind,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor,
            )
        };
        let result = Self(Local(descriptor));
        ensure!(
            status == 0,
            "cannot inspect owned object security: {status}"
        );
        ensure!(!descriptor.is_null(), "owned object security is missing");
        Ok(result)
    }
    pub(crate) fn from_sddl(sddl: &str) -> Result<Self> {
        ensure!(
            sddl.len() <= 4096,
            "native security descriptor is too large"
        );
        let encoded = wide(std::ffi::OsStr::new(sddl))?;
        let mut descriptor = null_mut();
        check(unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                encoded.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            )
        })?;
        Ok(Self(Local(descriptor)))
    }
    pub(crate) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0.0,
            bInheritHandle: 0,
        }
    }
}
pub(crate) fn validate_account_sid(sid: &str) -> Result<()> {
    ensure!(
        sid.starts_with("S-1-5-21-") && sid.len() < 128,
        "management identity must be a local account SID"
    );
    let encoded = wide(std::ffi::OsStr::new(sid))?;
    let mut decoded = null_mut();
    check(unsafe { ConvertStringSidToSidW(encoded.as_ptr(), &mut decoded) })?;
    let _memory = Local(decoded);
    ensure!(sid_text(decoded)? == sid, "noncanonical native account SID");
    Ok(())
}

pub(crate) fn validate_app_sid(sid: &str) -> Result<()> {
    ensure!(
        sid.starts_with("S-1-15-2-") && sid.split('-').count() == 11 && sid.len() < 192,
        "resource identity must be a complete AppContainer SID"
    );
    validate_sid_text(sid)
}
pub(crate) fn validate_logon_sid(sid: &str) -> Result<()> {
    ensure!(
        sid.starts_with("S-1-5-5-") && sid.split('-').count() == 6 && sid.len() < 96,
        "resource identity must be a logon SID"
    );
    validate_sid_text(sid)
}
fn validate_sid_text(sid: &str) -> Result<()> {
    let encoded = wide(std::ffi::OsStr::new(sid))?;
    let mut decoded = null_mut();
    check(unsafe { ConvertStringSidToSidW(encoded.as_ptr(), &mut decoded) })?;
    let _memory = Local(decoded);
    ensure!(sid_text(decoded)? == sid, "noncanonical resource SID");
    Ok(())
}
pub(crate) fn token_logon_sid(token: &Handle) -> Result<String> {
    use windows_sys::Win32::{
        Security::{TOKEN_GROUPS, TokenGroups},
        System::SystemServices::SE_GROUP_LOGON_ID,
    };
    let mut size = 0;
    unsafe {
        GetTokenInformation(token.0, TokenGroups, null_mut(), 0, &mut size);
    }
    ensure!(
        size >= size_of::<TOKEN_GROUPS>() as u32 && size < 65536,
        "invalid resource logon groups"
    );
    let mut data = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    check(unsafe {
        GetTokenInformation(
            token.0,
            TokenGroups,
            data.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    })?;
    let groups = unsafe { &*data.as_ptr().cast::<TOKEN_GROUPS>() };
    ensure!(
        std::mem::offset_of!(TOKEN_GROUPS, Groups)
            + groups.GroupCount as usize
                * size_of::<windows_sys::Win32::Security::SID_AND_ATTRIBUTES>()
            <= size as usize,
        "truncated resource logon groups"
    );
    let mut found = None;
    for group in
        unsafe { std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize) }
    {
        if group.Attributes & SE_GROUP_LOGON_ID as u32 == SE_GROUP_LOGON_ID as u32 {
            ensure!(found.is_none(), "ambiguous resource logon identity");
            let sid = sid_text(group.Sid)?;
            validate_logon_sid(&sid)?;
            found = Some(sid);
        }
    }
    found.context("resource token has no logon SID")
}
#[path = "resource_directory.rs"]
mod resource_directory;
pub(crate) use resource_directory::ResourceDirectory;
pub(crate) use resource_directory::probe_root as probe_resource_root;

pub fn check(value: i32) -> Result<()> {
    if value == 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}
pub fn wide(value: &std::ffi::OsStr) -> Result<Vec<u16>> {
    let mut result: Vec<_> = value.encode_wide().collect();
    ensure!(
        !result.contains(&0),
        "embedded NUL in native path or argument"
    );
    result.push(0);
    Ok(result)
}
/// Existing-link checks for controller/service-private trees. This does not
/// replace handle-based collection from a concurrently target-writable tree.
pub fn no_links(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && !path.components().any(|c| matches!(
                c,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )),
        "native path must be absolute without relative aliases"
    );
    for current in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        match fs::symlink_metadata(current) {
            Ok(metadata) => ensure!(
                !metadata.file_type().is_symlink() && metadata.file_attributes() & 0x400 == 0,
                "native reparse/link paths are forbidden"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
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
pub fn current_user_sid() -> Result<String> {
    let mut token = null_mut();
    check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
    token_user_sid(&Handle(token))
}
pub(crate) fn token_user_sid(token: &Handle) -> Result<String> {
    let mut size = 0;
    unsafe {
        GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut size);
    }
    ensure!(size > 0 && size < 64 * 1024, "invalid native token size");
    let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    check(unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    })?;
    sid_text(unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}
pub fn require_system() -> Result<()> {
    ensure!(
        current_user_sid()? == "S-1-5-18",
        "worker service requires LocalSystem; no unprivileged fallback"
    );
    Ok(())
}
pub(crate) fn require_plain_account_token(token: &Handle) -> Result<()> {
    use windows_sys::Win32::Security::{SID_AND_ATTRIBUTES, TOKEN_GROUPS, TokenGroups};
    let user = token_user_sid(token)?;
    validate_account_sid(&user)?;
    let mut size = 0;
    unsafe {
        GetTokenInformation(token.0, TokenGroups, null_mut(), 0, &mut size);
    }
    ensure!(
        size > 0 && size < 65536,
        "invalid worker account group list"
    );
    let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    check(unsafe {
        GetTokenInformation(
            token.0,
            TokenGroups,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    })?;
    let groups = unsafe { &*buffer.as_ptr().cast::<TOKEN_GROUPS>() };
    ensure!(
        std::mem::offset_of!(TOKEN_GROUPS, Groups)
            + groups.GroupCount as usize * size_of::<SID_AND_ATTRIBUTES>()
            <= size as usize,
        "truncated worker account groups"
    );
    for group in
        unsafe { std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize) }
    {
        let sid = sid_text(group.Sid)?;
        ensure!(
            !sid.starts_with("S-1-5-32-") || sid == "S-1-5-32-545",
            "worker account belongs to a privileged or unexpected builtin group"
        );
    }
    Ok(())
}

/// Every granted principal is explicit. Native VM metadata uses System/admins;
/// inert development tests can use an owner-only policy inside task temp roots.
#[derive(Clone)]
pub struct AclPolicy {
    owner: String,
    grants: std::collections::BTreeMap<String, u32>,
}
impl AclPolicy {
    pub fn private_vm() -> Self {
        Self {
            owner: "S-1-5-32-544".into(),
            grants: [
                ("S-1-5-18".into(), FILE_ALL_ACCESS),
                ("S-1-5-32-544".into(), FILE_ALL_ACCESS),
            ]
            .into_iter()
            .collect(),
        }
    }
    pub fn code_vm() -> Self {
        let mut result = Self::private_vm();
        result.grants.insert(
            "S-1-5-32-545".into(),
            FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        );
        result.grants.insert(
            "S-1-15-2-1".into(),
            FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        );
        result
    }
    pub fn with_account(mut self, sid: &str, writable: bool) -> Result<Self> {
        validate_account_sid(sid)?;
        self.grants.insert(
            sid.into(),
            if writable {
                FILE_ALL_ACCESS
            } else {
                FILE_GENERIC_READ | FILE_GENERIC_EXECUTE
            },
        );
        Ok(self)
    }
    pub fn current_user() -> Result<Self> {
        let owner = current_user_sid()?;
        Ok(Self {
            owner: owner.clone(),
            grants: [
                (owner, FILE_ALL_ACCESS),
                ("S-1-5-18".into(), FILE_ALL_ACCESS),
            ]
            .into_iter()
            .collect(),
        })
    }
    fn sddl(&self, directory: bool) -> String {
        let flags = if directory { "OICI" } else { "" };
        let mut sddl = format!("O:{}D:P", self.owner);
        for (sid, mask) in &self.grants {
            sddl.push_str(&format!("(A;{flags};0x{mask:08x};;;{sid})"));
        }
        sddl
    }
    fn descriptor(&self, directory: bool) -> Result<Local> {
        Ok(SecurityDescriptor::from_sddl(&self.sddl(directory))?.0)
    }
    pub fn create_directory(&self, path: &Path) -> Result<()> {
        no_links(path)?;
        ensure!(
            path.parent().is_some_and(Path::is_dir),
            "native directory parent must exist"
        );
        let descriptor = self.descriptor(true)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        let encoded = wide(absolute_new_path(path)?.as_os_str())?;
        check(unsafe { CreateDirectoryW(encoded.as_ptr(), &attributes) })?;
        self.verify(path, true)
    }
    pub fn create_file(&self, path: &Path) -> Result<fs::File> {
        no_links(path)?;
        let descriptor = self.descriptor(false)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        let encoded = wide(absolute_new_path(path)?.as_os_str())?;
        let handle = unsafe {
            CreateFileW(
                encoded.as_ptr(),
                GENERIC_WRITE,
                0,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        ensure!(
            handle != INVALID_HANDLE_VALUE,
            "native secure file creation failed: {}",
            std::io::Error::last_os_error()
        );
        Ok(unsafe { fs::File::from_raw_handle(handle) })
    }
    /// Installer/service-private objects only, never target-writable trees.
    pub fn apply(&self, path: &Path, directory: bool) -> Result<()> {
        no_links(path)?;
        let descriptor = self.descriptor(directory)?;
        let encoded = wide(path.canonicalize()?.as_os_str())?;
        check(unsafe {
            windows_sys::Win32::Security::SetFileSecurityW(
                encoded.as_ptr(),
                OWNER_SECURITY_INFORMATION
                    | DACL_SECURITY_INFORMATION
                    | windows_sys::Win32::Security::PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor.0,
            )
        })?;
        self.verify(path, directory)
    }
    pub fn verify(&self, path: &Path, directory: bool) -> Result<()> {
        no_links(path)?;
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            metadata.is_dir() == directory && (directory || metadata.is_file()),
            "invalid native protected entry type"
        );
        let encoded = wide(path.canonicalize()?.as_os_str())?;
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        let mut owner: PSID = null_mut();
        let mut acl: *mut ACL = null_mut();
        let status = unsafe {
            GetNamedSecurityInfoW(
                encoded.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut acl,
                null_mut(),
                &mut descriptor,
            )
        };
        ensure!(status == 0, "cannot inspect native permissions: {status}");
        let _memory = Local(descriptor);
        verify_descriptor(descriptor, &self.owner, &self.grants, true)
    }
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

#[cfg(test)]
#[path = "security_authz_tests.rs"]
pub(crate) mod test_authz;

pub fn absolute_new_path(path: &Path) -> Result<PathBuf> {
    no_links(path)?;
    let parent = path.parent().context("native path has no parent")?;
    let parent = parent.canonicalize()?;
    Ok(parent.join(path.file_name().context("native path has no filename")?))
}

/// Normal Win32 spelling for SCM/firewall identities, after checking the actual
/// local file. Do not emit mixed separators or a verbatim prefix to these APIs.
pub(crate) fn local_file_path(path: &Path) -> Result<PathBuf> {
    no_links(path)?;
    ensure!(path.is_file(), "native installation file is missing");
    let canonical = path.canonicalize()?;
    ensure!(
        matches!(canonical.components().next(), Some(std::path::Component::Prefix(prefix))
            if matches!(prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_))),
        "native installation file must use a local drive"
    );
    let text = canonical
        .to_str()
        .context("native installation path must be Unicode")?;
    Ok(PathBuf::from(
        text.strip_prefix(r"\\?\")
            .unwrap_or(text)
            .replace('/', "\\"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use windows_sys::Win32::{
        Security::{
            AccessCheck, CreateRestrictedToken, DuplicateToken, GENERIC_MAPPING,
            SID_AND_ATTRIBUTES, SecurityImpersonation, TOKEN_DUPLICATE,
        },
        Storage::FileSystem::{FILE_GENERIC_WRITE, FILE_READ_DATA},
    };

    #[test]
    fn ordinary_read_access_cannot_reach_private_host_key_generation_policy() -> Result<()> {
        // Use a real Windows restricted token: even an elevated test process
        // must pass the extra Users-only access check. No accounts, files,
        // impersonation or machine policy are changed by this fixture.
        let encoded = wide(OsStr::new("S-1-5-32-545"))?;
        let mut users = null_mut();
        check(unsafe { ConvertStringSidToSidW(encoded.as_ptr(), &mut users) })?;
        let _users = Local(users);
        let restriction = SID_AND_ATTRIBUTES {
            Sid: users,
            Attributes: 0,
        };
        let mut original = null_mut();
        check(unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE,
                &mut original,
            )
        })?;
        let original = Handle(original);
        let mut restricted = null_mut();
        check(unsafe {
            CreateRestrictedToken(
                original.0,
                0,
                0,
                null_mut(),
                0,
                null_mut(),
                1,
                &restriction,
                &mut restricted,
            )
        })?;
        let restricted = Handle(restricted);
        let mut impersonation = null_mut();
        check(unsafe { DuplicateToken(restricted.0, SecurityImpersonation, &mut impersonation) })?;
        let impersonation = Handle(impersonation);
        let mapping = GENERIC_MAPPING {
            GenericRead: FILE_GENERIC_READ,
            GenericWrite: FILE_GENERIC_WRITE,
            GenericExecute: FILE_GENERIC_EXECUTE,
            GenericAll: FILE_ALL_ACCESS,
        };
        for directory in [true, false] {
            for (policy, expected) in [
                (AclPolicy::private_vm(), false),
                (AclPolicy::code_vm(), true),
            ] {
                // NTFS fills a missing primary group at object creation.
                // AccessCheck requires it in the in-memory descriptor too.
                let descriptor =
                    SecurityDescriptor::from_sddl(&format!("G:BU{}", policy.sddl(directory)))?;
                let mut privileges = [0u64; 1024];
                let mut bytes = std::mem::size_of_val(&privileges) as u32;
                let (mut granted, mut allowed) = (0, 0);
                check(unsafe {
                    AccessCheck(
                        descriptor.0.0,
                        impersonation.0,
                        FILE_READ_DATA,
                        &mapping,
                        privileges.as_mut_ptr().cast(),
                        &mut bytes,
                        &mut granted,
                        &mut allowed,
                    )
                })?;
                assert_eq!(allowed != 0, expected);
            }
        }
        Ok(())
    }
}
