//! Fixed sealed x64/x86 targets only. These probes never admit model-selected
//! executables, process IDs, helper paths, source paths or symbol servers.
use super::*;
use std::io::Write;
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
    let target_log_path = root
        .join("metadata")
        .join(format!("resource-target-{target_nonce}.log"));
    let target_log = private.create_file(&target_log_path)?;
    let input = fs::File::open("NUL")?;
    let target_environment = scratch_environment(system_environment, &target_path);
    let target = Spawn {
        executable: &package.join(&probe_member),
        arguments: &[],
        directory: &target_path,
        environment: &target_environment,
        log: &target_log,
        input: &input,
    }
    .sandbox_debug_target(&target_persona, target_directory, &debugger_persona)?;
    // The PID comes from our still-held kernel process handle, never a worker.
    let requests = [
        json!({"id":1,"operation":"doctor"}),
        json!({"id":2,"operation":"open","mode":"attach","pid":target.pid(),"architecture":architecture,"timeout_secs":20}),
        json!({"id":3,"operation":"inspect","kind":"registers","limit":8}),
        json!({"id":4,"operation":"inspect","kind":"modules","limit":64}),
        json!({"id":5,"operation":"close"}),
    ];
    let request_path = root
        .join("metadata")
        .join(format!("resource-debugger-{debugger_nonce}.jsonl"));
    {
        let mut file = private.create_file(&request_path)?;
        for request in requests {
            serde_json::to_writer(&mut file, &request)?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
    }
    let debugger_input = fs::File::open(&request_path)?;
    let log_path = root
        .join("metadata")
        .join(format!("resource-debugger-{debugger_nonce}.log"));
    let log = private.create_file(&log_path)?;
    let debugger_environment = scratch_environment(system_environment, &debugger_path);
    let engine = package.join(&manifest.manifest().entrypoints.dbgeng_library);
    let symbols = package.join(format!("runtime/selftest/{architecture}"));
    let helper = Spawn {
        executable: &package.join(&manifest.manifest().entrypoints.dbgeng_helper),
        arguments: &[
            engine
                .parent()
                .context("missing sealed engine directory")?
                .as_os_str()
                .to_owned(),
            symbols.as_os_str().to_owned(),
        ],
        directory: &debugger_path,
        environment: &debugger_environment,
        log: &log,
        input: &debugger_input,
    }
    .sandbox(&debugger_persona, debugger_directory)?;
    wait(&helper, Duration::from_secs(45))?;
    let code = helper.exit_code()?;
    helper.stop(Duration::from_secs(10))?;
    target.stop(Duration::from_secs(10))?;
    ensure!(
        helper.job().active_processes()? == 0 && target.job().active_processes()? == 0,
        "debugger qualification Jobs are not empty"
    );
    drop(helper);
    drop(target);
    drop(log);
    drop(target_log);
    drop(debugger_input);
    let bytes = deployment::read_bytes(&log_path, 512 * 1024)?;
    ensure!(
        code == Some(0),
        "contained {architecture} DbgEng failed (exit {code:?}): {}",
        String::from_utf8_lossy(&bytes[..bytes.len().min(4096)])
    );
    validate_replies(&bytes, architecture)?;
    target_persona.close()?;
    debugger_persona.close()?;
    fs::remove_dir(&target_path)?;
    fs::remove_dir(&debugger_path)?;
    Ok(
        json!({"passed":true,"architecture":architecture,"engine":"dbgeng",
        "target_appcontainer_sid":target_sid,"debugger_appcontainer_sid":debugger_sid,
        "target_logon_sid":target_persona.logon_sid(),"debugger_logon_sid":debugger_persona.logon_sid(),
        "privileges":["SeChangeNotifyPrivilege"],"capabilities":0,"debug_privilege_enabled":false,
        "owned_attach":true,"registers_observed":true,"target_module_observed":true,
        "jobs_empty":true,"profiles_removed":true,"scratch_removed":true}),
    )
}

fn validate_replies(bytes: &[u8], architecture: &str) -> Result<()> {
    let replies: Vec<Value> = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice)
        .collect::<std::result::Result<_, _>>()?;
    ensure!(
        replies.len() == 5,
        "contained DbgEng reply count differs: {}",
        String::from_utf8_lossy(&bytes[..bytes.len().min(4096)])
    );
    for (index, reply) in replies.iter().enumerate() {
        ensure!(
            reply["id"] == (index + 1) as u64
                && reply.get("error").is_none()
                && reply["output_truncated"] == false,
            "contained DbgEng operation failed: {reply}"
        );
    }
    ensure!(
        replies[0]["result"]["engine"] == "dbgeng"
            && replies[0]["result"]["local_symbols_only"] == true,
        "contained debugger did not select the local-only engine"
    );
    let opened = &replies[1]["result"];
    ensure!(
        opened["mode"] == "live"
            && opened["debuggee_class"] == 2
            && opened["architecture"] == if architecture == "x86" { 0x14c } else { 0x8664 }
            && opened["debug_privilege_enabled"] == false
            && opened["state"]["execution_state"] == "stopped",
        "contained attachment differs: {opened}"
    );
    ensure!(
        replies[2]["result"]["data"]["registers"]
            .as_array()
            .is_some_and(|values| !values.is_empty()),
        "contained debugger observed no registers"
    );
    ensure!(
        replies[3]["result"]["data"]["modules"]
            .as_array()
            .is_some_and(|modules| modules.iter().any(|module| module["image"]
                .as_str()
                .is_some_and(|image| image
                    .rsplit(['\\', '/'])
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("wukong-probe.exe"))))),
        "contained debugger did not observe the sealed target module"
    );
    ensure!(
        replies[4]["result"]["state"] == "closed",
        "contained debugger did not close"
    );
    Ok(())
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
                && debugger["privileges"] == json!(["SeChangeNotifyPrivilege"])
                && debugger["capabilities"] == 0
                && debugger["debug_privilege_enabled"] == false,
            "contained DbgEng identity or privilege report differs"
        );
        for key in [
            "passed",
            "owned_attach",
            "registers_observed",
            "target_module_observed",
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
    fn transcript() -> Vec<Value> {
        vec![
            json!({"id":1,"result":{"engine":"dbgeng","local_symbols_only":true},"output_truncated":false}),
            json!({"id":2,"result":{"mode":"live","debuggee_class":2,"architecture":0x8664,
                "debug_privilege_enabled":false,"state":{"execution_state":"stopped"}},"output_truncated":false}),
            json!({"id":3,"result":{"data":{"registers":[{"name":"rip"}]}},"output_truncated":false}),
            json!({"id":4,"result":{"data":{"modules":[{"image":"C:\\sealed\\wukong-probe.exe"}]}},"output_truncated":false}),
            json!({"id":5,"result":{"state":"closed"},"output_truncated":false}),
        ]
    }
    fn encode(replies: &[Value]) -> Vec<u8> {
        replies
            .iter()
            .flat_map(|reply| {
                let mut bytes = serde_json::to_vec(reply).unwrap();
                bytes.push(b'\n');
                bytes
            })
            .collect()
    }
    #[test]
    fn debugger_qualification_rejects_partial_privileged_or_wrong_target_transcripts() -> Result<()>
    {
        let baseline = transcript();
        validate_replies(&encode(&baseline), "x64")?;
        assert!(validate_replies(&encode(&baseline), "x86").is_err());
        assert!(validate_replies(&encode(&baseline[..4]), "x64").is_err());
        for (index, key, replacement) in [
            (1, "debug_privilege_enabled", json!(true)),
            (1, "debuggee_class", json!(1)),
            (1, "state", json!({"execution_state":"running"})),
        ] {
            let mut replies = transcript();
            replies[index]["result"][key] = replacement;
            assert!(validate_replies(&encode(&replies), "x64").is_err());
        }
        let mut replies = transcript();
        replies[3]["result"]["data"]["modules"][0]["image"] = json!("C:\\other.exe");
        assert!(validate_replies(&encode(&replies), "x64").is_err());
        let mut replies = transcript();
        replies[2]["error"] = json!("access denied");
        assert!(validate_replies(&encode(&replies), "x64").is_err());
        Ok(())
    }
}
