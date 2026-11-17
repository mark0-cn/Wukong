//! Installer-owned nonadministrative local identities. Passwords remain in LSA;
//! neither controller enrollments nor runtime RPCs can read or choose them.
use crate::{
    isolation::security::{self, Handle},
    rpc::pipe_transport::service_name,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    mem::size_of,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    NetworkManagement::NetManagement::{
        LOCALGROUP_MEMBERS_INFO_0, NetApiBufferFree, NetLocalGroupAddMembers, NetUserAdd,
        NetUserGetInfo, UF_DONT_EXPIRE_PASSWD, UF_NORMAL_ACCOUNT, UF_PASSWD_CANT_CHANGE, UF_SCRIPT,
        USER_INFO_1, USER_INFO_4, USER_PRIV_USER,
    },
    Security::{
        Authentication::Identity::*,
        Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
        LOGON32_LOGON_BATCH, LOGON32_PROVIDER_DEFAULT, LogonUserW,
    },
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Management,
    Worker,
    Debugger,
    Target,
}
impl Role {
    fn prefix(self) -> &'static str {
        match self {
            Self::Management => "wkgm",
            Self::Worker => "wkgw",
            Self::Debugger => "wkgd",
            Self::Target => "wkgt",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub role: Role,
    pub name: String,
    pub sid: String,
}
impl Account {
    pub fn validate(&self, installation: &str) -> Result<()> {
        service_name(installation)?;
        ensure!(
            self.name == format!("{}{}", self.role.prefix(), &installation[..16]),
            "account is not owned by this installation"
        );
        security::validate_account_sid(&self.sid)?;
        ensure!(
            lookup(&self.name)? == self.sid,
            "installation account SID changed"
        );
        Ok(())
    }
    fn secret_name(&self, installation: &str) -> Result<Vec<u16>> {
        self.validate(installation)?;
        security::wide(OsStr::new(&format!(
            "WukongWorker:{installation}:{}",
            self.role.prefix()
        )))
    }
    pub(crate) fn logon(&self, installation: &str) -> Result<Handle> {
        security::require_system()?;
        ensure!(
            self.role != Role::Management,
            "management credentials never launch worker processes"
        );
        let policy = Policy::open(POLICY_GET_PRIVATE_INFORMATION as u32)?;
        let mut name = self.secret_name(installation)?;
        let name = lsa_string(&mut name)?;
        let mut value = null_mut();
        // SAFETY: policy.0 is the live LsaOpenPolicy handle owned by Policy; name is a valid
        // LSA_UNICODE_STRING over a live buffer, and value is only a writable out pointer.
        status(unsafe { LsaRetrievePrivateData(policy.0, &name, &mut value) })?;
        ensure!(!value.is_null(), "missing worker logon secret");
        struct Secret(*mut LSA_UNICODE_STRING);
        impl Drop for Secret {
            fn drop(&mut self) {
                // SAFETY: Secret owns the non-null LSA_UNICODE_STRING from LsaRetrievePrivateData.
                // The buffer is zeroized only when present: a malformed API response can report a
                // null Buffer or a zero Length, and from_raw_parts_mut requires a non-null pointer.
                // LsaFreeMemory frees the allocation exactly once in either case.
                unsafe {
                    let value = &mut *self.0;
                    if !value.Buffer.is_null() && value.Length > 0 {
                        std::slice::from_raw_parts_mut(value.Buffer, value.Length as usize / 2)
                            .zeroize();
                    }
                    LsaFreeMemory(self.0.cast());
                }
            }
        }
        let secret = Secret(value);
        // SAFETY: secret.0 is non-null, checked immediately after LsaRetrievePrivateData, and
        // Secret keeps that LSA allocation alive for this shared borrow.
        let value = unsafe { &*secret.0 };
        ensure!(
            value.Length > 0 && value.Length <= 256 && value.Length % 2 == 0,
            "invalid worker logon secret"
        );
        let mut password = Zeroizing::new(
            // SAFETY: Length is checked in 1..=256 and even above, so Buffer holds that many UTF-16
            // units in the LSA allocation Secret still owns; to_vec copies before that free.
            unsafe { std::slice::from_raw_parts(value.Buffer, value.Length as usize / 2) }.to_vec(),
        );
        password.push(0);
        let name = security::wide(OsStr::new(&self.name))?;
        let domain = security::wide(OsStr::new("."))?;
        let mut token = null_mut();
        // SAFETY: name, domain and password are NUL-terminated wide buffers alive for the call;
        // token is a writable out pointer and the returned token handle is checked and owned.
        security::check(unsafe {
            LogonUserW(
                name.as_ptr(),
                domain.as_ptr(),
                password.as_ptr(),
                LOGON32_LOGON_BATCH,
                LOGON32_PROVIDER_DEFAULT,
                &mut token,
            )
        })?;
        let token = Handle(token);
        ensure!(
            security::token_user_sid(&token)? == self.sid,
            "worker logon identity changed"
        );
        security::require_plain_account_token(&token)?;
        Ok(token)
    }
}

fn status(value: i32) -> Result<()> {
    ensure!(
        value == 0,
        "Windows account policy operation failed: {}",
        // SAFETY: value is the NTSTATUS from the LSA call under check; this only maps the code.
        unsafe { LsaNtStatusToWinError(value) }
    );
    Ok(())
}
struct Policy(LSA_HANDLE);
impl Policy {
    fn open(access: u32) -> Result<Self> {
        let attributes = LSA_OBJECT_ATTRIBUTES {
            Length: size_of::<LSA_OBJECT_ATTRIBUTES>() as u32,
            ..Default::default()
        };
        let mut handle: LSA_HANDLE = 0;
        // SAFETY: attributes is a fully initialized LSA_OBJECT_ATTRIBUTES with Length set, and
        // handle is a writable out pointer that receives the checked, Policy-owned handle.
        status(unsafe { LsaOpenPolicy(null(), &attributes, access, &mut handle) })?;
        Ok(Self(handle))
    }
}
impl Drop for Policy {
    fn drop(&mut self) {
        // SAFETY: Policy exists only around a successful LsaOpenPolicy handle, which this closes
        // exactly once; no other owner frees it.
        unsafe {
            LsaClose(self.0);
        }
    }
}
fn lsa_string(value: &mut [u16]) -> Result<LSA_UNICODE_STRING> {
    ensure!(
        value.last() == Some(&0) && value.len() < u16::MAX as usize / 2,
        "invalid LSA string"
    );
    Ok(LSA_UNICODE_STRING {
        Length: ((value.len() - 1) * 2) as u16,
        MaximumLength: (value.len() * 2) as u16,
        Buffer: value.as_mut_ptr(),
    })
}
pub fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    ensure!(N <= u32::MAX as usize, "random request too large");
    // SAFETY: bytes is a live [u8; N] and N is checked to fit u32, so the pointer is valid
    // for the requested byte count; the system-preferred RNG needs no algorithm handle.
    status(unsafe {
        BCryptGenRandom(
            null_mut(),
            bytes.as_mut_ptr(),
            N as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    })?;
    Ok(bytes)
}
fn lookup(name: &str) -> Result<String> {
    let name = security::wide(OsStr::new(name))?;
    let mut buffer = null_mut();
    // SAFETY: name is a NUL-terminated wide name alive for the call; buffer is a writable
    // out pointer, and level 4 makes NetUserGetInfo allocate the USER_INFO_4 it returns.
    let result = unsafe { NetUserGetInfo(null(), name.as_ptr(), 4, &mut buffer) };
    ensure!(result == 0, "cannot inspect installation account: {result}");
    struct Buffer(*mut u8);
    impl Drop for Buffer {
        fn drop(&mut self) {
            // SAFETY: Buffer owns the pointer NetUserGetInfo allocated at level 4 and releases it
            // exactly once with the matching NetApiBufferFree.
            unsafe {
                NetApiBufferFree(self.0.cast());
            }
        }
    }
    let buffer = Buffer(buffer);
    ensure!(!buffer.0.is_null(), "missing local account record");
    // SAFETY: the ensure! above rejects a null buffer, and a successful level-4
    // NetUserGetInfo returned a USER_INFO_4 that Buffer keeps alive for this borrow.
    let user = unsafe { &*buffer.0.cast::<USER_INFO_4>() };
    ensure!(
        user.usri4_priv == USER_PRIV_USER,
        "installation account must remain an ordinary user (reported privilege {})",
        user.usri4_priv
    );
    security::sid_text(user.usri4_user_sid)
}

/// Called only by the elevated VM installer, never by host development tests.
/// Existing names fail; the transaction journal must record ownership before
/// recovering a partial creation. No existing user is adopted or reset.
pub fn create(installation: &str, role: Role) -> Result<Account> {
    service_name(installation)?;
    let name = format!("{}{}", role.prefix(), &installation[..16]);
    let encoded_random =
        Zeroizing::new(hex::encode(Zeroizing::new(random_bytes::<32>()?).as_ref()));
    let text_password = Zeroizing::new(format!("Wk!7{}", encoded_random.as_str()));
    let mut password = Zeroizing::new(text_password.encode_utf16().collect::<Vec<_>>());
    password.push(0);
    let mut encoded_name = security::wide(OsStr::new(&name))?;
    let mut comment = security::wide(OsStr::new(&format!("Wukong owned {installation} {role:?}")))?;
    let user = USER_INFO_1 {
        usri1_name: encoded_name.as_mut_ptr(),
        usri1_password: password.as_mut_ptr(),
        usri1_priv: USER_PRIV_USER,
        usri1_comment: comment.as_mut_ptr(),
        usri1_flags: UF_NORMAL_ACCOUNT | UF_SCRIPT | UF_DONT_EXPIRE_PASSWD | UF_PASSWD_CANT_CHANGE,
        ..Default::default()
    };
    let mut parameter = 0;
    // SAFETY: user is a fully initialized USER_INFO_1 whose name, password and comment
    // pointers reference live locals; level 1 matches that layout and parameter is writable.
    let result = unsafe {
        NetUserAdd(
            null(),
            1,
            (&user as *const USER_INFO_1).cast(),
            &mut parameter,
        )
    };
    ensure!(
        result == 0,
        "cannot create installation-owned account: {result} (field {parameter}); no existing account will be modified"
    );
    add_to_builtin_users(&name)?;
    let account = Account {
        role,
        name: name.clone(),
        sid: lookup(&name)?,
    };
    configure_rights(&account, installation, Some(&mut password))?;
    Ok(account)
}
fn configure_rights(
    account: &Account,
    installation: &str,
    password: Option<&mut [u16]>,
) -> Result<()> {
    let role = account.role;
    // Deny interactive/RDP logon for every identity. Only broker-owned accounts
    // receive batch logon; SSH management never receives a worker logon path.
    let policy =
        Policy::open((POLICY_CREATE_ACCOUNT | POLICY_LOOKUP_NAMES | POLICY_CREATE_SECRET) as u32)?;
    let mut sid = null_mut();
    let encoded_sid = security::wide(OsStr::new(&account.sid))?;
    // SAFETY: encoded_sid is a NUL-terminated wide SID alive for the call; sid is a writable
    // out pointer that receives a LocalAlloc SID owned by the Sid guard below.
    security::check(unsafe {
        windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW(
            encoded_sid.as_ptr(),
            &mut sid,
        )
    })?;
    struct Sid(windows_sys::Win32::Security::PSID);
    impl Drop for Sid {
        fn drop(&mut self) {
            // SAFETY: this Sid owns the LocalAlloc SID from ConvertStringSidToSidW and releases it
            // exactly once with the matching LocalFree.
            unsafe {
                windows_sys::Win32::Foundation::LocalFree(self.0);
            }
        }
    }
    let sid = Sid(sid);
    let mut rights = vec![
        "SeDenyInteractiveLogonRight",
        "SeDenyRemoteInteractiveLogonRight",
    ];
    if role != Role::Management {
        rights.push("SeBatchLogonRight");
    }
    for right in rights {
        let mut encoded = security::wide(OsStr::new(right))?;
        // SAFETY: policy.0 is the live policy handle, sid.0 the valid LocalAlloc SID, and the
        // LSA_UNICODE_STRING temporary borrows the live, NUL-terminated encoded buffer.
        status(unsafe { LsaAddAccountRights(policy.0, sid.0, &lsa_string(&mut encoded)?, 1) })?;
    }
    if role != Role::Management {
        let password = password
            .ok_or_else(|| anyhow::anyhow!("worker account creation requires an owned password"))?;
        let mut secret_name = account.secret_name(installation)?;
        // SAFETY: both LSA_UNICODE_STRINGs borrow live NUL-terminated buffers (secret_name and
        // the caller-owned password) for the call, and policy.0 has POLICY_CREATE_SECRET access.
        status(unsafe {
            LsaStorePrivateData(
                policy.0,
                &lsa_string(&mut secret_name)?,
                &lsa_string(password)?,
            )
        })?;
    }
    Ok(())
}

fn add_to_builtin_users(name: &str) -> Result<()> {
    use windows_sys::Win32::Security::{Authorization::ConvertStringSidToSidW, LookupAccountSidW};
    let text = security::wide(OsStr::new("S-1-5-32-545"))?;
    let mut sid = null_mut();
    // SAFETY: text is the fixed NUL-terminated "S-1-5-32-545" wide string alive for the call;
    // sid is a writable out pointer whose LocalAlloc result the Sid guard owns.
    security::check(unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut sid) })?;
    struct Sid(windows_sys::Win32::Security::PSID);
    impl Drop for Sid {
        fn drop(&mut self) {
            // SAFETY: this Sid owns the LocalAlloc SID returned by ConvertStringSidToSidW above and
            // frees it exactly once with the matching LocalFree.
            unsafe {
                windows_sys::Win32::Foundation::LocalFree(self.0);
            }
        }
    }
    let sid = Sid(sid);
    let (mut name_length, mut domain_length, mut use_kind) = (0, 0, 0);
    // SAFETY: sid.0 is the valid LocalAlloc SID owned by Sid; null buffers with zero lengths
    // are the documented size query, which writes only to the three live out parameters.
    unsafe {
        LookupAccountSidW(
            null(),
            sid.0,
            null_mut(),
            &mut name_length,
            null_mut(),
            &mut domain_length,
            &mut use_kind,
        );
    }
    ensure!(
        name_length > 0 && name_length <= 256 && domain_length <= 256,
        "invalid builtin Users alias"
    );
    let mut group = vec![0u16; name_length as usize];
    let mut domain = vec![0u16; domain_length as usize];
    // SAFETY: group and domain are sized by the preceding query and valid for
    // name_length/domain_length; sid.0 is still live, and the lengths are writable in/out.
    security::check(unsafe {
        LookupAccountSidW(
            null(),
            sid.0,
            group.as_mut_ptr(),
            &mut name_length,
            domain.as_mut_ptr(),
            &mut domain_length,
            &mut use_kind,
        )
    })?;
    let account = security::wide(OsStr::new(name))?;
    let mut record = null_mut();
    // SAFETY: account is the fixed NUL-terminated wide name alive through the call; record
    // is a writable out pointer, and level 4 makes NetUserGetInfo allocate USER_INFO_4.
    let status = unsafe { NetUserGetInfo(null(), account.as_ptr(), 4, &mut record) };
    ensure!(
        status == 0 && !record.is_null(),
        "cannot resolve created account SID for group membership: {status}"
    );
    struct Buffer(*mut u8);
    impl Drop for Buffer {
        fn drop(&mut self) {
            // SAFETY: Buffer owns the non-null USER_INFO_4 allocation returned by NetUserGetInfo at
            // level 4 and frees it exactly once with NetApiBufferFree.
            unsafe {
                NetApiBufferFree(self.0.cast());
            }
        }
    }
    let record = Buffer(record);
    // SAFETY: record is the non-null USER_INFO_4 allocation checked above and kept alive by
    // Buffer; only its SID pointer is copied out, and record outlives its later use.
    let sid = unsafe { (*record.0.cast::<USER_INFO_4>()).usri4_user_sid };
    security::validate_account_sid(&security::sid_text(sid)?)?;
    let member = LOCALGROUP_MEMBERS_INFO_0 { lgrmi0_sid: sid };
    // SAFETY: group is the live wide builtin-group name, member is a fully initialized level-0
    // record holding the live SID from record, and the count is exactly one.
    let result = unsafe {
        NetLocalGroupAddMembers(
            null(),
            group.as_ptr(),
            0,
            (&member as *const LOCALGROUP_MEMBERS_INFO_0).cast(),
            1,
        )
    };
    ensure!(
        result == 0 || result == 1378,
        "cannot add owned account to the localized builtin Users group: {result}"
    );
    Ok(())
}

/// Only the explicit, signed early-transaction recovery path may call this.
/// The protected intent and exact ownership comment establish the failed create;
/// this never adopts an arbitrary existing name or resets its password.
pub(crate) fn recover_management(installation: &str) -> Result<Account> {
    service_name(installation)?;
    let name = format!("{}{}", Role::Management.prefix(), &installation[..16]);
    let encoded = security::wide(OsStr::new(&name))?;
    let mut pointer = null_mut();
    // SAFETY: encoded is the NUL-terminated wide account name alive for the call; pointer
    // is a writable out pointer, and level 4 allocates a USER_INFO_4 owned by Buffer below.
    let result = unsafe { NetUserGetInfo(null(), encoded.as_ptr(), 4, &mut pointer) };
    ensure!(
        result == 0,
        "missing interrupted management account: {result}"
    );
    struct Buffer(*mut u8);
    impl Drop for Buffer {
        fn drop(&mut self) {
            // SAFETY: Buffer owns the pointer NetUserGetInfo returned at level 4 and releases it
            // exactly once with NetApiBufferFree.
            unsafe {
                NetApiBufferFree(self.0.cast());
            }
        }
    }
    let buffer = Buffer(pointer);
    ensure!(!buffer.0.is_null(), "missing interrupted account details");
    // SAFETY: buffer.0 was checked non-null after a successful level-4 NetUserGetInfo and
    // stays allocated in Buffer, so it is a valid USER_INFO_4 for this borrow.
    let info = unsafe { &*buffer.0.cast::<USER_INFO_4>() };
    ensure!(
        info.usri4_priv <= USER_PRIV_USER && info.usri4_flags & UF_NORMAL_ACCOUNT != 0,
        "interrupted account has unexpected privilege {}",
        info.usri4_priv
    );
    ensure!(
        !info.usri4_comment.is_null(),
        "interrupted account lacks ownership evidence"
    );
    let mut length = 0;
    // SAFETY: info.usri4_comment was checked non-null and points at the NUL-terminated
    // comment in the live record; the scan stops at 256, within that allocation.
    while length < 256 && unsafe { *info.usri4_comment.add(length) } != 0 {
        length += 1;
    }
    ensure!(length < 256, "unbounded interrupted account comment");
    let comment =
        // SAFETY: length was found by that scan, so it is a valid count in the same live
        // NUL-terminated buffer; from_utf16 validates the contents.
        String::from_utf16(unsafe { std::slice::from_raw_parts(info.usri4_comment, length) })?;
    ensure!(
        comment == format!("Wukong owned {installation} Management"),
        "existing management account is not owned by this interrupted transaction"
    );
    let sid = security::sid_text(info.usri4_user_sid)?;
    security::validate_account_sid(&sid)?;
    drop(buffer);
    add_to_builtin_users(&name)?;
    let account = Account {
        role: Role::Management,
        name,
        sid,
    };
    account.validate(installation)?;
    configure_rights(&account, installation, None)?;
    Ok(account)
}
