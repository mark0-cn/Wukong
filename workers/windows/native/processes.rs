//! Native process ownership. Every child is placed in its non-breakaway Job
//! during creation, before its first instruction, with an explicit handle list.
use crate::{
    accounts::Account,
    security::{self, Handle, SecurityDescriptor},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::File,
    mem::size_of,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::Path,
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, WAIT_OBJECT_0},
    Security::{CreateRestrictedToken, GetTokenInformation, TOKEN_PRIVILEGES, TokenPrivileges},
    System::{JobObjects::*, StationsAndDesktops::*, Threading::*},
};

pub struct Job(Handle);
impl Job {
    pub fn new() -> Result<Self> {
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        ensure!(
            !handle.is_null(),
            "cannot create native worker Job: {}",
            std::io::Error::last_os_error()
        );
        let job = Self(Handle(handle));
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        security::check(unsafe {
            SetInformationJobObject(
                job.0.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        })?;
        Ok(job)
    }
    pub fn terminate(&self) -> Result<()> {
        security::check(unsafe { TerminateJobObject(self.0.0, 1) })
    }
    pub fn active_processes(&self) -> Result<u32> {
        let mut information = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        security::check(unsafe {
            QueryInformationJobObject(
                self.0.0,
                JobObjectBasicAccountingInformation,
                (&mut information as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            )
        })?;
        Ok(information.ActiveProcesses)
    }
    /// A signaled leader is insufficient: detached descendants can outlive it.
    /// Keep the owned Job handle until Windows confirms every member exited.
    pub fn terminate_and_wait(&self, timeout: Duration) -> Result<()> {
        if self.active_processes()? == 0 {
            return Ok(());
        }
        self.terminate()?;
        let deadline = Instant::now() + timeout;
        loop {
            if self.active_processes()? == 0 {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "owned native Job did not become quiescent"
            );
            std::thread::sleep(
                Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
}

struct Desktop {
    desktop: HDESK,
    station: HWINSTA,
    name: Vec<u16>,
}
impl Drop for Desktop {
    fn drop(&mut self) {
        unsafe {
            CloseDesktop(self.desktop);
            CloseWindowStation(self.station);
        }
    }
}
impl Desktop {
    /// Creation temporarily selects a private station. Call before dispatching
    /// service work, on the single startup thread; restore it before returning.
    fn new(sid: &str) -> Result<Self> {
        security::validate_account_sid(sid)?;
        let name = format!(
            "Wukong-{}",
            hex::encode(crate::accounts::random_bytes::<16>()?)
        );
        let station_name = security::wide(OsStr::new(&name))?;
        let descriptor = SecurityDescriptor::from_sddl(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})"))?;
        let attributes = descriptor.attributes();
        let station =
            unsafe { CreateWindowStationW(station_name.as_ptr(), 0, 0x000f037f, &attributes) };
        ensure!(
            !station.is_null(),
            "cannot create private worker window station: {}",
            std::io::Error::last_os_error()
        );
        let mut result = Self {
            desktop: null_mut(),
            station,
            name: security::wide(OsStr::new(&format!("{name}\\worker")))?,
        };
        let previous = unsafe { GetProcessWindowStation() };
        security::check(unsafe { SetProcessWindowStation(station) })?;
        let desktop_name = security::wide(OsStr::new("worker"))?;
        result.desktop =
            unsafe { CreateDesktopW(desktop_name.as_ptr(), null(), null(), 0, 0x1ff, &attributes) };
        let failure = std::io::Error::last_os_error();
        if unsafe { SetProcessWindowStation(previous) } == 0 {
            std::process::abort();
        }
        ensure!(
            !result.desktop.is_null(),
            "cannot create private worker desktop: {failure}"
        );
        Ok(result)
    }
}

fn without_privileges(token: &Handle) -> Result<Handle> {
    let mut size = 0;
    unsafe {
        GetTokenInformation(token.0, TokenPrivileges, null_mut(), 0, &mut size);
    }
    ensure!(size > 0 && size < 65536, "invalid process privilege list");
    let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    security::check(unsafe {
        GetTokenInformation(
            token.0,
            TokenPrivileges,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    })?;
    let privileges = unsafe { &*buffer.as_ptr().cast::<TOKEN_PRIVILEGES>() };
    let offset = std::mem::offset_of!(TOKEN_PRIVILEGES, Privileges);
    ensure!(
        offset
            + privileges.PrivilegeCount as usize
                * size_of::<windows_sys::Win32::Security::LUID_AND_ATTRIBUTES>()
            <= size as usize,
        "truncated process privilege list"
    );
    let mut restricted = null_mut();
    // DISABLE_MAX_PRIVILEGE retains SeChangeNotifyPrivilege and ignores the
    // explicit deletion list. Flags=0 deletes every enumerated privilege.
    security::check(unsafe {
        CreateRestrictedToken(
            token.0,
            0,
            0,
            null(),
            privileges.PrivilegeCount,
            privileges.Privileges.as_ptr(),
            0,
            null(),
            &mut restricted,
        )
    })?;
    let restricted = Handle(restricted);
    let mut actual = TOKEN_PRIVILEGES::default();
    let mut returned = 0;
    security::check(unsafe {
        GetTokenInformation(
            restricted.0,
            TokenPrivileges,
            (&mut actual as *mut TOKEN_PRIVILEGES).cast(),
            size_of::<TOKEN_PRIVILEGES>() as u32,
            &mut returned,
        )
    })?;
    ensure!(
        actual.PrivilegeCount == 0,
        "worker token retains native privileges"
    );
    Ok(restricted)
}

struct Attributes {
    storage: Vec<usize>,
    initialized: bool,
}
impl Attributes {
    fn new(count: u32) -> Result<Self> {
        let mut bytes = 0;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), count, 0, &mut bytes);
        }
        ensure!(bytes > 0 && bytes < 65536, "invalid process attribute list");
        let mut result = Self {
            storage: vec![0; bytes.div_ceil(size_of::<usize>())],
            initialized: false,
        };
        security::check(unsafe {
            InitializeProcThreadAttributeList(result.pointer(), count, 0, &mut bytes)
        })?;
        result.initialized = true;
        Ok(result)
    }
    fn pointer(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
    fn set_handles(&mut self, attribute: u32, handles: &mut [HANDLE]) -> Result<()> {
        security::check(unsafe {
            UpdateProcThreadAttribute(
                self.pointer(),
                0,
                attribute as usize,
                handles.as_mut_ptr().cast(),
                std::mem::size_of_val(handles),
                null_mut(),
                null(),
            )
        })
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                DeleteProcThreadAttributeList(self.pointer());
            }
        }
    }
}
fn inheritable(file: &File) -> Result<Handle> {
    let mut output = null_mut();
    security::check(unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            file.as_raw_handle(),
            GetCurrentProcess(),
            &mut output,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    })?;
    Ok(Handle(output))
}

/// Windows command-line encoding, used only with an explicit executable path.
pub fn command_line(arguments: &[OsString]) -> Result<Vec<u16>> {
    ensure!(!arguments.is_empty(), "missing native process argv");
    let mut result = Vec::new();
    for (index, value) in arguments.iter().enumerate() {
        if index > 0 {
            result.push(b' ' as u16);
        }
        result.push(b'"' as u16);
        let mut backslashes = 0;
        for value in value.encode_wide() {
            ensure!(value != 0, "NUL in native process argument");
            if value == b'\\' as u16 {
                backslashes += 1;
                continue;
            }
            result.extend(std::iter::repeat_n(
                b'\\' as u16,
                backslashes * if value == b'"' as u16 { 2 } else { 1 },
            ));
            backslashes = 0;
            if value == b'"' as u16 {
                result.push(b'\\' as u16);
            }
            result.push(value);
        }
        result.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
        result.push(b'"' as u16);
    }
    ensure!(
        result.len() < 32767,
        "native process command line exceeds Windows limit"
    );
    result.push(0);
    Ok(result)
}
fn environment(values: &BTreeMap<String, OsString>) -> Result<Vec<u16>> {
    ensure!(!values.is_empty(), "empty native process environment");
    let mut output = Vec::new();
    for (key, value) in values {
        ensure!(
            !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "invalid native environment key"
        );
        output.extend(key.encode_utf16());
        output.push(b'=' as u16);
        let value = security::wide(value)?;
        output.extend(value);
    }
    output.push(0);
    Ok(output)
}

pub struct OwnedProcess {
    process: Handle,
    pid: u32,
    job: Job,
    _desktop: Option<Desktop>,
}
impl OwnedProcess {
    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn exited(&self) -> bool {
        unsafe { WaitForSingleObject(self.process.0, 0) == WAIT_OBJECT_0 }
    }
    pub fn exit_code(&self) -> Result<Option<u32>> {
        if !self.exited() {
            return Ok(None);
        }
        let mut code = 0;
        security::check(unsafe { GetExitCodeProcess(self.process.0, &mut code) })?;
        Ok(Some(code))
    }
    pub fn job(&self) -> &Job {
        &self.job
    }
    pub fn stop(&self, timeout: Duration) -> Result<()> {
        self.job.terminate_and_wait(timeout)
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        // Error/unwind fallback only. Successful lifecycle gates call stop()
        // explicitly and propagate failure rather than treating Drop as proof.
        let _ = self.stop(Duration::from_secs(10));
    }
}

pub struct Spawn<'a> {
    pub executable: &'a Path,
    pub arguments: &'a [OsString],
    pub directory: &'a Path,
    pub environment: &'a BTreeMap<String, OsString>,
    pub log: &'a File,
    pub input: &'a File,
}
impl Spawn<'_> {
    pub fn worker(&self, installation: &str, account: &Account) -> Result<OwnedProcess> {
        security::require_system()?;
        let token = without_privileges(&account.logon(installation)?)?;
        let desktop = Desktop::new(&account.sid)?;
        self.create(Some(token), Some(desktop))
    }
    /// Fixed trusted executables such as the packaged SSH daemon. This is never
    /// a target execution API and must not be reachable from analysis requests.
    pub(crate) fn system(&self) -> Result<OwnedProcess> {
        security::require_system()?;
        self.create(None, None)
    }
    fn create(&self, token: Option<Handle>, mut desktop: Option<Desktop>) -> Result<OwnedProcess> {
        security::no_links(self.executable)?;
        security::no_links(self.directory)?;
        ensure!(
            self.executable.is_file() && self.directory.is_dir(),
            "invalid native executable or working directory"
        );
        let app = security::wide(self.executable.as_os_str())?;
        let directory = security::wide(self.directory.as_os_str())?;
        let mut argv = vec![self.executable.as_os_str().to_owned()];
        argv.extend_from_slice(self.arguments);
        let mut command = command_line(&argv)?;
        let environment = environment(self.environment)?;
        let job = Job::new()?;
        let (log, input) = (inheritable(self.log)?, inheritable(self.input)?);
        let mut handles = [log.0, input.0];
        let mut jobs = [job.0.0];
        let mut attributes = Attributes::new(2)?;
        attributes.set_handles(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &mut handles)?;
        attributes.set_handles(PROC_THREAD_ATTRIBUTE_JOB_LIST, &mut jobs)?;
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = input.0;
        startup.StartupInfo.hStdOutput = log.0;
        startup.StartupInfo.hStdError = log.0;
        if let Some(desktop) = desktop.as_mut() {
            startup.StartupInfo.lpDesktop = desktop.name.as_mut_ptr();
        }
        startup.lpAttributeList = attributes.pointer();
        let flags = CREATE_SUSPENDED
            | CREATE_UNICODE_ENVIRONMENT
            | EXTENDED_STARTUPINFO_PRESENT
            | CREATE_NO_WINDOW;
        let mut information = PROCESS_INFORMATION::default();
        let success = unsafe {
            match token.as_ref() {
                Some(token) => CreateProcessAsUserW(
                    token.0,
                    app.as_ptr(),
                    command.as_mut_ptr(),
                    null(),
                    null(),
                    1,
                    flags,
                    environment.as_ptr().cast(),
                    directory.as_ptr(),
                    &startup.StartupInfo,
                    &mut information,
                ),
                None => CreateProcessW(
                    app.as_ptr(),
                    command.as_mut_ptr(),
                    null(),
                    null(),
                    1,
                    flags,
                    environment.as_ptr().cast(),
                    directory.as_ptr(),
                    &startup.StartupInfo,
                    &mut information,
                ),
            }
        };
        security::check(success).context("native suspended process creation failed")?;
        let process = OwnedProcess {
            process: Handle(information.hProcess),
            pid: information.dwProcessId,
            job,
            _desktop: desktop,
        };
        let thread = Handle(information.hThread);
        ensure!(
            unsafe { ResumeThread(thread.0) } != u32::MAX,
            "cannot resume owned native child"
        );
        Ok(process)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_job_fixture_entry() {
        let Some(mode) = std::env::var_os("WUKONG_NATIVE_JOB_FIXTURE") else {
            return;
        };
        if mode == "parent" || mode == "parent-exits" {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "processes::tests::native_job_fixture_entry",
                    "--nocapture",
                ])
                .env("WUKONG_NATIVE_JOB_FIXTURE", "leaf");
            let mut child = command.spawn().unwrap();
            if mode == "parent-exits" {
                std::process::exit(0);
            }
            let _ = child.wait();
        } else {
            std::fs::write(
                std::env::current_dir().unwrap().join("leaf.pid"),
                std::process::id().to_string(),
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
    }
    #[test]
    fn native_job_contains_spawned_descendants_before_execution_and_kills_them_together()
    -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let log = File::create(temporary.path().join("fixture.log"))?;
        let input = File::open("NUL")?;
        let executable = std::env::current_exe()?;
        let mut values = BTreeMap::new();
        for key in ["SystemRoot", "PATH", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                values.insert(key.into(), value);
            }
        }
        values.insert("WUKONG_NATIVE_JOB_FIXTURE".into(), "parent".into());
        let fixture = Spawn {
            executable: &executable,
            arguments: &[
                "--exact".into(),
                "processes::tests::native_job_fixture_entry".into(),
                "--nocapture".into(),
            ],
            directory: temporary.path(),
            environment: &values,
            log: &log,
            input: &input,
        }
        .create(None, None)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let leaf_path = temporary.path().join("leaf.pid");
        while !leaf_path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        ensure!(
            leaf_path.exists() && fixture.job().active_processes()? >= 2,
            "fixture child did not inherit the non-breakaway Job"
        );
        let leaf_pid: u32 = std::fs::read_to_string(leaf_path)?.parse()?;
        let leaf = Handle(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | 0x00100000, 0, leaf_pid)
        });
        ensure!(!leaf.0.is_null(), "fixture descendant cannot be inspected");
        let mut contained = 0;
        security::check(unsafe { IsProcessInJob(leaf.0, fixture.job.0.0, &mut contained) })?;
        assert_eq!(contained, 1);
        fixture.stop(Duration::from_secs(10))?;
        assert_eq!(fixture.job().active_processes()?, 0);
        assert_eq!(
            unsafe { WaitForSingleObject(fixture.process.0, 10000) },
            WAIT_OBJECT_0
        );
        assert_eq!(unsafe { WaitForSingleObject(leaf.0, 10000) }, WAIT_OBJECT_0);
        Ok(())
    }
    #[test]
    fn exited_leader_is_not_quiescent_until_its_owned_descendant_exits() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let log = File::create(temporary.path().join("fixture.log"))?;
        let input = File::open("NUL")?;
        let executable = std::env::current_exe()?;
        let mut values = BTreeMap::new();
        for key in ["SystemRoot", "PATH", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                values.insert(key.into(), value);
            }
        }
        values.insert("WUKONG_NATIVE_JOB_FIXTURE".into(), "parent-exits".into());
        let fixture = Spawn {
            executable: &executable,
            arguments: &[
                "--exact".into(),
                "processes::tests::native_job_fixture_entry".into(),
                "--nocapture".into(),
            ],
            directory: temporary.path(),
            environment: &values,
            log: &log,
            input: &input,
        }
        .create(None, None)?;
        let leaf_path = temporary.path().join("leaf.pid");
        let deadline = Instant::now() + Duration::from_secs(10);
        while (!leaf_path.exists() || !fixture.exited()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        ensure!(
            leaf_path.exists() && fixture.exited(),
            "fixture leader failed to leave a descendant"
        );
        assert_eq!(fixture.exit_code()?, Some(0));
        assert!(fixture.job().active_processes()? > 0);
        let leaf_pid: u32 = std::fs::read_to_string(leaf_path)?.parse()?;
        let leaf = Handle(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | 0x00100000, 0, leaf_pid)
        });
        ensure!(!leaf.0.is_null(), "fixture descendant cannot be inspected");
        fixture.stop(Duration::from_secs(10))?;
        assert_eq!(fixture.job().active_processes()?, 0);
        assert_eq!(unsafe { WaitForSingleObject(leaf.0, 1000) }, WAIT_OBJECT_0);
        Ok(())
    }
    #[test]
    fn native_argv_preserves_quotes_empty_arguments_and_trailing_backslashes() -> Result<()> {
        let encoded = command_line(&[
            "fixed.exe".into(),
            "".into(),
            "with space".into(),
            "a\"b".into(),
            "x\\".into(),
        ])?;
        assert_eq!(
            String::from_utf16(&encoded[..encoded.len() - 1])?,
            r#""fixed.exe" "" "with space" "a\"b" "x\\""#
        );
        assert!(command_line(&[OsString::from("invalid\0argument")]).is_err());
        Ok(())
    }
}
