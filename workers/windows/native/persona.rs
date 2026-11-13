//! Owned per-resource AppContainer identities. Real profile operations are
//! System-only and run in the single-threaded resource host, never the worker.
use crate::{
    accounts::{Account, Role},
    security::{self, Handle},
};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsStr,
    mem::size_of,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{ERROR_NO_TOKEN, ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE},
    Security::{
        AdjustTokenPrivileges, FreeSid, ImpersonateLoggedOnUser,
        Isolation::{CreateAppContainerProfile, DeleteAppContainerProfile},
        LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, PSID, RevertToSelf, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_QUERY,
    },
    System::Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken},
    UI::Shell::{LoadUserProfileW, PROFILEINFOW, UnloadUserProfile},
};

#[repr(C)]
#[derive(Default)]
struct Privileges {
    count: u32,
    values: [LUID_AND_ATTRIBUTES; 2],
}
struct ProfilePrivileges {
    token: Handle,
    previous: Privileges,
    changed: bool,
}
impl ProfilePrivileges {
    fn enable() -> Result<Self> {
        security::require_system()?;
        let mut token = null_mut();
        security::check(unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES,
                &mut token,
            )
        })?;
        let mut result = Self {
            token: Handle(token),
            previous: Privileges::default(),
            changed: false,
        };
        let mut requested = Privileges {
            count: 2,
            ..Default::default()
        };
        for (entry, name) in requested
            .values
            .iter_mut()
            .zip(["SeBackupPrivilege", "SeRestorePrivilege"])
        {
            let name = security::wide(OsStr::new(name))?;
            security::check(unsafe {
                LookupPrivilegeValueW(null(), name.as_ptr(), &mut entry.Luid)
            })?;
            entry.Attributes = SE_PRIVILEGE_ENABLED;
        }
        let mut bytes = 0;
        unsafe {
            windows_sys::Win32::Foundation::SetLastError(0);
        }
        let adjusted = unsafe {
            AdjustTokenPrivileges(
                result.token.0,
                0,
                (&requested as *const Privileges).cast(),
                size_of::<Privileges>() as u32,
                (&mut result.previous as *mut Privileges).cast(),
                &mut bytes,
            )
        };
        let error = unsafe { GetLastError() };
        result.changed = adjusted != 0;
        security::check(adjusted)?;
        ensure!(
            error != ERROR_NOT_ALL_ASSIGNED,
            "System profile privileges are unavailable"
        );
        Ok(result)
    }
}
impl Drop for ProfilePrivileges {
    fn drop(&mut self) {
        if self.changed
            && unsafe {
                AdjustTokenPrivileges(
                    self.token.0,
                    0,
                    (&self.previous as *const Privileges).cast(),
                    0,
                    null_mut(),
                    null_mut(),
                )
            } == 0
        {
            std::process::abort();
        }
    }
}
/// Synchronous OS setup only. Always revert before callers inspect handles or await.
pub(crate) fn as_user<T>(token: &Handle, action: impl FnOnce() -> Result<T>) -> Result<T> {
    security::require_system()?;
    in_user_context(token, action)
}
fn in_user_context<T>(token: &Handle, action: impl FnOnce() -> Result<T>) -> Result<T> {
    // RevertToSelf restores the process identity, not a previous thread token.
    // Refuse nested scopes rather than silently destroying caller impersonation.
    let mut previous = null_mut();
    if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut previous) } != 0 {
        let _previous = Handle(previous);
        anyhow::bail!("owned user context refuses existing thread impersonation");
    }
    let error = std::io::Error::last_os_error();
    ensure!(
        error.raw_os_error() == Some(ERROR_NO_TOKEN as i32),
        "cannot inspect existing thread impersonation: {error}"
    );
    security::check(unsafe { ImpersonateLoggedOnUser(token.0) })?;
    struct Revert;
    impl Drop for Revert {
        fn drop(&mut self) {
            if unsafe { RevertToSelf() } == 0 {
                std::process::abort();
            }
        }
    }
    let guard = Revert;
    let result = action();
    drop(guard);
    result
}

pub(crate) struct Persona {
    token: Handle,
    logon: String,
    profile: HANDLE,
    name: Vec<u16>,
    sid: PSID,
    app_created: bool,
}
impl Persona {
    pub fn create(installation: &str, account: &Account, name: &str) -> Result<Self> {
        let mut result = Self::load(installation, account, name)?;
        let hr = as_user(&result.token, || {
            Ok(unsafe {
                CreateAppContainerProfile(
                    result.name.as_ptr(),
                    result.name.as_ptr(),
                    result.name.as_ptr(),
                    null(),
                    0,
                    &mut result.sid,
                )
            })
        })?;
        ensure!(
            hr >= 0,
            "create owned AppContainer profile failed: 0x{:08x}",
            hr as u32
        );
        result.app_created = true;
        security::validate_app_sid(&result.sid_text()?)?;
        Ok(result)
    }
    /// Delete only a name authorized by a private installation/resource intent.
    /// The SDK also succeeds if that per-user AppContainer never existed.
    pub(crate) fn recover(installation: &str, account: &Account, name: &str) -> Result<()> {
        let mut result = Self::load(installation, account, name)?;
        result.app_created = true;
        result.close()
    }
    pub(crate) fn cleanup_privileges() -> Result<impl Drop> {
        ProfilePrivileges::enable()
    }
    fn load(installation: &str, account: &Account, name: &str) -> Result<Self> {
        security::require_system()?;
        ensure!(
            matches!(account.role, Role::Target | Role::Debugger),
            "only native target/debugger roles own resource personas"
        );
        ensure!(
            name.len() == 55
                && name.starts_with("Wukong-")
                && name[7..]
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid owned resource profile name"
        );
        let name = security::wide(OsStr::new(name))?;
        let token = account.logon(installation)?;
        let logon = security::token_logon_sid(&token)?;
        let username = security::wide(OsStr::new(&account.name))?;
        let mut info = PROFILEINFOW {
            dwSize: size_of::<PROFILEINFOW>() as u32,
            dwFlags: 1,
            lpUserName: username.as_ptr().cast_mut(),
            ..Default::default()
        };
        {
            let _privileges = ProfilePrivileges::enable()?;
            security::check(unsafe { LoadUserProfileW(token.0, &mut info) })
                .context("load owned resource user profile")?;
        }
        Ok(Self {
            token,
            logon,
            profile: info.hProfile,
            name,
            sid: null_mut(),
            app_created: false,
        })
    }
    pub fn token(&self) -> &Handle {
        &self.token
    }
    pub fn logon_sid(&self) -> &str {
        &self.logon
    }
    pub fn sid(&self) -> PSID {
        self.sid
    }
    pub fn sid_text(&self) -> Result<String> {
        security::sid_text(self.sid)
    }
    pub fn close(&mut self) -> Result<()> {
        security::require_system()?;
        if self.app_created {
            let hr = as_user(&self.token, || {
                Ok(unsafe { DeleteAppContainerProfile(self.name.as_ptr()) })
            })?;
            ensure!(
                hr >= 0,
                "delete owned AppContainer profile failed: 0x{:08x}",
                hr as u32
            );
            self.app_created = false;
        }
        if !self.profile.is_null() {
            let _privileges = ProfilePrivileges::enable()?;
            security::check(unsafe { UnloadUserProfile(self.token.0, self.profile) })
                .context("unload owned resource user profile")?;
            self.profile = null_mut();
        }
        Ok(())
    }
}
impl Drop for Persona {
    fn drop(&mut self) {
        let _ = self.close();
        if !self.sid.is_null() {
            unsafe {
                FreeSid(self.sid);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::{
        Foundation::SetLastError,
        Security::{TOKEN_DUPLICATE, TOKEN_IMPERSONATE},
    };

    fn thread_sid() -> Result<Option<String>> {
        let mut token = null_mut();
        if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
            let error = std::io::Error::last_os_error();
            ensure!(
                error.raw_os_error() == Some(ERROR_NO_TOKEN as i32),
                "thread token query failed: {error}"
            );
            return Ok(None);
        }
        Ok(Some(security::token_user_sid(&Handle(token))?))
    }

    #[test]
    fn own_identity_context_reverts_errors_and_unwind_and_refuses_nesting() -> Result<()> {
        // Impersonate only this trusted test process's own identity on a fresh
        // thread. No account, profile, desktop or global policy is created.
        std::thread::spawn(|| -> Result<()> {
            assert_eq!(thread_sid()?, None);
            let mut token = null_mut();
            security::check(unsafe {
                OpenProcessToken(
                    GetCurrentProcess(),
                    TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE,
                    &mut token,
                )
            })?;
            let token = Handle(token);
            let expected = security::current_user_sid()?;
            ensure!(
                security::token_user_sid(&token)? == expected,
                "fixture token identity changed"
            );
            in_user_context(&token, || {
                assert_eq!(thread_sid()?, Some(expected.clone()));
                assert!(in_user_context(&token, || Ok(())).is_err());
                assert_eq!(thread_sid()?, Some(expected.clone()));
                Ok(())
            })?;
            assert_eq!(thread_sid()?, None);
            let error = in_user_context(&token, || {
                unsafe {
                    SetLastError(2);
                }
                security::check(0)
            })
            .unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .and_then(std::io::Error::raw_os_error),
                Some(2)
            );
            assert_eq!(thread_sid()?, None);
            assert!(
                std::panic::catch_unwind(|| {
                    let _: Result<()> =
                        in_user_context(&token, || panic!("trusted fixture unwind"));
                })
                .is_err()
            );
            assert_eq!(thread_sid()?, None);
            Ok(())
        })
        .join()
        .expect("identity fixture thread panicked")
    }
}
