use super::*;
use std::{
    ffi::c_void,
    mem::size_of,
    os::windows::{ffi::OsStrExt, io::FromRawHandle},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    },
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetNamedSecurityInfoW, SE_FILE_OBJECT,
        },
        Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
        DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl, GetTokenInformation,
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
        SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL,
        FILE_FLAG_OPEN_REPARSE_POINT,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};
use zeroize::Zeroize;

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
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn check(value: i32) -> Result<()> {
    if value == 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}
fn wide(path: &Path) -> Result<Vec<u16>> {
    let mut result: Vec<_> = path.as_os_str().encode_wide().collect();
    ensure!(!result.contains(&0), "invalid controller key path");
    result.push(0);
    Ok(result)
}
fn sid_text(sid: PSID) -> Result<String> {
    ensure!(!sid.is_null(), "missing key-store owner SID");
    let mut text = null_mut();
    check(unsafe { ConvertSidToStringSidW(sid, &mut text) })?;
    let _memory = Local(text.cast());
    let mut length = 0;
    while length < 256 && unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    ensure!(length < 256, "unbounded key-store owner SID");
    Ok(String::from_utf16(unsafe {
        std::slice::from_raw_parts(text, length)
    })?)
}
fn user_sid() -> Result<String> {
    let mut token = null_mut();
    check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
    let token = Handle(token);
    let mut size = 0;
    unsafe {
        GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut size);
    }
    ensure!(
        size > 0 && size < 64 * 1024,
        "invalid controller token size"
    );
    // TOKEN_USER contains pointers and requires native alignment.
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
fn descriptor(directory: bool) -> Result<Local> {
    let owner = user_sid()?;
    let flags = if directory { "OICI" } else { "" };
    let text = format!("O:{owner}D:P(A;{flags};FA;;;SY)(A;{flags};FA;;;{owner})");
    let encoded: Vec<_> = text.encode_utf16().chain(Some(0)).collect();
    let mut descriptor = null_mut();
    check(unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            encoded.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    })?;
    Ok(Local(descriptor))
}
fn attributes(descriptor: &Local) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    }
}
pub fn directory(path: &Path) -> Result<()> {
    let descriptor = descriptor(true)?;
    let attributes = attributes(&descriptor);
    let encoded = wide(path)?;
    if unsafe { CreateDirectoryW(encoded.as_ptr(), &attributes) } == 0 {
        let error = std::io::Error::last_os_error();
        ensure!(
            error.raw_os_error() == Some(ERROR_ALREADY_EXISTS as i32),
            "cannot create private controller key directory: {error}"
        );
    }
    permissions(path, true)
}
pub fn permissions(path: &Path, directory: bool) -> Result<()> {
    let metadata = files::reject_link(path)?;
    ensure!(
        metadata.is_dir() == directory && (directory || metadata.is_file()),
        "invalid controller key entry"
    );
    let encoded = wide(path)?;
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            encoded.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    ensure!(
        status == 0,
        "cannot inspect controller key permissions: {status}"
    );
    let _memory = Local(descriptor);
    let expected = user_sid()?;
    ensure!(
        sid_text(owner)? == expected && !dacl.is_null(),
        "controller signing key owner or ACL is unsafe"
    );
    let (mut control, mut revision) = (0, 0);
    check(unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) })?;
    ensure!(
        control & SE_DACL_PROTECTED != 0 && unsafe { (*dacl).AceCount } == 2,
        "controller key ACL must be protected and owner-only"
    );
    let mut principals = std::collections::BTreeSet::new();
    for index in 0..2 {
        let mut ace = null_mut();
        check(unsafe { GetAce(dacl, index, &mut ace) })?;
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        ensure!(
            header.AceType == 0 && header.AceSize as usize >= size_of::<ACCESS_ALLOWED_ACE>(),
            "invalid controller key ACL entry"
        );
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        ensure!(
            allowed.Header.AceType == 0
                && allowed.Header.AceFlags & 0x18 == 0
                && allowed.Mask == FILE_ALL_ACCESS,
            "unexpected controller key ACL entry"
        );
        let sid = std::ptr::addr_of!(allowed.SidStart).cast_mut().cast();
        principals.insert(sid_text(sid)?);
    }
    ensure!(
        principals == [expected, "S-1-5-18".into()].into_iter().collect(),
        "controller key ACL grants another principal"
    );
    Ok(())
}
pub fn create(path: &Path) -> Result<fs::File> {
    let descriptor = descriptor(false)?;
    let attributes = attributes(&descriptor);
    let encoded = wide(path)?;
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
        "cannot create private controller key file: {}",
        std::io::Error::last_os_error()
    );
    Ok(unsafe { fs::File::from_raw_handle(handle) })
}
fn crypt(bytes: &[u8], decrypt: bool) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= 64 * 1024,
        "invalid controller private key length"
    );
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    let result = unsafe {
        if decrypt {
            CryptUnprotectData(
                &input,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptProtectData(
                &input,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    check(result).context("controller signing-key protection failed")?;
    let _memory = Local(output.pbData.cast());
    ensure!(
        !output.pbData.is_null() && output.cbData > 0 && output.cbData <= 64 * 1024,
        "invalid protected controller key result"
    );
    let temporary =
        unsafe { std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize) };
    let result = Zeroizing::new(temporary.to_vec());
    temporary.zeroize();
    Ok(result)
}
pub fn protect(bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    crypt(bytes, false)
}
pub fn unprotect(bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    crypt(bytes, true)
}
