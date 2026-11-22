//! Service Control Manager support for installing and activating the Worker.
use super::{accounts::Role, deployment::Deployment, service_access};
use crate::{
    isolation::{processes, security},
    rpc::pipe_transport,
};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsStr,
    mem::size_of,
    path::Path,
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Storage::FileSystem::{READ_CONTROL, WRITE_DAC, WRITE_OWNER},
    System::Services::*,
};
use wukong_worker_package::VerifiedManifest;

struct Service(SC_HANDLE);
impl Drop for Service {
    fn drop(&mut self) {
        // SAFETY: `Service` owns the `SC_HANDLE` from `OpenSCManagerW`/`CreateServiceW`, and
        // `Drop` runs once, so every non-null handle is closed exactly once.
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}
fn service_command(
    root: &Path,
    deployment: &Deployment,
    manifest: &VerifiedManifest,
) -> Result<Vec<u16>> {
    processes::command_line(&[
        security::local_file_path(
            &deployment
                .package(root)
                .join(&manifest.manifest().entrypoints.worker),
        )?
        .into_os_string(),
        "service".into(),
        deployment.installation_id.clone().into(),
        security::local_file_path(&root.join("metadata/deployment.json"))?.into_os_string(),
    ])
}

fn open(manager: SC_HANDLE, deployment: &Deployment, access: u32) -> Result<Service> {
    let name = security::wide(OsStr::new(&pipe_transport::service_name(
        &deployment.installation_id,
    )?))?;
    // SAFETY: manager is a live SCM handle and name is a terminated wide string;
    // the returned non-null handle is owned and closed by Service.
    let handle = unsafe { OpenServiceW(manager, name.as_ptr(), access) };
    ensure!(
        !handle.is_null(),
        "cannot open the owned worker service: {}",
        std::io::Error::last_os_error()
    );
    Ok(Service(handle))
}

fn manager(access: u32) -> Result<Service> {
    // SAFETY: null names select the local SCM; the checked handle is owned by Service.
    let handle = unsafe { OpenSCManagerW(null(), null(), access) };
    ensure!(
        !handle.is_null(),
        "cannot open SCM for the owned worker service: {}",
        std::io::Error::last_os_error()
    );
    Ok(Service(handle))
}

pub(super) fn register(
    root: &Path,
    deployment: &Deployment,
    manifest: &VerifiedManifest,
) -> Result<()> {
    let manager = manager(SC_MANAGER_CREATE_SERVICE)?;
    let name = security::wide(OsStr::new(&pipe_transport::service_name(
        &deployment.installation_id,
    )?))?;
    let command = service_command(root, deployment, manifest)?;
    // SAFETY: `manager.0` is a non-null SCM handle from the `ensure!` above, and `name`/`command`
    // are NUL-terminated wide buffers in locals that outlive this call; options are null.
    let handle = unsafe {
        CreateServiceW(
            manager.0,
            name.as_ptr(),
            name.as_ptr(),
            SERVICE_START | SERVICE_QUERY_STATUS | READ_CONTROL | WRITE_DAC | WRITE_OWNER,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            command.as_ptr(),
            null(),
            null_mut(),
            null(),
            null(),
            null(),
        )
    };
    ensure!(
        !handle.is_null(),
        "cannot create owned worker service; no existing service will be adopted: {}",
        std::io::Error::last_os_error()
    );
    let service = Service(handle);
    service_access::apply(service.0, &deployment.account(Role::Management)?.sid)
        .context("set owned service status-only relay access")?;
    start_and_wait(&service)
}

fn start_and_wait(service: &Service) -> Result<()> {
    // SAFETY: `service.0` is a live handle created with SERVICE_START, and a zero
    // argument count with a null vector is the documented no-arguments form.
    security::check(unsafe { StartServiceW(service.0, 0, null()) })?;
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        let mut status = SERVICE_STATUS_PROCESS::default();
        let mut returned = 0;
        // SAFETY: `service.0` is a live handle created with SERVICE_QUERY_STATUS, and `status` is a
        // live `SERVICE_STATUS_PROCESS` whose declared buffer size is exactly its own type.
        security::check(unsafe {
            QueryServiceStatusEx(
                service.0,
                SC_STATUS_PROCESS_INFO,
                (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
                size_of::<SERVICE_STATUS_PROCESS>() as u32,
                &mut returned,
            )
        })?;
        if status.dwCurrentState == SERVICE_RUNNING {
            return Ok(());
        }
        ensure!(
            status.dwCurrentState != SERVICE_STOPPED,
            "offline worker service failed readiness (Windows error {}); inspect protected service diagnostics",
            status.dwWin32ExitCode
        );
        ensure!(
            Instant::now() < deadline,
            "offline worker service did not reach readiness"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn verify_current_command(
    root: &Path,
    deployment: &Deployment,
    manifest: &VerifiedManifest,
) -> Result<()> {
    security::require_system()?;
    service_access::verify_registered(
        &deployment.installation_id,
        &deployment.account(Role::Management)?.sid,
    )?;
    let manager = manager(SC_MANAGER_CONNECT)?;
    let service = open(
        manager.0,
        deployment,
        SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS,
    )?;
    let mut bytes = 0;
    // SAFETY: the null-buffer query obtains the required allocation size. The
    // expected ERROR_INSUFFICIENT_BUFFER is checked through the nonzero size.
    unsafe {
        QueryServiceConfigW(service.0, null_mut(), 0, &mut bytes);
    }
    ensure!(
        bytes > 0 && bytes <= 64 * 1024,
        "invalid service command size"
    );
    let mut storage = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
    // SAFETY: storage is usize-aligned and at least `bytes` long; `bytes` receives
    // the actual size and service remains live for the call.
    security::check(unsafe {
        QueryServiceConfigW(service.0, storage.as_mut_ptr().cast(), bytes, &mut bytes)
    })?;
    let config = storage.as_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    // SAFETY: a successful QueryServiceConfigW initialized the structure and its
    // non-null lpBinaryPathName points into the live query allocation.
    let pointer = unsafe { (*config).lpBinaryPathName };
    ensure!(!pointer.is_null(), "owned service has no binary path");
    // SAFETY: Windows returns a NUL-terminated command inside the live buffer.
    let actual = unsafe { wide_string(pointer) };
    let expected = service_command(root, deployment, manifest)?;
    ensure!(
        actual.as_slice() == expected.strip_suffix(&[0]).unwrap_or(&expected),
        "owned service binary path differs from the protected active package"
    );
    Ok(())
}

/// Compare a terminated Windows command buffer with the exact Rust-built wide
/// command, without accepting a prefix or an alternate executable.
unsafe fn wide_string(pointer: *const u16) -> Vec<u16> {
    let mut length = 0;
    // SAFETY: the caller established a live NUL-terminated SCM buffer.
    while unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: the counted prefix is inside that same live buffer.
    unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec()
}

/// Stop only the service whose current path matches the verified installation.
/// The VM's old base remains the rollback boundary if a later update step fails.
pub(super) fn stop_for_upgrade(
    root: &Path,
    deployment: &Deployment,
    manifest: &VerifiedManifest,
) -> Result<()> {
    verify_current_command(root, deployment, manifest)?;
    let manager = manager(SC_MANAGER_CONNECT)?;
    let service = open(manager.0, deployment, SERVICE_STOP | SERVICE_QUERY_STATUS)?;
    let mut status = SERVICE_STATUS::default();
    // SAFETY: the handle has SERVICE_STOP and status is a live output structure.
    if unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } == 0 {
        let error = std::io::Error::last_os_error();
        ensure!(
            error.raw_os_error() == Some(1062),
            "cannot stop the owned worker service: {error}"
        );
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        // SAFETY: the service handle has SERVICE_QUERY_STATUS and status is live.
        security::check(unsafe { QueryServiceStatus(service.0, &mut status) })?;
        if status.dwCurrentState == SERVICE_STOPPED {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "owned worker service did not stop before the upgrade deadline"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Point the existing, owned service at the newly verified package and start it.
/// This never creates a service or changes its installation/account identity.
pub(super) fn activate_upgrade(
    root: &Path,
    deployment: &Deployment,
    manifest: &VerifiedManifest,
) -> Result<()> {
    security::require_system()?;
    service_access::verify_registered(
        &deployment.installation_id,
        &deployment.account(Role::Management)?.sid,
    )?;
    let manager = manager(SC_MANAGER_CONNECT)?;
    let service = open(
        manager.0,
        deployment,
        SERVICE_CHANGE_CONFIG | SERVICE_START | SERVICE_QUERY_STATUS,
    )?;
    let command = service_command(root, deployment, manifest)?;
    // SAFETY: all selectors other than the binary command are NO_CHANGE/null;
    // command is a live terminated wide string and service has CHANGE_CONFIG.
    security::check(unsafe {
        ChangeServiceConfigW(
            service.0,
            SERVICE_NO_CHANGE,
            SERVICE_NO_CHANGE,
            SERVICE_NO_CHANGE,
            command.as_ptr(),
            null(),
            null_mut(),
            null(),
            null(),
            null(),
            null(),
        )
    })?;
    start_and_wait(&service)
}
