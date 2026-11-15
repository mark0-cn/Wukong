//! Validated executor observations and immutable output publication; no lifecycle commits.
use crate::{
    artifacts::outputs::Artifacts, executors::resource_process::Request,
    isolation::security::AclPolicy,
};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) fn publish(artifacts: &Artifacts, request: &Request) -> Result<()> {
    if request.replay.is_some() {
        replay(artifacts, request)
    } else {
        process(artifacts, request)
    }
}
pub(crate) fn process(artifacts: &Artifacts, request: &Request) -> Result<()> {
    let id = &request.resource_id;
    let private = AclPolicy::private_vm();
    let path = artifacts.result_path(id, "json")?;
    private.verify(&path, false)?;
    let bytes = crate::artifacts::io::read_bytes(&path, 12 * 1024 * 1024)?;
    let result: Value = serde_json::from_slice(&bytes)?;
    let streams = crate::executors::resource_process::output(request, &result)?;
    for (name, data) in ["stdout", "stderr"].into_iter().zip(&streams) {
        artifacts.retain(&artifacts.result_path(id, name)?, data)?;
    }
    let reason = result["stop_reason"]
        .as_str()
        .context("native stop reason is absent")?;
    let exit = result["exit_code"]
        .as_u64()
        .context("native exit is absent")?;
    let state = match reason {
        "exited" => {
            if exit == 0 {
                "completed"
            } else {
                "failed"
            }
        }
        "cancelled" => "cancelled",
        "deadline" => "timed_out",
        "output_limit" | "io_failure" => "failed",
        _ => anyhow::bail!("invalid native stop reason"),
    };
    fn tail(bytes: &[u8]) -> String {
        String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(4096)..]).into()
    }
    let value = json!({"job_id":id,"kind":"process","state":state,"native_owner":request.owner,"stdout":tail(&streams[0]),"stderr":tail(&streams[1]),"result":{"exit_code":exit,"stop_reason":reason,"stdout_sha256":result["stdout_sha256"],"stderr_sha256":result["stderr_sha256"],"stdin_written":result["stdin_written"],"stdin_complete":result["stdin_complete"],"stdin_error":result["stdin_error"],"native_receipt_sha256":hex::encode(Sha256::digest(&bytes)),"jobs_empty":true,"profiles_removed":true,"scratch_removed":true,
            "duration_secs":result["duration_secs"],"output_bytes_retained":{"stdout":streams[0].len(),"stderr":streams[1].len()},"output_truncated":reason == "output_limit" || streams.iter().any(|stream|stream.len()>4096)},"isolation":"appcontainer"});
    artifacts.retain(
        &artifacts.result_path(id, "status.json")?,
        &serde_json::to_vec(&value)?,
    )?;
    Ok(())
}
pub(crate) fn replay(artifacts: &Artifacts, request: &Request) -> Result<()> {
    let id = &request.resource_id;
    let path = artifacts.result_path(id, "json")?;
    AclPolicy::private_vm().verify(&path, false)?;
    let minimizing = request
        .replay
        .as_ref()
        .is_some_and(|replay| replay.configuration.minimization.is_some());
    let bytes =
        crate::artifacts::io::read_bytes(&path, if minimizing { 4 * 1024 * 1024 } else { 65536 })?;
    let receipt: Value = serde_json::from_slice(&bytes)?;
    let mut result = if minimizing {
        crate::executors::resource_minimize::summarize(request, &receipt, |suffix| {
            let path = artifacts.result_path(id, suffix)?;
            AclPolicy::private_vm().verify(&path, false)?;
            crate::artifacts::io::read_bytes(&path, 32 * 1024 * 1024)
        })?
    } else {
        crate::executors::resource_replay::summarize(request, &receipt, |index| {
            let path = artifacts.result_path(id, &format!("trial-{index}.json"))?;
            AclPolicy::private_vm().verify(&path, false)?;
            crate::artifacts::io::read_bytes(&path, 12 * 1024 * 1024)
        })?
    };
    result["native_receipt_sha256"] = json!(hex::encode(Sha256::digest(&bytes)));
    let status = json!({"job_id":id,"kind":request.kind(),"state":if receipt["cancelled"] == true {"cancelled"} else if minimizing && !receipt["failed"].is_null() {"failed"} else {"completed"},
            "native_owner":request.owner,"result":result,"isolation":"appcontainer"});
    artifacts.retain(
        &artifacts.result_path(id, "status.json")?,
        &serde_json::to_vec(&status)?,
    )?;
    Ok(())
}
