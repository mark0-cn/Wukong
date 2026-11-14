//! Creation-only controls for the signed self-test binary after a launch error.
//! Every result is an unresumed, Job-drained diagnostic, never an execution fallback.
use super::*;
use serde_json::{Value, json};

struct Case {
    name: &'static str,
    stripped: bool,
    appcontainer: bool,
    desktop: bool,
    explicit_policy: bool,
    context: CreationContext,
}
impl Case {
    const ORDINARY: Self = Self {
        name: "ordinary_role",
        stripped: false,
        appcontainer: true,
        desktop: true,
        explicit_policy: true,
        context: CreationContext::RoleProfileEnvironment,
    };
}

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
    for case in [
        Case::ORDINARY,
        Case {
            name: "without_profile_environment",
            context: CreationContext::Role,
            ..Case::ORDINARY
        },
        Case {
            name: "system_creation_context",
            context: CreationContext::Process,
            ..Case::ORDINARY
        },
        Case {
            name: "stripped_source",
            stripped: true,
            ..Case::ORDINARY
        },
        Case {
            name: "no_appcontainer",
            appcontainer: false,
            context: CreationContext::Process,
            ..Case::ORDINARY
        },
        Case {
            name: "default_desktop",
            desktop: false,
            ..Case::ORDINARY
        },
        Case {
            name: "default_object_policy",
            explicit_policy: false,
            ..Case::ORDINARY
        },
    ] {
        let name = case.name;
        let result = control(spawn, persona, case);
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
fn control(spawn: &Spawn<'_>, persona: &crate::persona::Persona, case: Case) -> Result<()> {
    let token = if case.stripped {
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
    let desktop = case
        .desktop
        .then(|| Desktop::sandbox(&persona.sid_text()?, persona.logon_sid()))
        .transpose()?;
    let capabilities =
        case.appcontainer
            .then_some(windows_sys::Win32::Security::SECURITY_CAPABILITIES {
                AppContainerSid: persona.sid(),
                Capabilities: null_mut(),
                CapabilityCount: 0,
                Reserved: 0,
            });
    let policy = case
        .explicit_policy
        .then(|| crate::process_access::Policy::owned(persona, None))
        .transpose()?;
    let process = spawn.create(
        Some(token),
        desktop,
        capabilities,
        policy,
        Start::InspectSuspended,
        case.context,
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
