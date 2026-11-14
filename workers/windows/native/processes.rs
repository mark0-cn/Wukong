//! Native process ownership. Every child is placed in its non-breakaway Job
//! during creation, before its first instruction, with an explicit handle list.
use crate::{
    accounts::Account,
    security::{self, Handle, SecurityDescriptor},
    token_policy::{same_privilege, traverse_privilege},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::File,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
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
#[path = "resource_job.rs"]
pub(crate) mod resource_job;
/// Aggregate limits for one native analysis resource, before child creation.
#[derive(Clone, Copy, Debug)]
pub struct ResourceLimits {
    processes: u32,
    memory: usize,
}
impl ResourceLimits {
    pub fn new(processes: u32, memory_bytes: u64) -> Result<Self> {
        ensure!(
            (1..=64).contains(&processes)
                && (16 * 1024 * 1024..=4 * 1024 * 1024 * 1024).contains(&memory_bytes),
            "native resource limits exceed supported bounds"
        );
        Ok(Self {
            processes,
            memory: usize::try_from(memory_bytes)?,
        })
    }
}
impl Job {
    pub fn new() -> Result<Self> {
        Self::configured(None)
    }
    fn configured(resource: Option<ResourceLimits>) -> Result<Self> {
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        ensure!(
            !handle.is_null(),
            "cannot create native worker Job: {}",
            std::io::Error::last_os_error()
        );
        Self(Handle(handle)).configure(resource)
    }
    fn configure(self, resource: Option<ResourceLimits>) -> Result<Self> {
        let job = self;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        if let Some(resource) = resource {
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_JOB_MEMORY
                | JOB_OBJECT_LIMIT_PRIORITY_CLASS;
            limits.BasicLimitInformation.ActiveProcessLimit = resource.processes;
            limits.BasicLimitInformation.PriorityClass = BELOW_NORMAL_PRIORITY_CLASS;
            limits.JobMemoryLimit = resource.memory;
        }
        security::check(unsafe {
            SetInformationJobObject(
                job.0.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        })?;
        if let Some(resource) = resource {
            let mut actual = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            security::check(unsafe {
                QueryInformationJobObject(
                    job.0.0,
                    JobObjectExtendedLimitInformation,
                    (&mut actual as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    null_mut(),
                )
            })?;
            ensure!(
                actual.BasicLimitInformation.LimitFlags == limits.BasicLimitInformation.LimitFlags
                    && actual.BasicLimitInformation.ActiveProcessLimit == resource.processes
                    && actual.BasicLimitInformation.PriorityClass == BELOW_NORMAL_PRIORITY_CLASS
                    && actual.JobMemoryLimit == resource.memory,
                "native resource Job limits differ after SDK readback"
            );
        }
        Ok(job)
    }
    pub fn terminate(&self) -> Result<()> {
        security::check(unsafe { TerminateJobObject(self.0.0, 1) })
    }
    pub fn active_processes(&self) -> Result<u32> {
        Self::active_handle(self.0.0)
    }
    fn active_handle(handle: HANDLE) -> Result<u32> {
        let mut information = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        security::check(unsafe {
            QueryInformationJobObject(
                handle,
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
        Self::drain_handle(self.0.0, timeout)
    }
    fn drain_handle(handle: HANDLE, timeout: Duration) -> Result<()> {
        if Self::active_handle(handle).context("query owned Job before termination")? == 0 {
            return Ok(());
        }
        security::check(unsafe { TerminateJobObject(handle, 1) })
            .context("terminate owned native Job")?;
        let deadline = Instant::now() + timeout;
        loop {
            if Self::active_handle(handle).context("query owned Job while draining")? == 0 {
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
        Self::create(sid, false, None)
    }
    fn sandbox(sid: &str, logon: &str) -> Result<Self> {
        security::validate_app_sid(sid)?;
        security::validate_logon_sid(logon)?;
        Self::create(sid, true, Some(logon))
    }
    fn create(sid: &str, low: bool, logon: Option<&str>) -> Result<Self> {
        let name = format!(
            "Wukong-{}",
            hex::encode(crate::accounts::random_bytes::<16>()?)
        );
        let station_name = security::wide(OsStr::new(&name))?;
        let label = if low { "S:(ML;;NW;;;LW)" } else { "" };
        let logon = logon
            .map(|sid| format!("(A;;GA;;;{sid})"))
            .unwrap_or_default();
        let descriptor = SecurityDescriptor::from_sddl(&format!(
            "D:P(A;;GA;;;SY){logon}(A;;GA;;;{sid}){label}"
        ))?;
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
        result.desktop = unsafe {
            CreateDesktopW(
                desktop_name.as_ptr(),
                null(),
                null(),
                0,
                0x000f01ff,
                &attributes,
            )
        };
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

fn seal_sandbox_token(
    process: &OwnedProcess,
    expected_sid: windows_sys::Win32::Security::PSID,
) -> Result<()> {
    use windows_sys::Win32::Security::{
        TOKEN_ADJUST_PRIVILEGES, TOKEN_APPCONTAINER_INFORMATION, TOKEN_QUERY, TokenAppContainerSid,
    };
    let mut token = null_mut();
    security::check(unsafe {
        OpenProcessToken(
            process.process.0,
            TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES,
            &mut token,
        )
    })?;
    let token = Handle(token);
    crate::token_policy::verify_network_isolation(token.0)?;
    let mut bytes = 0;
    unsafe {
        GetTokenInformation(token.0, TokenAppContainerSid, null_mut(), 0, &mut bytes);
    }
    ensure!(
        bytes as usize >= size_of::<TOKEN_APPCONTAINER_INFORMATION>() && bytes < 65536,
        "invalid created AppContainer token data"
    );
    let mut app_data = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
    security::check(unsafe {
        GetTokenInformation(
            token.0,
            TokenAppContainerSid,
            app_data.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    })?;
    let app = unsafe { &*app_data.as_ptr().cast::<TOKEN_APPCONTAINER_INFORMATION>() };
    ensure!(
        security::sid_text(app.TokenAppContainer)? == security::sid_text(expected_sid)?,
        "native target AppContainer SID changed"
    );
    seal_token_privileges(&token, true)
}

pub(crate) fn verify_sandbox_privileges(token: &Handle) -> Result<()> {
    crate::token_policy::verify_privileges(token.0)
}
fn seal_token_privileges(token: &Handle, retain_traverse: bool) -> Result<()> {
    use windows_sys::Win32::Security::{AdjustTokenPrivileges, SE_PRIVILEGE_REMOVED};
    let traverse = retain_traverse.then(traverse_privilege).transpose()?;
    let mut bytes = 0;
    unsafe {
        GetTokenInformation(token.0, TokenPrivileges, null_mut(), 0, &mut bytes);
    }
    ensure!(
        bytes > 0 && bytes < 65536,
        "invalid created token privilege list"
    );
    let mut data = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
    security::check(unsafe {
        GetTokenInformation(
            token.0,
            TokenPrivileges,
            data.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    })?;
    let count = unsafe { *data.as_ptr().cast::<u32>() };
    if count > 0 {
        ensure!(
            std::mem::offset_of!(TOKEN_PRIVILEGES, Privileges)
                + count as usize * size_of::<windows_sys::Win32::Security::LUID_AND_ATTRIBUTES>()
                <= bytes as usize,
            "truncated created token privilege list"
        );
        let privileges = unsafe { &mut *data.as_mut_ptr().cast::<TOKEN_PRIVILEGES>() };
        let entries = unsafe {
            std::slice::from_raw_parts_mut(privileges.Privileges.as_mut_ptr(), count as usize)
        };
        ensure!(
            !retain_traverse
                || entries.iter().any(|entry| traverse
                    .as_ref()
                    .is_some_and(|luid| same_privilege(&entry.Luid, luid))),
            "sandbox source lacks SeChangeNotifyPrivilege"
        );
        for entry in entries {
            entry.Attributes = if traverse
                .as_ref()
                .is_some_and(|luid| same_privilege(&entry.Luid, luid))
            {
                windows_sys::Win32::Security::SE_PRIVILEGE_ENABLED
            } else {
                SE_PRIVILEGE_REMOVED
            };
        }
        security::check(unsafe {
            AdjustTokenPrivileges(token.0, 0, privileges, 0, null_mut(), null_mut())
        })?;
    }
    if retain_traverse {
        return verify_sandbox_privileges(token);
    }
    let mut remaining = TOKEN_PRIVILEGES::default();
    security::check(unsafe {
        GetTokenInformation(
            token.0,
            TokenPrivileges,
            (&mut remaining as *mut TOKEN_PRIVILEGES).cast(),
            size_of::<TOKEN_PRIVILEGES>() as u32,
            &mut bytes,
        )
    })?;
    ensure!(
        remaining.PrivilegeCount == 0,
        "created target token retains privileges"
    );
    Ok(())
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
    initial_thread: Option<Handle>,
    pid: u32,
    job: Job,
    _desktop: Option<Desktop>,
    _directory: Option<security::ResourceDirectory>,
    launch_diagnostics: Option<serde_json::Value>,
    stop_on_drop: bool,
}
impl OwnedProcess {
    /// Release only our CREATE_SUSPENDED count, after Windows has attached a
    /// debugger to this held process. A pending debug event still freezes it.
    pub(crate) fn release_debug_start(&mut self) -> Result<bool> {
        let thread = self
            .initial_thread
            .as_ref()
            .context("native debug target has no held initial suspension")?;
        ensure!(
            !self.exited(),
            "native debug target exited before attachment"
        );
        let mut attached = 0;
        security::check(unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::CheckRemoteDebuggerPresent(
                self.process.0,
                &mut attached,
            )
        })?;
        if attached == 0 {
            return Ok(false);
        }
        let previous = unsafe { ResumeThread(thread.0) };
        // DbgEng may add its own suspension while attaching. Release only our
        // one count, leaving every debugger-owned count intact. Never retry a
        // failed release or try to drive the count to zero.
        self.initial_thread.take();
        ensure!(
            previous != 0 && previous != u32::MAX,
            "owned initial suspension is absent or could not be released: {previous}"
        );
        Ok(true)
    }
    pub(crate) fn launch_diagnostics(&self) -> Option<&serde_json::Value> {
        self.launch_diagnostics.as_ref()
    }
    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub(crate) fn profile(&self) -> Result<serde_json::Value> {
        ensure!(!self.exited(), "owned interactive target has exited");
        let result = crate::process_profile::observe(self.process.0, self.pid)?;
        ensure!(!self.exited(), "owned target exited during observation");
        Ok(result)
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
        let deadline = Instant::now() + timeout;
        self.job.terminate_and_wait(timeout)?;
        // ActiveProcesses can reach zero before process-object rundown signals
        // the held leader. Wait before reading its final exit or reclaiming.
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min((u32::MAX - 1) as u128) as u32;
        ensure!(
            unsafe { WaitForSingleObject(self.process.0, remaining) } == WAIT_OBJECT_0,
            "owned native process did not signal after Job drain"
        );
        if let Some(directory) = &self._directory {
            directory.reclaim()?;
        }
        Ok(())
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        // Error/unwind fallback only. Successful lifecycle gates call stop()
        // explicitly and propagate failure rather than treating Drop as proof.
        if self.stop_on_drop {
            let _ = self.stop(Duration::from_secs(10));
        }
    }
}

/// Only a trusted System host without a private desktop/scratch may transfer
/// control into the asynchronous service. OwnedHandle supplies Send/Sync; no
/// thread-affine desktop or raw-handle wrapper crosses that boundary.
pub(crate) struct SystemHost {
    process: OwnedHandle,
    job: OwnedHandle,
    pid: u32,
}
/// A wait-only duplicate; it cannot terminate or inspect the host's memory.
pub(crate) struct ExitSignal(OwnedHandle);
impl ExitSignal {
    pub(crate) async fn wait(&self) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let state = unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) };
                if state == WAIT_OBJECT_0 {
                    return Ok::<_, anyhow::Error>(());
                }
                ensure!(
                    state == windows_sys::Win32::Foundation::WAIT_TIMEOUT,
                    "native host wait failed: {}",
                    std::io::Error::last_os_error()
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("native host replied before its exit could be confirmed")??;
        Ok(())
    }
}
impl SystemHost {
    pub(crate) fn exit_signal(&self) -> Result<ExitSignal> {
        let mut handle = null_mut();
        security::check(unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.process.as_raw_handle(),
                GetCurrentProcess(),
                &mut handle,
                PROCESS_SYNCHRONIZE,
                0,
                0,
            )
        })?;
        Ok(ExitSignal(unsafe { OwnedHandle::from_raw_handle(handle) }))
    }
    fn take(mut source: OwnedProcess) -> Result<Self> {
        ensure!(
            source._desktop.is_none()
                && source._directory.is_none()
                && source.initial_thread.is_none(),
            "resource persona cannot transfer to the service"
        );
        fn duplicate(handle: HANDLE) -> Result<OwnedHandle> {
            let mut copy = null_mut();
            security::check(unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    handle,
                    GetCurrentProcess(),
                    &mut copy,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            })?;
            Ok(unsafe { OwnedHandle::from_raw_handle(copy) })
        }
        let result = Self {
            process: duplicate(source.process.0)?,
            job: duplicate(source.job.0.0)?,
            pid: source.pid,
        };
        // Both controls are now retained by the destination. Closing the source
        // duplicates must not terminate the Job during this ownership transfer.
        source.stop_on_drop = false;
        Ok(result)
    }
    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }
    pub(crate) fn exited(&self) -> bool {
        unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) == WAIT_OBJECT_0 }
    }
    pub(crate) fn stop(&self, timeout: Duration) -> Result<u32> {
        let deadline = Instant::now() + timeout;
        Job::drain_handle(self.job.as_raw_handle(), timeout)?;
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min((u32::MAX - 1) as u128) as u32;
        ensure!(
            unsafe { WaitForSingleObject(self.process.as_raw_handle(), remaining) }
                == WAIT_OBJECT_0,
            "native System host did not signal after Job drain"
        );
        let mut code = 0;
        security::check(unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) })?;
        Ok(code)
    }
}
impl Drop for SystemHost {
    fn drop(&mut self) {
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
#[derive(Clone, Copy)]
enum Start<'a> {
    Execute,
    AwaitDebugger {
        streams: Option<(&'a File, ResourceLimits)>,
    },
    ResourceHost {
        installation: &'a str,
        resource: &'a str,
        error: Option<&'a File>,
    },
    Streamed {
        error: &'a File,
        limits: ResourceLimits,
    },
    InspectSuspended,
}
#[derive(Clone, Copy)]
enum CreationContext {
    Process,
    Role,
    RoleProfileEnvironment,
}
#[derive(Debug)]
struct DrainFailure(anyhow::Error);
impl std::fmt::Display for DrainFailure {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{}", self.0)
    }
}
impl std::error::Error for DrainFailure {}
#[path = "process_creation_probe.rs"]
mod creation_probe;
#[path = "process_launch_probe.rs"]
mod launch_probe;
impl Spawn<'_> {
    #[cfg(test)]
    pub(crate) fn inert_fixture(&self) -> Result<OwnedProcess> {
        ensure!(
            self.executable == std::env::current_exe()?,
            "native fixture must execute this test binary"
        );
        self.create(
            None,
            None,
            None,
            None,
            Start::Execute,
            CreationContext::Process,
        )
    }
    #[cfg(test)]
    pub(crate) fn inert_streamed_fixture(
        &self,
        error: &File,
        limits: ResourceLimits,
    ) -> Result<OwnedProcess> {
        ensure!(
            self.executable == std::env::current_exe()?,
            "native fixture must execute this test binary"
        );
        self.create(
            None,
            None,
            None,
            None,
            Start::Streamed { error, limits },
            CreationContext::Process,
        )
    }
    pub fn worker(&self, installation: &str, account: &Account) -> Result<OwnedProcess> {
        security::require_system()?;
        let token = without_privileges(&account.logon(installation)?)?;
        let desktop = Desktop::new(&account.sid)?;
        self.create(
            Some(token),
            Some(desktop),
            None,
            None,
            Start::Execute,
            CreationContext::Process,
        )
    }
    /// Fixed trusted executables such as the packaged SSH daemon. This is never
    /// a target execution API and must not be reachable from analysis requests.
    pub(crate) fn system(&self) -> Result<OwnedProcess> {
        security::require_system()?;
        self.create(
            None,
            None,
            None,
            None,
            Start::Execute,
            CreationContext::Process,
        )
    }
    pub(crate) fn system_host(&self, installation: &str, resource: &str) -> Result<SystemHost> {
        self.resource_host(installation, resource, None)
    }
    pub(crate) fn system_host_streamed(
        &self,
        installation: &str,
        resource: &str,
        error: &File,
    ) -> Result<SystemHost> {
        self.resource_host(installation, resource, Some(error))
    }
    fn resource_host(
        &self,
        installation: &str,
        resource: &str,
        error: Option<&File>,
    ) -> Result<SystemHost> {
        security::require_system()?;
        SystemHost::take(self.create(
            None,
            None,
            None,
            None,
            Start::ResourceHost {
                installation,
                resource,
                error,
            },
            CreationContext::Process,
        )?)
    }
    /// A fresh trusted recovery child runs outside the Job it must terminate.
    pub(crate) fn system_control(&self) -> Result<SystemHost> {
        SystemHost::take(self.system()?)
    }
    /// Resource-host thread only: private station setup is process-wide.
    pub(crate) fn sandbox(
        &self,
        persona: &crate::persona::Persona,
        directory: security::ResourceDirectory,
    ) -> Result<OwnedProcess> {
        self.sandbox_peer(persona, directory, None, Start::Execute)
    }
    /// Native resource host only; I/O readers start after private station setup.
    pub(crate) fn sandbox_streamed(
        &self,
        persona: &crate::persona::Persona,
        directory: security::ResourceDirectory,
        error: &File,
        limits: ResourceLimits,
    ) -> Result<OwnedProcess> {
        self.sandbox_peer(persona, directory, None, Start::Streamed { error, limits })
    }
    /// Only the fixed resource host can pair its target with its independently
    /// created debugger persona. No caller-supplied SID or PID enters this path.
    pub(crate) fn sandbox_debug_target(
        &self,
        target: &crate::persona::Persona,
        directory: security::ResourceDirectory,
        debugger: &crate::persona::Persona,
    ) -> Result<OwnedProcess> {
        self.sandbox_peer(
            target,
            directory,
            Some(debugger),
            Start::AwaitDebugger { streams: None },
        )
    }
    pub(crate) fn sandbox_debug_target_streamed(
        &self,
        target: &crate::persona::Persona,
        directory: security::ResourceDirectory,
        debugger: &crate::persona::Persona,
        error: &File,
        limits: ResourceLimits,
    ) -> Result<OwnedProcess> {
        self.sandbox_peer(
            target,
            directory,
            Some(debugger),
            Start::AwaitDebugger {
                streams: Some((error, limits)),
            },
        )
    }
    fn sandbox_peer(
        &self,
        persona: &crate::persona::Persona,
        directory: security::ResourceDirectory,
        debugger: Option<&crate::persona::Persona>,
        start: Start<'_>,
    ) -> Result<OwnedProcess> {
        security::require_system()?;
        // Windows prepares the lowbox namespace/profile using this ordinary
        // role token. It remains parent-only; the actual child is suspended,
        // checked and stripped of every privilege except Windows' ordinary
        // traverse-check bypass before its first instruction.
        let mut token = null_mut();
        security::check(unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                persona.token().0,
                GetCurrentProcess(),
                &mut token,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        })?;
        let token = Handle(token);
        let desktop = Desktop::sandbox(&persona.sid_text()?, persona.logon_sid())?;
        let capabilities = windows_sys::Win32::Security::SECURITY_CAPABILITIES {
            AppContainerSid: persona.sid(),
            Capabilities: null_mut(),
            CapabilityCount: 0,
            Reserved: 0,
        };
        let descriptor = crate::process_access::Policy::owned(persona, debugger)?;
        let mut process = self.create(
            Some(token),
            Some(desktop),
            Some(capabilities),
            Some(descriptor),
            start,
            CreationContext::RoleProfileEnvironment,
        )?;
        process._directory = Some(directory);
        Ok(process)
    }
    fn create(
        &self,
        token: Option<Handle>,
        mut desktop: Option<Desktop>,
        mut capabilities: Option<windows_sys::Win32::Security::SECURITY_CAPABILITIES>,
        descriptor: Option<crate::process_access::Policy>,
        start: Start<'_>,
        creation_context: CreationContext,
    ) -> Result<OwnedProcess> {
        security::no_links(self.executable)?;
        security::no_links(self.directory)?;
        ensure!(
            self.executable.is_file() && self.directory.is_dir(),
            "invalid native executable or working directory"
        );
        let executable = security::local_file_path(self.executable)?;
        let app = security::wide(executable.as_os_str())?;
        let directory = security::wide(self.directory.as_os_str())?;
        let mut argv = vec![executable.into_os_string()];
        argv.extend_from_slice(self.arguments);
        let mut command = command_line(&argv)?;
        let profile = if matches!(creation_context, CreationContext::RoleProfileEnvironment) {
            Some(crate::environment::role_paths(token.as_ref().context(
                "profile environment requires an owned role token",
            )?)?)
        } else {
            None
        };
        let mut child_environment = self.environment.clone();
        if let Some(profile) = &profile {
            child_environment.extend(profile.clone());
        }
        let environment = environment(&child_environment)?;
        let resource_limits = match start {
            Start::Streamed { limits, .. }
            | Start::AwaitDebugger {
                streams: Some((_, limits)),
            } => Some(limits),
            _ => None,
        };
        let job = match start {
            Start::ResourceHost {
                installation,
                resource,
                ..
            } => Job::resource(installation, resource)?,
            _ => Job::configured(resource_limits)?,
        };
        let (log, input) = (inheritable(self.log)?, inheritable(self.input)?);
        let error = match start {
            Start::Streamed { error, .. }
            | Start::AwaitDebugger {
                streams: Some((error, _)),
            }
            | Start::ResourceHost {
                error: Some(error), ..
            } => Some(inheritable(error)?),
            _ => None,
        };
        let mut handles = vec![log.0, input.0];
        if let Some(error) = &error {
            handles.push(error.0);
        }
        let mut jobs = [job.0.0];
        let mut attributes = Attributes::new(if capabilities.is_some() { 3 } else { 2 })?;
        attributes.set_handles(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &mut handles)?;
        attributes.set_handles(PROC_THREAD_ATTRIBUTE_JOB_LIST, &mut jobs)?;
        if let Some(capabilities) = capabilities.as_mut() {
            security::check(unsafe {
                UpdateProcThreadAttribute(
                    attributes.pointer(),
                    0,
                    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                    (capabilities as *mut windows_sys::Win32::Security::SECURITY_CAPABILITIES)
                        .cast(),
                    size_of::<windows_sys::Win32::Security::SECURITY_CAPABILITIES>(),
                    null_mut(),
                    null(),
                )
            })?;
        }
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = input.0;
        startup.StartupInfo.hStdOutput = log.0;
        startup.StartupInfo.hStdError = error.as_ref().map_or(log.0, |error| error.0);
        if let Some(desktop) = desktop.as_mut() {
            startup.StartupInfo.lpDesktop = desktop.name.as_mut_ptr();
        }
        startup.lpAttributeList = attributes.pointer();
        let flags = CREATE_SUSPENDED
            | CREATE_UNICODE_ENVIRONMENT
            | EXTENDED_STARTUPINFO_PRESENT
            | CREATE_NO_WINDOW;
        let mut information = PROCESS_INFORMATION::default();
        let object_attributes = descriptor
            .as_ref()
            .map(crate::process_access::Policy::attributes);
        let object_attributes = object_attributes.as_ref().map_or(null(), |value| value);
        let mut launch = || {
            let success = unsafe {
                match token.as_ref() {
                    Some(token) => CreateProcessAsUserW(
                        token.0,
                        app.as_ptr(),
                        command.as_mut_ptr(),
                        object_attributes,
                        object_attributes,
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
                        object_attributes,
                        object_attributes,
                        1,
                        flags,
                        environment.as_ptr().cast(),
                        directory.as_ptr(),
                        &startup.StartupInfo,
                        &mut information,
                    ),
                }
            };
            // Capture GetLastError before the impersonation guard reverts.
            security::check(success)
        };
        let created = match creation_context {
            CreationContext::Process => launch(),
            CreationContext::Role | CreationContext::RoleProfileEnvironment => {
                ensure!(
                    capabilities.is_some(),
                    "role creation context requires an AppContainer"
                );
                let token = token
                    .as_ref()
                    .context("role creation context requires its owned token")?;
                crate::persona::as_user(token, launch)
            }
        };
        created.with_context(|| {
            format!(
                "native suspended process creation failed (appcontainer={},role_context={})",
                capabilities.is_some(),
                !matches!(creation_context, CreationContext::Process)
            )
        })?;
        let mut process = OwnedProcess {
            process: Handle(information.hProcess),
            initial_thread: None,
            pid: information.dwProcessId,
            job,
            _desktop: desktop,
            _directory: None,
            launch_diagnostics: None,
            stop_on_drop: true,
        };
        let thread = Handle(information.hThread);
        let prepared: Result<()> = (|| {
            if let Some(policy) = &descriptor {
                policy
                    .seal(process.process.0)
                    .context("verify suspended resource process policy")?;
                policy
                    .seal(thread.0)
                    .context("verify suspended resource thread policy")?;
            }
            #[cfg(test)]
            if self
                .environment
                .get("WUKONG_NATIVE_SEAL_LABELS")
                .is_some_and(|value| value == "1")
            {
                crate::process_access::seal_low_integrity(process.process.0)?;
                crate::process_access::seal_low_integrity(thread.0)?;
            }
            if let Some(capabilities) = capabilities.as_ref() {
                seal_sandbox_token(&process, capabilities.AppContainerSid)?;
                if matches!(start, Start::Execute)
                    && self.executable == std::env::current_exe()?
                    && self
                        .arguments
                        .first()
                        .is_some_and(|value| value == "isolation-probe")
                {
                    process.launch_diagnostics = Some(
                        match launch_probe::inspect(&process, self.directory, &child_environment) {
                            Ok(value) => value,
                            Err(error) => serde_json::json!({"diagnostic_error":
                            format!("{error:#}").chars().take(256).collect::<String>()}),
                        },
                    );
                }
            }
            #[cfg(test)]
            if let Some(policy) = self.environment.get("WUKONG_NATIVE_SEAL_PRIVILEGES") {
                let mut token = null_mut();
                security::check(unsafe {
                    OpenProcessToken(
                        process.process.0,
                        windows_sys::Win32::Security::TOKEN_QUERY
                            | windows_sys::Win32::Security::TOKEN_ADJUST_PRIVILEGES,
                        &mut token,
                    )
                })?;
                seal_token_privileges(&Handle(token), policy == "traverse")?;
            }
            if !matches!(start, Start::InspectSuspended | Start::AwaitDebugger { .. }) {
                ensure!(
                    unsafe { ResumeThread(thread.0) } != u32::MAX,
                    "cannot resume owned native child"
                );
            }
            Ok(())
        })();
        if let Err(error) = prepared {
            if let Err(drain) = process.stop(Duration::from_secs(10)) {
                return Err(DrainFailure(anyhow::anyhow!(
                    "native child preparation failed: {error:#}; Job drain failed: {drain:#}"
                ))
                .into());
            }
            return Err(error);
        }
        if matches!(start, Start::AwaitDebugger { .. }) {
            process.initial_thread = Some(thread);
        }
        Ok(process)
    }
    pub(crate) fn sandbox_creation_diagnostics(
        &self,
        persona: &crate::persona::Persona,
        failure: &anyhow::Error,
    ) -> Result<serde_json::Value> {
        ensure!(
            failure.downcast_ref::<DrainFailure>().is_none(),
            "creation controls refused after an uncertain original Job drain"
        );
        creation_probe::inspect(self, persona)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn debug_target_cannot_run_or_transfer_before_os_attachment() -> Result<()> {
        let root = tempfile::tempdir()?;
        let executable = std::env::current_exe()?;
        let log = File::create(root.path().join("held.log"))?;
        let input = File::open("NUL")?;
        let environment = BTreeMap::from([
            ("SystemRoot".into(), std::env::var_os("SystemRoot").unwrap()),
            ("WUKONG_NATIVE_JOB_FIXTURE".into(), "leaf".into()),
        ]);
        let mut target = Spawn {
            executable: &executable,
            arguments: &[
                "--exact".into(),
                "processes::tests::native_job_fixture_entry".into(),
                "--nocapture".into(),
            ],
            directory: root.path(),
            environment: &environment,
            log: &log,
            input: &input,
        }
        .create(
            None,
            None,
            None,
            None,
            Start::AwaitDebugger { streams: None },
            CreationContext::Process,
        )?;
        assert!(!target.release_debug_start()?);
        assert!(target.initial_thread.is_some());
        assert!(!target.exited());
        assert!(!root.path().join("leaf.pid").exists());
        assert!(SystemHost::take(target).is_err());
        assert!(!root.path().join("leaf.pid").exists());
        Ok(())
    }

    #[test]
    fn owned_system_host_control_transfers_without_killing_and_drains_on_another_thread()
    -> Result<()> {
        let root = tempfile::tempdir()?;
        let executable = std::env::current_exe()?;
        let log = File::create(root.path().join("host.log"))?;
        let input = File::open("NUL")?;
        let mut environment = BTreeMap::new();
        environment.insert("SystemRoot".into(), std::env::var_os("SystemRoot").unwrap());
        environment.insert("WUKONG_NATIVE_JOB_FIXTURE".into(), "parent".into());
        let source = Spawn {
            executable: &executable,
            arguments: &[
                "--exact".into(),
                "processes::tests::native_job_fixture_entry".into(),
                "--nocapture".into(),
            ],
            directory: root.path(),
            environment: &environment,
            log: &log,
            input: &input,
        }
        .inert_fixture()?;
        let host = SystemHost::take(source)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !root.path().join("leaf.pid").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid: u32 = std::fs::read_to_string(root.path().join("leaf.pid"))?.parse()?;
        let leaf = Handle(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) });
        ensure!(
            !leaf.0.is_null() && !host.exited(),
            "transfer killed the owned host or lost its descendant"
        );
        std::thread::spawn(move || host.stop(Duration::from_secs(10)))
            .join()
            .unwrap()?;
        assert_eq!(unsafe { WaitForSingleObject(leaf.0, 1000) }, WAIT_OBJECT_0);
        Ok(())
    }
    #[test]
    fn native_job_fixture_entry() {
        let Some(mode) = std::env::var_os("WUKONG_NATIVE_JOB_FIXTURE") else {
            return;
        };
        if mode == "privileges" {
            let mut token = null_mut();
            security::check(unsafe {
                OpenProcessToken(
                    GetCurrentProcess(),
                    windows_sys::Win32::Security::TOKEN_QUERY,
                    &mut token,
                )
            })
            .unwrap();
            let token = Handle(token);
            let count = privilege_count(&token).unwrap();
            if count > 0 {
                verify_sandbox_privileges(&token).unwrap();
            } else {
                assert!(verify_sandbox_privileges(&token).is_err());
            }
            traversal_fixture(count == 1).unwrap();
            std::fs::write(
                std::env::current_dir()
                    .unwrap()
                    .join("observed-privileges.txt"),
                count.to_string(),
            )
            .unwrap();
        } else if mode == "parent" || mode == "parent-exits" {
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
                std::env::current_dir().unwrap().join("leaf.pending"),
                std::process::id().to_string(),
            )
            .unwrap();
            std::fs::rename("leaf.pending", "leaf.pid").unwrap();
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
    }
    fn traversal_fixture(retained: bool) -> Result<()> {
        use windows_sys::Win32::Security::{
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
        };
        fn set(path: &Path, sddl: &str) -> Result<()> {
            let descriptor = SecurityDescriptor::from_sddl(sddl)?;
            let encoded = security::wide(path.as_os_str())?;
            security::check(unsafe {
                SetFileSecurityW(
                    encoded.as_ptr(),
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    descriptor.attributes().lpSecurityDescriptor,
                )
            })
            .with_context(|| format!("set owned traversal fixture DACL: {}", path.display()))
        }
        let directory = std::env::current_dir()?.join("traverse-fixture");
        std::fs::create_dir(&directory).context("create inert traversal fixture directory")?;
        let readable = directory.join("readable.bin");
        let private = directory.join("private.bin");
        std::fs::write(&readable, b"readable")?;
        std::fs::write(&private, b"private")?;
        let sid = security::current_user_sid()?;
        let full = format!("D:P(A;;FA;;;SY)(A;;FA;;;{sid})");
        set(&readable, &full)?;
        set(&private, "D:P(A;;FA;;;SY)")?;
        set(&directory, &format!("D:P(A;;FA;;;SY)(A;;RCWDWO;;;{sid})"))?;
        let result = (|| -> Result<()> {
            if retained {
                ensure!(
                    std::fs::read(&readable)? == b"readable",
                    "traverse fixture content differs"
                );
            } else {
                ensure!(
                    std::fs::read(&readable)
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied),
                    "zero-privilege fixture bypassed a closed ancestor"
                );
            }
            ensure!(
                std::fs::read(&private)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied),
                "traverse privilege bypassed final file read policy"
            );
            ensure!(
                std::fs::read_dir(&directory)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied),
                "traverse privilege granted directory enumeration"
            );
            Ok(())
        })();
        // Restore only this fixed fixture's own entries before reporting errors.
        set(&directory, &full)?;
        set(&private, &full)?;
        result
    }
    fn privilege_count(token: &Handle) -> Result<u32> {
        let mut size = 0;
        unsafe {
            GetTokenInformation(token.0, TokenPrivileges, null_mut(), 0, &mut size);
        }
        ensure!(size > 0 && size < 65536, "invalid fixture privilege list");
        let mut data = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        security::check(unsafe {
            GetTokenInformation(
                token.0,
                TokenPrivileges,
                data.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        })?;
        Ok(unsafe { *data.as_ptr().cast::<u32>() })
    }
    #[test]
    fn suspended_child_privileges_are_removed_without_changing_its_parent() -> Result<()> {
        sealed_privilege_fixture("1", 0)
    }
    #[test]
    fn suspended_sandbox_retains_only_traverse_without_changing_its_parent() -> Result<()> {
        sealed_privilege_fixture("traverse", 1)
    }
    fn sealed_privilege_fixture(policy: &str, expected: u32) -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let mut parent = null_mut();
        security::check(unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                windows_sys::Win32::Security::TOKEN_QUERY,
                &mut parent,
            )
        })?;
        let parent = Handle(parent);
        let before = privilege_count(&parent)?;
        let mut values = BTreeMap::new();
        for name in [
            "SystemRoot",
            "PATH",
            "TEMP",
            "TMP",
            "SYSTEMDRIVE",
            "PROGRAMDATA",
        ] {
            if let Some(value) = std::env::var_os(name) {
                values.insert(name.into(), value);
            }
        }
        values.insert("WUKONG_NATIVE_JOB_FIXTURE".into(), "privileges".into());
        values.insert("WUKONG_NATIVE_SEAL_PRIVILEGES".into(), policy.into());
        let log_path = temporary.path().join("fixture.log");
        let log = File::create(&log_path)?;
        let input = File::open("NUL")?;
        let executable = std::env::current_exe()?;
        let child = Spawn {
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
        .inert_fixture()?;
        let deadline = Instant::now() + Duration::from_secs(15);
        while !child.exited() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let code = child.exit_code()?;
        child.stop(Duration::from_secs(10))?;
        drop(child);
        drop(log);
        ensure!(
            code == Some(0),
            "sealed child failed: {}",
            String::from_utf8_lossy(&crate::deployment::read_bytes(&log_path, 8192)?)
        );
        assert_eq!(
            std::fs::read_to_string(temporary.path().join("observed-privileges.txt"))?,
            expected.to_string()
        );
        assert_eq!(privilege_count(&parent)?, before);
        Ok(())
    }
    #[test]
    fn creation_control_drains_without_running_the_child() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let mut values = BTreeMap::new();
        for name in ["SystemRoot", "PATH", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                values.insert(name.into(), value);
            }
        }
        values.insert("WUKONG_NATIVE_JOB_FIXTURE".into(), "privileges".into());
        values.insert("WUKONG_NATIVE_SEAL_LABELS".into(), "1".into());
        let log = File::create(temporary.path().join("fixture.log"))?;
        let input = File::open("NUL")?;
        let executable = std::env::current_exe()?;
        let child = Spawn {
            executable: &executable,
            arguments: &[
                "--exact".into(),
                "processes::tests::native_job_fixture_entry".into(),
            ],
            directory: temporary.path(),
            environment: &values,
            log: &log,
            input: &input,
        }
        .create(
            None,
            None,
            None,
            None,
            Start::InspectSuspended,
            CreationContext::Process,
        )?;
        assert!(!child.exited());
        assert_eq!(child.job().active_processes()?, 1);
        child.stop(Duration::from_secs(10))?;
        assert_eq!(child.job().active_processes()?, 0);
        assert!(!temporary.path().join("observed-privileges.txt").exists());
        Ok(())
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
        .create(
            None,
            None,
            None,
            None,
            Start::Execute,
            CreationContext::Process,
        )?;
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
        .create(
            None,
            None,
            None,
            None,
            Start::Execute,
            CreationContext::Process,
        )?;
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
