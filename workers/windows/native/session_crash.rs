//! A fresh contained MCP observation corroborates an already observed native crash.
use crate::{
    debugger_mcp::Client,
    deployment,
    inputs::digest,
    resource_host::Host,
    resource_minimize::{self, Candidate},
    resource_process::Request,
    security::AclPolicy,
    session_backend::Backend,
    session_request::{self, Mode},
    session_target::Prepared,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn address(value: &Value) -> Result<u64> {
    let text = value.as_str().context("MCP address is not a string")?;
    let hex = text
        .strip_prefix("0x")
        .context("MCP address is not hexadecimal")?;
    ensure!(
        !hex.is_empty() && hex.len() <= 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "MCP address exceeds its hexadecimal bound"
    );
    Ok(u64::from_str_radix(hex, 16)?)
}
async fn exception(client: &mut Client, id: &str) -> Result<(Value, Value)> {
    let stop = client.structured("go", json!({"session_id":id})).await?;
    ensure!(
        stop["timed_out"] == false
            && stop["target_gone"] == false
            && stop["interrupted"] == false
            && stop["thread"].as_u64().is_some_and(|thread| thread > 0),
        "debugger did not stop at an identified thread"
    );
    let event = client
        .structured(
            "exception_triage",
            json!({"session_id":id,"frames":16,"scan_stack":false}),
        )
        .await?;
    Ok((stop, event))
}

fn confirm_unhandled(
    first: &Value,
    first_stop: &Value,
    second: &Value,
    second_stop: &Value,
) -> Result<()> {
    ensure!(
        first["first_chance"] == true
            && second["first_chance"] == false
            && first_stop["thread"]
                .as_u64()
                .is_some_and(|thread| thread > 0)
            && first_stop["thread"] == second_stop["thread"]
            && ["code", "address", "flags", "noncontinuable", "parameters"]
                .iter()
                .all(|key| !first[*key].is_null() && first[*key] == second[*key]),
        "first-chance context did not reach the same unhandled exception on the same thread"
    );
    Ok(())
}

async fn snapshot(client: &mut Client, target: &mut Prepared, code: u32) -> Result<Value> {
    let opened = target.open(client).await?;
    let id = opened["session_id"]
        .as_str()
        .context("MCP session handle missing")?
        .to_owned();
    let observed = async {
        let mut symbols = json!({"session_id":id,"path":target.symbols(),"append":false});
        if target.identity()?.has_symbols() { symbols["reload"] = json!("/f"); }
        client.call("set_symbol_path", symbols).await?;
        // Capture the fault context before WOW64 can report the second chance
        // from its native notification layer. A first chance alone never qualifies.
        client.call("execute", json!({"session_id":id,"command":
            "sxe 0xc0000005; sxe 0xc000001d; sxe 0xc0000094; sxe 0xc00000fd; sxe 0xc0000409"})).await?;
        for startup in 0..=2 {
            let (stop, event) = exception(client, &id).await?;
            let record = &event["exception"];
            let observed_code = address(&record["code"])?;
            if observed_code == u64::from(code) && record["first_chance"].is_boolean() {
                let modules = client.structured("modules", json!({"session_id":id,"limit":2000})).await?;
                let pc = address(&record["address"])?;
                ensure!(pc == address(&stop["stopped_at"])? && pc != 0,
                    "exception and stopped instruction address differ");
                let registers = client.structured("registers", json!({"session_id":id})).await?;
                ensure!(address(&registers["instruction_pointer"])? == pc,
                    "exception does not match current instruction pointer");
                let identity = target.identity()?;
                let location = identity.normalize(pc, &modules)?;
                let frames = event["frames"].as_array().context("MCP exception frames missing")?;
                ensure!(!frames.is_empty() && frames.len() <= 16, "MCP exception frames exceed bounds");
                ensure!(address(&frames[0]["address"])? == pc,
                    "exception stack does not start at the fault instruction");
                let mut normalized = Vec::new();
                for frame in frames {
                    if let Some(location) = identity.normalize(address(&frame["address"])?, &modules)? {
                        normalized.push(location);
                    }
                    if normalized.len() == 6 { break; }
                }
                let stable_frames: Vec<_> = normalized.iter().map(|frame|
                    (&frame["module_sha256"], &frame["value"])).collect();
                let signature = location.as_ref().map(|pc| digest(&("windows.location.v1", code,
                    &pc["module_sha256"], &pc["value"], stable_frames))).transpose()?
                    .map(|sha| format!("location:{sha}"));
                let (unhandled_stop, unhandled_event) = if record["first_chance"] == true {
                    let (next_stop, next_event) = exception(client, &id).await?;
                    confirm_unhandled(record, &stop, &next_event["exception"], &next_stop)?;
                    (next_stop, next_event)
                } else { (stop.clone(), event.clone()) };
                return Ok(json!({"source":"windbg_mcp","exception_code":code,
                    "pc_location":location,"frames":normalized,"location_signature":signature,
                    "signature_strength":if signature.is_some(){"location_and_frames"}else{"unresolved"},
                    "identity":identity,"modules":modules,"exception":event,"registers":registers,
                    "stop":stop,"unhandled_exception":unhandled_event,"unhandled_stop":unhandled_stop,
                    "context_source":if record["first_chance"] == true {"first_chance"}else{"second_chance"},
                    "content_scope":"admitted main image; other module paths do not establish hashes"}));
            }
            ensure!(startup < 2 && record["first_chance"] == true
                && matches!(observed_code, 0x80000003 | 0x4000001f),
                "debugger stopped at a different or unresolved exception");
        }
        anyhow::bail!("startup stop budget exhausted")
    }.await;
    let released = client
        .structured("end_session", json!({"session_id":id}))
        .await;
    if let Ok(released) = &released {
        ensure!(
            released["released"] == true
                && released["worker_terminated"] == true
                && released["recovery_required"] == false,
            "MCP release remains uncertain"
        );
    }
    released?;
    observed
}

pub(crate) fn observe(
    host: &Host,
    request: &Request,
    candidate: &Candidate,
    index: u16,
    code: u32,
    cancel: &Path,
    deadline: Instant,
) -> Result<Value> {
    if cancel.try_exists()? || Instant::now() >= deadline {
        return Ok(json!({"location_signature":null,"error":"cancelled or deadline expired"}));
    }
    let session = session_request::Request {
        schema: crate::session_host::SCHEMA,
        target: request.clone(),
        mode: Mode::Launch,
    };
    let policy = AclPolicy::private_vm();
    let transcript = policy.create_file(&resource_minimize::result_path(
        host,
        request,
        &format!("trial-{index}.mcp"),
    ))?;
    let mut backend = Backend::launch(
        host,
        &session,
        &resource_minimize::debugger_nonce(
            &host.deployment.installation_id,
            &request.resource_id,
            index,
        )?,
        transcript,
        |debugger, _scratch| {
            Ok(Prepared::Launch {
                process: None,
                image: Box::new(crate::resource_target::Prepared::open_candidate(
                    host,
                    request,
                    index,
                    candidate.open()?,
                    Some(debugger),
                )?),
            })
        },
    )?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let observation = runtime.block_on(async {
        let mut client = backend.connect().await?;
        let output = backend.target.watch();
        let watch = crate::session_host::watch(
            cancel,
            deadline.min(Instant::now() + Duration::from_secs(request.timeout_secs)),
            output.as_ref(),
            backend.diagnostics.watch(),
            &backend.helper,
        );
        let observed = tokio::select! {
            result = snapshot(&mut client, &mut backend.target, code) => result,
            result = watch => {result?; unreachable!()},
        };
        let closed = client.close().await;
        closed?;
        observed
    });
    let closed = backend.close();
    runtime.shutdown_timeout(Duration::from_secs(5));
    let closed = closed?;
    let mut result = match observation {
        Ok(value) => value,
        Err(error) => {
            json!({"location_signature":null,"error":error.to_string().chars().take(1024).collect::<String>()})
        }
    };
    for (suffix, bytes) in [
        ("debug-log", &closed.diagnostics),
        (
            "debug-stdout",
            &closed
                .captured
                .as_ref()
                .context("crash observer did not execute target")?
                .stdout,
        ),
        ("debug-stderr", &closed.captured.as_ref().unwrap().stderr),
    ] {
        policy.publish(
            &resource_minimize::result_path(host, request, &format!("trial-{index}.{suffix}")),
            bytes,
        )?;
    }
    result["cleanup"] = json!({"jobs_empty":true,"profiles_removed":true,"scratch_removed":true});
    result["request_sha256"] = json!(digest(request)?);
    result["input"] = serde_json::to_value(&candidate.reference)?;
    result["trial"] = json!(index);
    deployment::publish_json(
        &resource_minimize::result_path(host, request, &format!("trial-{index}.observation.json")),
        &result,
        &policy,
    )?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wow64_context_requires_a_matching_unhandled_event() {
        let first = json!({"code":"0xc0000005","address":"0x009b7263","flags":"0x0",
            "noncontinuable":false,"parameters":["0x1","0x1"],"first_chance":true});
        let mut second = first.clone();
        second["first_chance"] = json!(false);
        let stop = json!({"thread":6648,"stopped_at":"0x009b7263"});
        let unhandled = json!({"thread":6648,"stopped_at":"0x770f95c8"});
        confirm_unhandled(&first, &stop, &second, &unhandled).unwrap();
        // A different native notification PC is allowed only after matching the
        // original exception, including the thread and fault parameters.
        for (key, value) in [
            ("first_chance", json!(true)),
            ("code", json!("0xc000001d")),
            ("address", json!("0x009b7264")),
            ("flags", json!("0x1")),
            ("noncontinuable", json!(true)),
            ("parameters", json!(["0x1", "0x2"])),
            ("parameters", Value::Null),
        ] {
            let mut changed = second.clone();
            changed[key] = value;
            assert!(confirm_unhandled(&first, &stop, &changed, &unhandled).is_err());
        }
        for thread in [Value::Null, json!(0), json!(6649)] {
            let mut changed = unhandled.clone();
            changed["thread"] = thread;
            assert!(confirm_unhandled(&first, &stop, &second, &changed).is_err());
        }
    }
}
