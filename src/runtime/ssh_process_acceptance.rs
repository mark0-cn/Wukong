//! Opt-in actual controller acceptance against the operator's dedicated VM.
use super::inventory_acceptance::inventory;
use super::tests::{enrolled_fixture_address, read_fixture_profile};
use crate::runtime::{
    RuntimeClient, RuntimeConfig,
    protocol::{EnvironmentLease, WorkerError, WorkerErrorKind},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Probe {
    pub(super) path: PathBuf,
    pub(super) sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    config: RuntimeConfig,
    output: PathBuf,
    manifest_sha256: String,
    probes: BTreeMap<String, Probe>,
    #[serde(default)]
    cleanup_adversary: bool,
    #[serde(default)]
    argument_files: bool,
    #[serde(default)]
    resource_inventory: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryProfile {
    config: RuntimeConfig,
    previous: PathBuf,
    previous_sha256: String,
    output: PathBuf,
}

#[tokio::test]
#[ignore = "requires completed process acceptance and an independently observed dedicated VM service restart"]
async fn remote_native_completed_process_recovery_acceptance() -> Result<()> {
    let profile: RecoveryProfile =
        read_fixture_profile("WUKONG_REMOTE_WORKER_PROCESS_RECOVERY_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    ensure!(!profile.output.try_exists()?, "recovery output must be new");
    let mut bytes = Vec::new();
    fs::File::open(profile.previous)?
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 256 * 1024 && hex::encode(Sha256::digest(&bytes)) == profile.previous_sha256,
        "completed process evidence differs"
    );
    let prior: Value = serde_json::from_slice(&bytes)?;
    ensure!(
        prior["passed"] == true && prior["scope"] == "limited_native_process_controller_vm",
        "unsupported prior process evidence"
    );
    let cases = prior["cases"]
        .as_array()
        .context("prior cases are missing")?;
    ensure!(cases.len() == 2, "prior architecture coverage differs");
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        serde_json::to_value(client.identity())? == prior["identity"],
        "restart recovery requires the same installation, package and OS boot"
    );
    let owner = &cases[0]["echo"]["native_owner"];
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call(
                "environment.lease.acquire",
                json!({"run_id":owner["run_id"],"owner_id":owner["owner_id"]}),
            )
            .await?,
    )?;
    ensure!(
        Some(lease.epoch) > prior["last_epoch"].as_u64(),
        "restart did not retain increasing native epochs"
    );
    let bound = client.with_lease(lease.clone());
    let mut observed = Vec::new();
    for case in cases {
        let architecture = case["architecture"]
            .as_str()
            .context("prior architecture missing")?;
        ensure!(
            matches!(architecture, "x64" | "x86"),
            "unknown prior architecture"
        );
        let cached = bound.call("job.start", json!({"argv":[case["input"]["path"],"--echo"],"request_id":format!("echo-{architecture}"),"call_id":format!("echo-{architecture}"),"timeout_secs":5})).await?;
        ensure!(
            cached == case["started"],
            "completed start changed after service restart"
        );
        ensure!(
            terminal(&bound, cached["job_id"].as_str().unwrap()).await? == case["echo"],
            "restart changed completed process status"
        );
        let closed = bound
            .call("job.cancel", json!({"job_id":cached["job_id"]}))
            .await?;
        ensure!(
            closed == case["echo"],
            "cancel reopened completed native work"
        );
        let receipt = case["originals"]
            .as_array()
            .context("original artifacts missing")?
            .iter()
            .find(|value| {
                value["artifact"]["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("/receipt"))
            })
            .context("original receipt missing")?;
        let current = bound
            .call("file.get", json!({"path":receipt["artifact"]["path"]}))
            .await?;
        ensure!(
            current["sha256"] == receipt["artifact"]["sha256"]
                && current["data_base64"] == receipt["data_base64"],
            "restart replaced original native receipt bytes"
        );
        if let Some(arguments) = case.get("argument_files").filter(|value| !value.is_null()) {
            let input = &arguments["input"]["path"];
            let recovered = bound.call("job.start", json!({"argv":[case["input"]["path"],"--read-argument",input,input],"timeout_secs":5,"request_id":format!("argument-{architecture}")})).await?;
            ensure!(
                recovered == arguments["started"],
                "restart replayed or changed an argument-file job"
            );
            ensure!(
                terminal(&bound, recovered["job_id"].as_str().unwrap()).await?
                    == arguments["status"],
                "restart changed argument-file status"
            );
            let receipt = &arguments["receipt"];
            let current = bound
                .call("file.get", json!({"path":receipt["path"]}))
                .await?;
            ensure!(
                current["sha256"] == receipt["sha256"]
                    && current["data_base64"] == receipt["data_base64"],
                "restart replaced argument-file receipt bytes"
            );
        }
        observed.push(cached);
    }
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    fs::File::create_new(profile.output)?.write_all(&serde_json::to_vec_pretty(&json!({"passed":true,"scope":"completed_native_process_after_observed_service_restart","prior_sha256":profile.previous_sha256,"identity":client.identity(),"epoch":lease.epoch,"cached":observed,"production_execution":false}))?)?;
    Ok(())
}

pub(super) async fn terminal(client: &RuntimeClient, id: &str) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(35);
    loop {
        let value = client.call("job.status", json!({"job_id":id})).await?;
        if !matches!(value["state"].as_str(), Some("running" | "queued")) {
            return Ok(value);
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "native job did not settle: {id}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
fn kind(error: &anyhow::Error, expected: WorkerErrorKind) -> bool {
    error
        .downcast_ref::<WorkerError>()
        .is_some_and(|error| error.kind == expected)
}

#[tokio::test]
#[ignore = "requires a separately enrolled dedicated Windows VM and SHA-pinned trusted probes"]
async fn remote_installed_windows_worker_process_acceptance() -> Result<()> {
    let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_PROCESS_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    ensure!(
        !profile.output.try_exists()?
            && profile.probes.keys().map(String::as_str).eq(["x64", "x86"]),
        "process fixture needs a new output and both pinned architectures"
    );
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        client.identity().package_sha256 == profile.manifest_sha256,
        "process fixture package differs"
    );
    super::inventory_acceptance::qualified(&client).await?;
    let unleased = client
        .call(
            "job.start",
            json!({"argv":["files/not-admitted.exe"],"request_id":"unleased"}),
        )
        .await
        .err()
        .context("unleased process start succeeded")?;
    ensure!(
        kind(&unleased, WorkerErrorKind::LeaseLost)
            || unleased
                .downcast_ref::<WorkerError>()
                .is_some_and(|error| error.kind == WorkerErrorKind::Internal
                    && error.message.contains("missing native lease")),
        "unleased request did not reach the native missing-lease guard: {unleased:#}"
    );
    if profile.resource_inventory {
        for method in ["job.list", "resource.list"] {
            let error = client
                .call(method, json!({}))
                .await
                .err()
                .context("unleased native inventory succeeded")?;
            ensure!(
                kind(&error, WorkerErrorKind::LeaseLost)
                    || error
                        .downcast_ref::<WorkerError>()
                        .is_some_and(|error| error.kind == WorkerErrorKind::Internal
                            && error.message.contains("missing native lease")),
                "unleased inventory failed for an unrelated reason: {error:#}"
            );
        }
    }
    let run = format!(
        "native-process-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let acquire = json!({"run_id":run,"owner_id":"process-controller-fixture"});
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call("environment.lease.acquire", acquire.clone())
            .await?,
    )?;
    lease.validate(&run, "process-controller-fixture", client.identity())?;
    let bound = client.with_lease(lease.clone());
    let mut evidence = Vec::new();
    let mut final_start = Value::Null;
    let mut final_receipt = Value::Null;
    let mut final_id = String::new();
    for (architecture, probe) in profile.probes {
        let mut bytes = Vec::new();
        fs::File::open(&probe.path)?
            .take(32 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= 32 * 1024 * 1024 && hex::encode(Sha256::digest(&bytes)) == probe.sha256,
            "trusted {architecture} probe differs"
        );
        let uploaded = bound.call("file.put", json!({"path":format!("process/{}/{architecture}.exe",probe.sha256),"data_base64":STANDARD.encode(bytes),"executable":true,"request_id":format!("upload-{}-{architecture}",probe.sha256)})).await?;
        ensure!(
            uploaded["sha256"] == probe.sha256 && uploaded["native_input"]["executable"] == true,
            "probe lacks native input provenance"
        );
        let request = json!({"argv":[uploaded["path"],"--echo"],"request_id":format!("echo-{architecture}"),"call_id":format!("echo-{architecture}"),"timeout_secs":5});
        let started = bound.call("job.start", request.clone()).await?;
        let id = started["job_id"]
            .as_str()
            .context("native job identity is absent")?;
        let status = terminal(&bound, id).await?;
        ensure!(
            status["state"] == "completed"
                && status["result"]["exit_code"] == 0
                && status["stdout"] == "wukong-worker-selftest\n",
            "native {architecture} execution failed: {status}"
        );
        let duplicate = bound.call("job.start", request.clone()).await?;
        ensure!(
            duplicate == started,
            "completed start retry changed its original result"
        );
        ensure!(
            terminal(&bound, id).await? == status,
            "start retry changed terminal process status"
        );
        let mut conflict = request.clone();
        conflict["argv"] = json!([uploaded["path"]]);
        ensure!(
            bound.call("job.start", conflict).await.is_err(),
            "changed native start reused a request ID"
        );
        let artifacts = bound.call("job.artifacts", json!({"job_id":id})).await?;
        let artifacts = artifacts
            .as_array()
            .context("native artifacts are absent")?;
        ensure!(
            artifacts.len() == 3,
            "native stdout/stderr/receipt artifacts are incomplete"
        );
        let mut originals = Vec::new();
        for artifact in artifacts {
            let result = bound
                .call("file.get", json!({"path":artifact["path"]}))
                .await?;
            let data = STANDARD.decode(
                result["data_base64"]
                    .as_str()
                    .context("artifact bytes are absent")?,
            )?;
            ensure!(
                result["sha256"] == artifact["sha256"]
                    && artifact["sha256"] == hex::encode(Sha256::digest(&data))
                    && artifact["size"].as_u64() == Some(data.len() as u64),
                "native artifact bytes differ"
            );
            let chunk = bound
                .call(
                    "file.read",
                    json!({"path":artifact["path"],"offset":0,"max_bytes":1024}),
                )
                .await?;
            let prefix = STANDARD.decode(
                chunk["data_base64"]
                    .as_str()
                    .context("artifact chunk is absent")?,
            )?;
            ensure!(
                prefix == data[..data.len().min(1024)]
                    && chunk["chunk_sha256"] == hex::encode(Sha256::digest(&prefix)),
                "native chunk bytes differ"
            );
            if artifact["path"].as_str().unwrap().ends_with("/receipt") {
                let receipt: Value = serde_json::from_slice(&data)?;
                ensure!(
                    receipt["owner"]["identity"] == serde_json::to_value(client.identity())?
                        && receipt["resource_id"] == id
                        && receipt["program"]["sha256"] == probe.sha256
                        && receipt["jobs_empty"] == true
                        && receipt["profiles_removed"] == true
                        && receipt["scratch_removed"] == true
                        && status["result"]["native_receipt_sha256"] == artifact["sha256"],
                    "native original receipt binding or cleanup differs"
                );
            }
            originals.push(json!({"artifact":artifact,"data_base64":STANDARD.encode(data)}));
        }
        ensure!(
            bound
                .call(
                    "file.get",
                    json!({"path":format!("native-results/{id}/../../authority.json")})
                )
                .await
                .is_err(),
            "native artifact traversal was accepted"
        );
        let timeout = bound.call("job.start", json!({"argv":[uploaded["path"]],"timeout_secs":1,"request_id":format!("timeout-{architecture}")})).await?;
        let timed_out = terminal(
            &bound,
            timeout["job_id"].as_str().context("timeout ID absent")?,
        )
        .await?;
        ensure!(
            timed_out["state"] == "timed_out" && timed_out["result"]["jobs_empty"] == true,
            "native deadline did not clean up: {timed_out}"
        );
        let active = bound.call("job.start", json!({"argv":[uploaded["path"]],"timeout_secs":15,"request_id":format!("cancel-{architecture}")})).await?;
        let active_id = active["job_id"].as_str().context("cancel ID absent")?;
        if profile.resource_inventory {
            let observed = inventory(&bound).await?;
            ensure!(
                observed["jobs"].as_array().is_some_and(|jobs| jobs
                    .iter()
                    .any(|job| job["job_id"] == active_id && job["state"] == "running")),
                "live native job is missing from authoritative inventory"
            );
        }
        ensure!(
            client
                .call("environment.lease.release", json!({"lease":lease}))
                .await
                .is_err(),
            "active native job did not block lease release"
        );
        let queued_start = bound.call("job.start", json!({"argv":[uploaded["path"],"--echo"],"request_id":format!("queued-{architecture}")}));
        tokio::pin!(queued_start);
        ensure!(
            tokio::time::timeout(Duration::from_millis(500), &mut queued_start)
                .await
                .is_err(),
            "second resource did not wait for the native capacity slot"
        );
        let renewed: EnvironmentLease = serde_json::from_value(
            client
                .call("environment.lease.acquire", acquire.clone())
                .await?,
        )?;
        ensure!(
            renewed.epoch == lease.epoch && renewed.token == lease.token,
            "live native job prevented renewal"
        );
        bound
            .call("job.cancel", json!({"job_id":active_id}))
            .await?;
        let cancelled = terminal(&bound, active_id).await?;
        ensure!(
            cancelled["state"] == "cancelled" && cancelled["result"]["jobs_empty"] == true,
            "native cancellation did not clean up: {cancelled}"
        );
        let queued = queued_start.await?;
        let queued = terminal(
            &bound,
            queued["job_id"].as_str().context("queued job ID absent")?,
        )
        .await?;
        ensure!(
            queued["state"] == "completed" && queued["result"]["jobs_empty"] == true,
            "queued start did not complete after owned cancellation: {queued}"
        );
        let cleanup = if profile.cleanup_adversary {
            let started = bound.call("job.start", json!({"argv":[uploaded["path"],"--cleanup-adversary"],"timeout_secs":10,"request_id":format!("cleanup-{architecture}")})).await?;
            let result = terminal(
                &bound,
                started["job_id"]
                    .as_str()
                    .context("cleanup job is absent")?,
            )
            .await?;
            ensure!(
                result["state"] == "completed"
                    && result["stdout"] == "wukong-cleanup-adversary\n"
                    && result["result"]["jobs_empty"] == true
                    && result["result"]["profiles_removed"] == true
                    && result["result"]["scratch_removed"] == true,
                "native restrictive/readonly cleanup failed: {result}"
            );
            result
        } else {
            Value::Null
        };
        let arguments = if profile.argument_files {
            let bytes = b"native argument\x00\xff\r\n";
            let hash = hex::encode(Sha256::digest(bytes));
            let input = bound.call("file.put", json!({"path":format!("process-arguments/{hash}/input.bin"),"data_base64":STANDARD.encode(bytes),"executable":false,"request_id":format!("argument-data-{hash}")})).await?;
            let rejected = bound.call("job.start", json!({"argv":[uploaded["path"],"--read-argument","files/not-admitted-data.bin","files/not-admitted-data.bin"],"timeout_secs":5,"request_id":format!("missing-argument-{architecture}")})).await;
            ensure!(
                rejected.is_err(),
                "unadmitted argument file reached native execution"
            );
            let started = bound.call("job.start", json!({"argv":[uploaded["path"],"--read-argument",input["path"],input["path"]],"timeout_secs":5,"request_id":format!("argument-{architecture}")})).await?;
            let id = started["job_id"]
                .as_str()
                .context("argument-file job is absent")?;
            let status = terminal(&bound, id).await?;
            ensure!(
                status["state"] == "completed",
                "native argument-file probe failed: {status}"
            );
            let output = bound
                .call(
                    "file.get",
                    json!({"path":format!("native-results/{id}/stdout")}),
                )
                .await?;
            ensure!(
                output["sha256"] == hash
                    && STANDARD.decode(
                        output["data_base64"]
                            .as_str()
                            .context("argument output absent")?
                    )? == bytes,
                "native argument-file bytes changed"
            );
            let receipt = bound
                .call(
                    "file.get",
                    json!({"path":format!("native-results/{id}/receipt")}),
                )
                .await?;
            let proof: Value = serde_json::from_slice(
                &STANDARD.decode(
                    receipt["data_base64"]
                        .as_str()
                        .context("argument receipt absent")?,
                )?,
            )?;
            ensure!(
                proof["argument_files"]
                    .as_array()
                    .is_some_and(|files| files.len() == 1
                        && files[0]["path"] == input["path"]
                        && files[0]["sha256"] == hash
                        && files[0]["executable"] == false),
                "duplicate argument references or data execution policy changed"
            );
            json!({"input":input,"started":started,"status":status,"output":output,"receipt":receipt})
        } else {
            Value::Null
        };
        evidence.push(json!({"architecture":architecture,"input":uploaded,"started":started,"echo":status,"originals":originals,"deadline":timed_out,"cancelled":cancelled,"queued":queued,"cleanup_adversary":cleanup,"argument_files":arguments}));
        final_start = request;
        final_id = id.to_owned();
        final_receipt = started;
    }
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    let successor: EnvironmentLease =
        serde_json::from_value(client.call("environment.lease.acquire", acquire).await?)?;
    ensure!(
        successor.epoch > lease.epoch,
        "native lease epoch did not advance"
    );
    let stale = bound
        .call("job.start", final_start.clone())
        .await
        .err()
        .context("stale fence reached a cached native start")?;
    ensure!(
        kind(&stale, WorkerErrorKind::LeaseLost),
        "stale native start did not fail at the lease fence"
    );
    let rebound = client.with_lease(successor.clone());
    let cached = rebound.call("job.start", final_start).await?;
    ensure!(
        cached == final_receipt
            && terminal(&rebound, &final_id).await? == evidence.last().unwrap()["echo"],
        "new controller epoch relaunched completed work or required new input bytes"
    );
    let observed_inventory = if profile.resource_inventory {
        inventory(&rebound).await?
    } else {
        Value::Null
    };
    client
        .call("environment.lease.release", json!({"lease":successor}))
        .await?;
    let report = json!({"passed":true,"scope":"limited_native_process_controller_vm","identity":client.identity(),"first_epoch":lease.epoch,"last_epoch":successor.epoch,"cases":evidence,"inventory":observed_inventory,"production_execution":false});
    client.close().await?;
    fs::File::create_new(profile.output)?.write_all(&serde_json::to_vec_pretty(&report)?)?;
    Ok(())
}
