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
pub(super) fn register(
    root: &Path,
    deployment: &Deployment,
    manifest: &VerifiedManifest,
) -> Result<()> {
    // SAFETY: Both arguments are null, which selects the local machine's default SCM database;
    // the returned handle is checked before `Service` adopts it, because that Drop closes it.
    let manager = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CREATE_SERVICE) };
    ensure!(
        !manager.is_null(),
        "cannot open SCM for offline worker installation: {}",
        std::io::Error::last_os_error()
    );
    let manager = Service(manager);
    let name = security::wide(OsStr::new(&pipe_transport::service_name(
        &deployment.installation_id,
    )?))?;
    let command = processes::command_line(&[
        security::local_file_path(
            &deployment
                .package(root)
                .join(&manifest.manifest().entrypoints.worker),
        )?
        .into_os_string(),
        "service".into(),
        deployment.installation_id.clone().into(),
        security::local_file_path(&root.join("metadata/deployment.json"))?.into_os_string(),
    ])?;
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
    // SAFETY: `service.0` is the non-null service handle proven by the `ensure!` above, and a zero
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
