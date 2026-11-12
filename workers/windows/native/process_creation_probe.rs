//! Creation-only controls for the signed self-test binary after a launch error.
//! Every result is an unresumed, Job-drained diagnostic, never an execution fallback.
use super::*;
use serde_json::{Value, json};

pub(super) fn inspect(spawn: &Spawn<'_>, persona: &crate::persona::Persona) -> Result<Value> {
    security::require_system()?;
    ensure!(
        spawn.executable == std::env::current_exe()?
            && spawn
                .arguments
                .first()
                .is_some_and(|value| value == "isolation-probe"),
        "creation controls accept only the sealed native isolation self-test"
    );
    let mut reports = Vec::new();
    for (name, stripped, appcontainer, desktop, explicit_policy) in [
        ("ordinary_role", false, true, true, true),
        ("stripped_source", true, true, true, true),
        ("no_appcontainer", false, false, true, true),
        ("default_desktop", false, true, false, true),
        ("default_object_policy", false, true, true, false),
    ] {
        let result = control(
            spawn,
            persona,
            stripped,
            appcontainer,
            desktop,
            explicit_policy,
        );
        match result {
            Ok(()) => reports.push(json!({"case":name,"creation_and_drain_passed":true,"never_resumed":true,"job_empty":true})),
            Err(error) => {
                if error.downcast_ref::<DrainFailure>().is_some() { return Err(error).context("creation controls stopped at an uncertain Job drain"); }
                reports.push(json!({"case":name,"creation_and_drain_passed":false,"never_resumed":true,
                    "error":format!("{error:#}").chars().take(192).collect::<String>()}));
            }
        }
    }
    Ok(
        json!({"scope":"sealed_selftest_suspended_creation_only","execution_fallback":false,"cases":reports}),
    )
}
fn control(
    spawn: &Spawn<'_>,
    persona: &crate::persona::Persona,
    stripped: bool,
    appcontainer: bool,
    desktop: bool,
    explicit_policy: bool,
) -> Result<()> {
    let token = if stripped {
        without_privileges(persona.token())?
    } else {
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
        Handle(token)
    };
    let desktop = desktop
        .then(|| Desktop::sandbox(&persona.sid_text()?, persona.logon_sid()))
        .transpose()?;
    let capabilities =
        appcontainer.then_some(windows_sys::Win32::Security::SECURITY_CAPABILITIES {
            AppContainerSid: persona.sid(),
            Capabilities: null_mut(),
            CapabilityCount: 0,
            Reserved: 0,
        });
    let policy = explicit_policy
        .then(|| crate::process_access::Policy::owned(persona, None))
        .transpose()?;
    let process = spawn.create(
        Some(token),
        desktop,
        capabilities,
        policy,
        Start::InspectSuspended,
    )?;
    let drained: Result<()> = (|| {
        process.stop(Duration::from_secs(10))?;
        ensure!(
            process.job().active_processes()? == 0,
            "creation control Job did not drain"
        );
        Ok(())
    })();
    drained.map_err(|error| DrainFailure(error).into())
}
