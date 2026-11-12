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
    Foundation::{ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE},
    Security::{
        AdjustTokenPrivileges, FreeSid, ImpersonateLoggedOnUser,
        Isolation::{CreateAppContainerProfile, DeleteAppContainerProfile},
        LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, PSID, RevertToSelf, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_QUERY,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
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
fn as_user<T>(token: &Handle, action: impl FnOnce() -> Result<T>) -> Result<T> {
    security::require_system()?;
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
        let mut result = Self {
            token,
            logon,
            profile: info.hProfile,
            name,
            sid: null_mut(),
            app_created: false,
        };
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
