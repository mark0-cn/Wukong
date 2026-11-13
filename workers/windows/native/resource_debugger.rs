//! Qualify an external MCP debugger against an independently contained sealed target.
use super::*;
use crate::{debugger_mcp::Client, processes::ResourceLimits};
use std::os::windows::io::OwnedHandle;
use wukong_worker_package::VerifiedManifest;

pub(super) fn check(
    deployment: &Deployment,
    root: &Path,
    resources: &Path,
    manifest: &VerifiedManifest,
    architecture: &str,
    system_environment: &std::collections::BTreeMap<String, std::ffi::OsString>,
) -> Result<Value> {
    ensure!(
        matches!(architecture, "x64" | "x86"),
        "invalid trusted architecture"
    );
    let package = deployment.package(root);
    let executable = manifest
        .manifest()
        .entrypoints
        .windbg_mcp
        .as_ref()
        .context("sealed package has no external WinDbg MCP backend")?;
    let probe_member = format!("runtime/selftest/{architecture}/wukong-probe.exe");
    ensure!(
        manifest.manifest().files.contains_key(&probe_member),
        "sealed probe is absent"
    );
    let (mut target_persona, target_nonce) = create_persona(deployment, root, Role::Target)?;
    let (mut debugger_persona, debugger_nonce) = create_persona(deployment, root, Role::Debugger)?;
    let target_sid = target_persona.sid_text()?;
    let debugger_sid = debugger_persona.sid_text()?;
    ensure!(
        target_sid != debugger_sid,
        "debugger reused its target identity"
    );
    let target_directory = security::ResourceDirectory::create(
        &resources.join(&target_nonce),
        &target_sid,
        target_persona.logon_sid(),
    )?;
    let debugger_directory = security::ResourceDirectory::create(
        &resources.join(&debugger_nonce),
        &debugger_sid,
        debugger_persona.logon_sid(),
    )?;
    let target_path = target_directory.path().to_owned();
    let debugger_path = debugger_directory.path().to_owned();
    let private = AclPolicy::private_vm();
    let target_log = private.create_file(
        &root
            .join("metadata")
            .join(format!("resource-target-{target_nonce}.log")),
    )?;
    let input = fs::File::open("NUL")?;
    let target_environment = scratch_environment(system_environment, &target_path);
    let mut target = Spawn {
        executable: &package.join(&probe_member),
        arguments: &["--debug-once".into()],
        directory: &target_path,
        environment: &target_environment,
        log: &target_log,
        input: &input,
    }
    .sandbox_debug_target(&target_persona, target_directory, &debugger_persona)?;

    let (input, writer) = std::io::pipe()?;
    let (reader, output) = std::io::pipe()?;
    let input = fs::File::from(OwnedHandle::from(input));
    let output = fs::File::from(OwnedHandle::from(output));
    let log = private.create_file(
        &root
            .join("metadata")
            .join(format!("resource-debugger-{debugger_nonce}.log")),
    )?;
    let transcript = private.create_file(
        &root
            .join("metadata")
            .join(format!("resource-debugger-{debugger_nonce}.jsonl")),
    )?;
    let symbols = package.join(format!("runtime/selftest/{architecture}"));
    let mut environment = scratch_environment(system_environment, &debugger_path);
    environment.insert("_NT_SYMBOL_PATH".into(), symbols.as_os_str().to_owned());
    environment.insert("_NT_ALT_SYMBOL_PATH".into(), symbols.as_os_str().to_owned());
    let helper = Spawn {
        executable: &package.join(executable),
        arguments: &["--tools".into(), "session,inspect,exec,crash".into()],
        directory: &debugger_path,
        environment: &environment,
        log: &output,
        input: &input,
    }
    .sandbox_streamed(
        &debugger_persona,
        debugger_directory,
        &log,
        ResourceLimits::new(4, 1024 * 1024 * 1024)?,
    )?;
    drop((input, output));

    // Private station/profile setup finishes before starting I/O threads. Drain
    // the Job before shutting down the runtime's blocking pipe readers.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let outcome = runtime.block_on(async {
        let reader = tokio::fs::File::from_std(fs::File::from(OwnedHandle::from(reader)));
        let writer = tokio::fs::File::from_std(fs::File::from(OwnedHandle::from(writer)));
        let mut client = Client::connect(reader, writer, transcript).await?;
        let result = probe(&mut client, &mut target, &symbols, architecture).await;
        let closed = client.close().await;
        let initial_breaks = result?;
        closed?;
        Ok::<_, anyhow::Error>(initial_breaks)
    });
    let helper_stopped = helper.stop(Duration::from_secs(10));
    let target_stopped = target.stop(Duration::from_secs(10));
    runtime.shutdown_timeout(Duration::from_secs(5));
    helper_stopped?;
    target_stopped?;
    ensure!(
        helper.job().active_processes()? == 0 && target.job().active_processes()? == 0,
        "debugger qualification Jobs are not empty"
    );
    drop((helper, target, log, target_log));
    target_persona.close()?;
    debugger_persona.close()?;
    fs::remove_dir(&target_path)?;
    fs::remove_dir(&debugger_path)?;
    let initial_breaks = outcome?;
    Ok(
        json!({"passed":true,"architecture":architecture,"engine":"dbgeng","backend":"windbg_mcp",
        "target_appcontainer_sid":target_sid,"debugger_appcontainer_sid":debugger_sid,
        "target_logon_sid":target_persona.logon_sid(),"debugger_logon_sid":debugger_persona.logon_sid(),
        "privileges":["SeChangeNotifyPrivilege"],"capabilities":0,"debug_privilege_enabled":false,
        "owned_attach":true,"pre_execution_attach":true,"single_pass_target":true,
        "wow64_loader_break_observed":initial_breaks.wow64,
        "native_initial_break_observed":initial_breaks.native,
        "registers_observed":true,"target_module_observed":true,
        "symbols_loaded":true,"symbol_breakpoint_hit":true,"stack_symbol_observed":true,
        "jobs_empty":true,"profiles_removed":true,"scratch_removed":true}),
    )
}

async fn probe(
    client: &mut Client,
    target: &mut crate::processes::OwnedProcess,
    symbols: &Path,
    architecture: &str,
) -> Result<InitialBreaks> {
    let opened = client.attach_owned(target).await?;
    let session = opened["session_id"]
        .as_str()
        .context("MCP debugger omitted its session handle")?;
    let route = json!({"session_id":session});
    let result = async {
        client
            .call(
                "set_symbol_path",
                json!({"session_id":session,"path":symbols,"append":false}),
            )
            .await?;
        let breakpoint = client
            .structured(
                "set_breakpoint",
                json!({"session_id":session,"expression":"wukong_probe!fixture_step"}),
            )
            .await?;
        let address = breakpoint["breakpoint"]["address"]
            .as_str()
            .context("fixture breakpoint is unresolved")?;
        let mut stop = client.structured("go", route.clone()).await?;
        let mut initial_breaks = InitialBreaks::default();
        while stop["stopped_at"] != address
            && stop["timed_out"] == false
            && stop["target_gone"] == false
            && stop["interrupted"] == false
        {
            let event = client
                .structured(
                    "exception_triage",
                    json!({"session_id":session,"frames":1,"scan_stack":false}),
                )
                .await?;
            ensure!(
                initial_breaks.observe(architecture, &stop, &event),
                "single-pass fixture stopped at an unexpected or repeated exception"
            );
            // At most one native break-in and one WOW64 loader notification,
            // in either order. This policy belongs only to the sealed fixture.
            stop = client.structured("go", route.clone()).await?;
        }
        ensure!(
            stop["timed_out"] == false
                && stop["target_gone"] == false
                && stop["interrupted"] == false
                && stop["stopped_at"] == address,
            "contained debugger did not hit the fixture breakpoint"
        );
        let registers = client.structured("registers", route.clone()).await?;
        let ip = registers["instruction_pointer"]
            .as_str()
            .context("debugger has no program counter")?;
        let ip_value = u64::from_str_radix(ip.trim_start_matches("0x"), 16)?;
        ensure!(
            ip == address && ip_value > 0 && (architecture != "x86" || ip_value < 1 << 32),
            "contained program counter differs from the target architecture/breakpoint"
        );
        let modules = client.structured("modules", route.clone()).await?;
        ensure!(
            modules["modules"].as_array().is_some_and(|rows| rows
                .iter()
                .any(|row| row["name"] == "wukong_probe" && row["symbols"] == "pdb")),
            "contained debugger did not load the sealed fixture PDB"
        );
        let stack = client.structured("backtrace", route.clone()).await?;
        ensure!(
            stack["frames"][0]["symbol"] == "wukong_probe!fixture_step"
                && stack["frames"][0]["address"] == address,
            "contained backtrace does not match the fixture breakpoint"
        );
        Ok(initial_breaks)
    }
    .await;
    let released = client.structured("end_session", route).await?;
    ensure!(
        released["released"] == true && released["recovery_required"] == false,
        "contained debugger did not release its session"
    );
    result
}

#[derive(Default)]
struct InitialBreaks {
    native: bool,
    wow64: bool,
}

impl InitialBreaks {
    fn observe(&mut self, architecture: &str, stop: &Value, event: &Value) -> bool {
        let exception = &event["exception"];
        if event["status"] != "ok"
            || event["stored_crash_context"] != false
            || exception["first_chance"] != true
            || exception["noncontinuable"] != false
            || !exception["address"].is_string()
            || exception["address"] != stop["stopped_at"]
        {
            return false;
        }
        let frame = &event["frames"][0];
        let observed = if architecture == "x86" && exception["code"] == "0x4000001f" {
            &mut self.wow64
        } else if exception["code"] == "0x80000003"
            && event["kind"] == "breakpoint"
            && event["frames_from_stored_context"] == false
            && frame["module"] == "ntdll"
            && frame["symbol"] == "ntdll!DbgBreakPoint"
            && frame["displacement"] == "0x0"
            && frame["address"] == exception["address"]
        {
            &mut self.native
        } else {
            return false;
        };
        !std::mem::replace(observed, true)
    }
}

pub(super) fn validate_report(value: &Value) -> Result<()> {
    let debuggers = value["dbgeng"]
        .as_array()
        .context("missing contained DbgEng qualification")?;
    ensure!(
        debuggers.len() == 2,
        "contained DbgEng architecture count differs"
    );
    for (debugger, architecture) in debuggers.iter().zip(["x64", "x86"]) {
        ensure!(
            debugger["architecture"] == architecture
                && debugger["engine"] == "dbgeng"
                && debugger["backend"] == "windbg_mcp"
                && debugger["privileges"] == json!(["SeChangeNotifyPrivilege"])
                && debugger["capabilities"] == 0
                && debugger["debug_privilege_enabled"] == false
                && debugger["native_initial_break_observed"].is_boolean()
                && debugger["wow64_loader_break_observed"]
                    .as_bool()
                    .is_some_and(|observed| !observed || architecture == "x86"),
            "contained DbgEng identity or privilege report differs"
        );
        for key in [
            "passed",
            "owned_attach",
            "pre_execution_attach",
            "single_pass_target",
            "registers_observed",
            "target_module_observed",
            "symbols_loaded",
            "symbol_breakpoint_hit",
            "stack_symbol_observed",
            "jobs_empty",
            "profiles_removed",
            "scratch_removed",
        ] {
            ensure!(
                debugger[key] == true,
                "contained DbgEng did not prove {key}"
            );
        }
        for role in ["target", "debugger"] {
            security::validate_app_sid(
                debugger[format!("{role}_appcontainer_sid")]
                    .as_str()
                    .context("missing debugger persona")?,
            )?;
            security::validate_logon_sid(
                debugger[format!("{role}_logon_sid")]
                    .as_str()
                    .context("missing debugger logon")?,
            )?;
        }
        ensure!(
            debugger["target_appcontainer_sid"] != debugger["debugger_appcontainer_sid"]
                && debugger["target_logon_sid"] != debugger["debugger_logon_sid"],
            "debugger shares its target persona"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_continuation_refuses_other_faults_or_a_different_stop() {
        let stop = json!({"stopped_at":"0x00000000771e7d89"});
        let event = json!({"status":"ok","stored_crash_context":false,
            "exception":{"code":"0x4000001f","address":stop["stopped_at"],"first_chance":true,"noncontinuable":false}});
        let mut observed = InitialBreaks::default();
        assert!(observed.observe("x86", &stop, &event));
        assert!(!observed.observe("x86", &stop, &event));
        assert!(!InitialBreaks::default().observe("x64", &stop, &event));
        for (field, value) in [
            ("code", json!("0xc0000005")),
            ("code", json!("0x80000003")),
            ("first_chance", json!(false)),
            ("noncontinuable", json!(true)),
            ("address", json!("0x0000000000000000")),
            ("address", Value::Null),
        ] {
            let mut other = event.clone();
            other["exception"][field] = value;
            assert!(!InitialBreaks::default().observe("x86", &stop, &other));
        }
        assert!(!InitialBreaks::default().observe("x86", &json!({}), &json!({})));
    }

    #[test]
    fn fixture_native_break_requires_matching_live_ntdll_frame_and_is_not_replayed() {
        let stop = json!({"stopped_at":"0x00007ffab334e370"});
        let event = json!({"status":"ok","stored_crash_context":false,
            "frames_from_stored_context":false,"kind":"breakpoint",
            "exception":{"code":"0x80000003","address":stop["stopped_at"],"first_chance":true,"noncontinuable":false},
            "frames":[{"address":stop["stopped_at"],"module":"ntdll","symbol":"ntdll!DbgBreakPoint","displacement":"0x0"}]});
        let mut observed = InitialBreaks::default();
        assert!(observed.observe("x86", &stop, &event));
        assert!(!observed.observe("x86", &stop, &event));
        for (pointer, value) in [
            ("/status", json!("error")),
            ("/stored_crash_context", json!(true)),
            ("/frames_from_stored_context", json!(true)),
            ("/kind", json!("unknown")),
            ("/exception/first_chance", json!(false)),
            ("/exception/noncontinuable", json!(true)),
            ("/frames/0/symbol", json!("wukong_probe!fixture_step")),
            ("/frames/0/module", json!("wukong_probe")),
            ("/frames/0/displacement", json!("0x1")),
            ("/frames/0/address", json!("0x0000000000000000")),
            ("/frames", Value::Null),
        ] {
            let mut other = event.clone();
            *other.pointer_mut(pointer).unwrap() = value;
            assert!(!InitialBreaks::default().observe("x86", &stop, &other));
        }
        let wow64 = json!({"status":"ok","stored_crash_context":false,
            "exception":{"code":"0x4000001f","address":stop["stopped_at"],"first_chance":true,"noncontinuable":false}});
        for events in [[&event, &wow64], [&wow64, &event]] {
            let mut observed = InitialBreaks::default();
            for event in events {
                assert!(observed.observe("x86", &stop, event));
            }
            assert!(observed.native && observed.wow64);
            assert!(!observed.observe("x86", &stop, &event));
            assert!(!observed.observe("x86", &stop, &wow64));
        }
    }
}
