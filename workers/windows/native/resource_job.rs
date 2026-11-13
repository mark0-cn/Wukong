//! Durable resource names refer to kernel Jobs, never caller-supplied PIDs.
//! A collision is refused before configuring or assigning any process.
use super::*;
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError},
    Security::Authorization::SE_KERNEL_OBJECT,
    Storage::FileSystem::READ_CONTROL,
    System::SystemServices::JOB_OBJECT_TERMINATE,
};

pub(crate) fn name(installation: &str, resource: &str) -> Result<String> {
    crate::pipe_transport::service_name(installation)?;
    ensure!(
        resource.len() == 32
            && resource
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid native resource Job identity"
    );
    Ok(format!("Global\\WukongResource-{installation}-{resource}"))
}
fn grants() -> BTreeMap<String, u32> {
    // Creation supplies the service's all-access handle. Reopening only needs
    // observation/termination; it never grants assignment or policy changes.
    let access = JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE | READ_CONTROL;
    [("S-1-5-18".into(), access), ("S-1-5-32-544".into(), access)].into()
}
fn descriptor() -> Result<SecurityDescriptor> {
    let entries = grants()
        .into_iter()
        .map(|(sid, mask)| format!("(A;;0x{mask:08x};;;{sid})"))
        .collect::<String>();
    SecurityDescriptor::from_sddl(&format!("O:SYG:SYD:P{entries}"))
}
fn verify(handle: HANDLE) -> Result<()> {
    security::verify_descriptor(
        SecurityDescriptor::from_object(handle, SE_KERNEL_OBJECT)?
            .attributes()
            .lpSecurityDescriptor,
        "S-1-5-18",
        &grants(),
        true,
    )
}
impl Job {
    pub(super) fn resource(installation: &str, resource: &str) -> Result<Self> {
        security::require_system()?;
        let name = security::wide(OsStr::new(&name(installation, resource)?))?;
        let policy = descriptor()?;
        unsafe {
            windows_sys::Win32::Foundation::SetLastError(0);
        }
        let handle = unsafe { CreateJobObjectW(&policy.attributes(), name.as_ptr()) };
        let error = unsafe { GetLastError() };
        let handle = Handle(handle);
        ensure!(
            !handle.0.is_null(),
            "cannot create owned named resource Job: {error}"
        );
        ensure!(
            error != ERROR_ALREADY_EXISTS,
            "native resource Job name already exists; refuse adoption or replay"
        );
        verify(handle.0)?;
        Self(handle).configure(None)
    }
}
fn open(installation: &str, resource: &str, access: u32) -> Result<Option<Handle>> {
    security::require_system()?;
    let name = security::wide(OsStr::new(&name(installation, resource)?))?;
    let raw = unsafe { OpenJobObjectW(access | READ_CONTROL, 0, name.as_ptr()) };
    if raw.is_null() {
        let error = unsafe { GetLastError() };
        ensure!(
            error == ERROR_FILE_NOT_FOUND,
            "cannot inspect owned resource Job: {error}"
        );
        return Ok(None);
    }
    let handle = Handle(raw);
    verify(handle.0)?;
    Ok(Some(handle))
}
/// Called before any persona/input setup in the fresh System execution host.
pub(crate) fn require_membership(installation: &str, resource: &str) -> Result<()> {
    let job = open(installation, resource, JOB_OBJECT_QUERY)?
        .context("native resource guardian Job is absent")?;
    let mut contained = 0;
    security::check(unsafe { IsProcessInJob(GetCurrentProcess(), job.0, &mut contained) })?;
    ensure!(
        contained != 0,
        "System process is outside its owned resource guardian Job"
    );
    Ok(())
}
/// Only a validated private journal intent authorizes this name. A missing Job
/// has either never been created or has been destroyed after all members exit.
/// Access denial, a different object/ACL, and a nonempty drain all fail closed.
pub(crate) fn drain(installation: &str, resource: &str, timeout: Duration) -> Result<bool> {
    let Some(job) = open(
        installation,
        resource,
        JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE,
    )?
    else {
        return Ok(false);
    };
    Job::drain_handle(job.0, timeout)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_guardian_observer_fixture_entry() -> Result<()> {
        let Some(config) = std::env::var_os("WUKONG_TEST_GUARDIAN_OBSERVER") else {
            return Ok(());
        };
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Config {
            installation: String,
            resource: String,
            ready: std::path::PathBuf,
            stop: std::path::PathBuf,
            output: std::path::PathBuf,
        }
        let config: Config =
            serde_json::from_slice(&crate::deployment::read_bytes(Path::new(&config), 4096)?)?;
        let wide = security::wide(OsStr::new(&name(&config.installation, &config.resource)?))?;
        let job =
            Handle(unsafe { OpenJobObjectW(JOB_OBJECT_QUERY | READ_CONTROL, 0, wide.as_ptr()) });
        ensure!(
            !job.0.is_null(),
            "dedicated observer cannot open the resource Job"
        );
        verify(job.0)?;
        let before = Job::active_handle(job.0)?;
        ensure!(
            before > 0,
            "observer must retain the actual live resource Job before the fault"
        );
        use std::io::Write;
        let mut ready = File::create_new(&config.ready)?;
        ready.write_all(&serde_json::to_vec(&serde_json::json!({"scope":"native_test_query_only_job_observer","installation_id":config.installation,"resource_id":config.resource,"pid":std::process::id(),"active_before":before,"access":JOB_OBJECT_QUERY | READ_CONTROL}))?)?;
        ready.sync_all()?;
        drop(ready);
        let deadline = Instant::now() + Duration::from_secs(180);
        while !config.stop.try_exists()? {
            ensure!(
                Instant::now() < deadline,
                "dedicated observer release was not supplied"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let after = Job::active_handle(job.0)?;
        ensure!(
            after == 0,
            "resource guardian still has active processes after recovery"
        );
        let mut output = File::create_new(&config.output)?;
        output.write_all(&serde_json::to_vec(&serde_json::json!({"scope":"native_test_query_only_job_observer","installation_id":config.installation,"resource_id":config.resource,"active_before":before,"active_after":after,"no_termination_or_assignment_authority":true}))?)?;
        output.sync_all()?;
        Ok(())
    }
    #[test]
    fn resource_names_are_fixed_bounded_and_do_not_accept_os_object_paths() -> Result<()> {
        assert_eq!(
            name(&"a".repeat(32), &"b".repeat(32))?,
            format!(
                "Global\\WukongResource-{}-{}",
                "a".repeat(32),
                "b".repeat(32)
            )
        );
        for bad in ["..", "Global\\other", "A0000000000000000000000000000000"] {
            assert!(name(&"a".repeat(32), bad).is_err());
        }
        let root = tempfile::tempdir()?;
        assert!(
            Job::resource(&"a".repeat(32), &"b".repeat(32))
                .err()
                .unwrap()
                .to_string()
                .contains("System")
        );
        assert_eq!(std::fs::read_dir(root.path())?.count(), 0);
        Ok(())
    }
}
