//! The operator injects faults only in the separately enrolled dedicated VM.
//! Coordination files contain selectors/provenance, never lease tokens or keys.
use super::{
    process_acceptance::{Probe, terminal},
    tests::{enrolled_fixture_address, read_fixture_profile},
};
use crate::runtime::{
    RuntimeClient, RuntimeConfig,
    protocol::{EnvironmentLease, WorkerError, WorkerErrorKind},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Fault {
    Preparation,
    Host,
    Service,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    config: RuntimeConfig,
    manifest_sha256: String,
    probe: Probe,
    fault: Fault,
    ready: PathBuf,
    resume: PathBuf,
    output: PathBuf,
}
fn publish(path: &Path, value: &Value) -> Result<()> {
    let mut file = fs::File::create_new(path)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    Ok(())
}
async fn resume(path: &Path) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    while !path.try_exists()? {
        ensure!(
            tokio::time::Instant::now() < deadline,
            "independent VM fault action was not observed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}
async fn reconciled(client: &RuntimeClient, id: &str) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let status = client.call("job.status", json!({"job_id":id})).await?;
        if status["state"] == "lost" && status["cleanup_pending"] == false {
            return Ok(status);
        }
        ensure!(
            matches!(
                status["state"].as_str(),
                Some("running" | "queued" | "lost")
            ) && tokio::time::Instant::now() < deadline,
            "interrupted native resource did not reconcile: {status}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "requires an enrolled dedicated Windows VM and independently injected preparation/host/service fault"]
async fn remote_native_interrupted_process_acceptance() -> Result<()> {
    let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_PROCESS_FAULT_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    for path in [&profile.ready, &profile.resume, &profile.output] {
        ensure!(
            path.is_absolute() && !path.try_exists()?,
            "fault coordination/output paths must be new absolute paths"
        );
    }
    let mut bytes = Vec::new();
    fs::File::open(&profile.probe.path)?
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 32 * 1024 * 1024
            && hex::encode(Sha256::digest(&bytes)) == profile.probe.sha256,
        "fault fixture probe differs"
    );
    let client = RuntimeClient::connect(profile.config.clone()).await?;
    ensure!(
        client.identity().package_sha256 == profile.manifest_sha256,
        "fault fixture package differs"
    );
    let run = format!(
        "native-fault-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let acquire = json!({"run_id":run,"owner_id":"fault-controller-fixture"});
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call("environment.lease.acquire", acquire.clone())
            .await?,
    )?;
    lease.validate(&run, "fault-controller-fixture", client.identity())?;
    let bound = client.with_lease(lease.clone());
    let uploaded = bound.call("file.put", json!({"path":"fault/target.exe","data_base64":STANDARD.encode(&bytes),"executable":true,"request_id":"fault-upload"})).await?;
    ensure!(
        uploaded["sha256"] == profile.probe.sha256
            && uploaded["native_input"]["identity"] == serde_json::to_value(client.identity())?
            && uploaded["native_input"]["epoch"] == lease.epoch,
        "fault fixture lacks current native input provenance"
    );
    let request = json!({"argv":[uploaded["path"]],"timeout_secs":90,"request_id":"interrupted-start","call_id":"interrupted-start"});
    if profile.fault == Fault::Preparation {
        publish(
            &profile.ready,
            &json!({"scope":"dedicated_native_fault_ready","fault":profile.fault,"identity":client.identity(),"run_id":run,"owner_id":"fault-controller-fixture","input":uploaded}),
        )?;
        resume(&profile.resume).await?;
    }
    let started = bound.call("job.start", request.clone()).await?;
    let id = started["job_id"]
        .as_str()
        .context("interrupted resource ID is absent")?;
    if profile.fault != Fault::Preparation {
        publish(
            &profile.ready,
            &json!({"scope":"dedicated_native_fault_ready","fault":profile.fault,"identity":client.identity(),"run_id":run,"owner_id":"fault-controller-fixture","input":uploaded,"job":started}),
        )?;
        resume(&profile.resume).await?;
    }
    let active = if profile.fault == Fault::Service {
        // Only a read is used to consume the dead transport. The mutating
        // stale start below is not retried and must reach the live lease gate.
        let _ = bound.call("job.status", json!({"job_id":id})).await;
        let fresh = RuntimeClient::connect(profile.config.clone()).await?;
        ensure!(
            fresh.identity() == client.identity(),
            "service fault unexpectedly changed OS/package identity"
        );
        fresh
    } else {
        client.clone()
    };
    let current: EnvironmentLease =
        serde_json::from_value(active.call("environment.lease.acquire", acquire).await?)?;
    if profile.fault == Fault::Service {
        ensure!(
            current.epoch > lease.epoch,
            "service fault reused a native lease epoch"
        );
        let stale = bound
            .call("job.start", request.clone())
            .await
            .err()
            .context("old service fence reached a native start")?;
        ensure!(
            stale
                .downcast_ref::<WorkerError>()
                .is_some_and(|error| error.kind == WorkerErrorKind::LeaseLost),
            "old fence failed for an unrelated reason: {stale:#}"
        );
    }
    let rebound = active.with_lease(current.clone());
    let status = reconciled(&rebound, id).await?;
    ensure!(
        status["result"]["outcome"] == "unknown" && status["result"].get("exit_code").is_none(),
        "interrupted execution manufactured a result"
    );
    let cached = rebound.call("job.start", request).await?;
    ensure!(
        cached == status,
        "interrupted start was relaunched or its result changed"
    );
    let artifacts = rebound.call("job.artifacts", json!({"job_id":id})).await?;
    let artifacts = artifacts.as_array().context("recovery artifacts missing")?;
    ensure!(
        artifacts.len() == 1,
        "interrupted execution fabricated stdout/stderr artifacts"
    );
    let original = rebound
        .call("file.get", json!({"path":artifacts[0]["path"]}))
        .await?;
    let proof_bytes = STANDARD.decode(
        original["data_base64"]
            .as_str()
            .context("recovery receipt bytes missing")?,
    )?;
    ensure!(
        original["sha256"] == hex::encode(Sha256::digest(&proof_bytes))
            && original["sha256"] == status["result"]["recovery_receipt_sha256"],
        "recovery receipt content differs"
    );
    let proof: Value = serde_json::from_slice(&proof_bytes)?;
    ensure!(
        proof["scope"] == "owned_resource_recovery"
            && proof["resource_id"] == id
            && proof["owner"]["identity"] == serde_json::to_value(client.identity())?
            && proof["owner"]["run_id"] == run
            && proof["jobs_empty"] == true
            && proof["profiles_removed"] == true
            && proof["scratch_removed"] == true,
        "native recovery receipt does not bind cleanup to the original resource"
    );
    if profile.fault == Fault::Service {
        ensure!(
            proof["guardian_found"] == true,
            "service recovery did not reopen the externally retained guardian Job"
        );
    }
    let next = rebound.call("file.put", json!({"path":"fault/after-recovery.exe","data_base64":STANDARD.encode(&bytes),"executable":true,"request_id":"after-upload"})).await?;
    let next = rebound
        .call(
            "job.start",
            json!({"argv":[next["path"],"--echo"],"timeout_secs":5,"request_id":"after-recovery"}),
        )
        .await?;
    let after = terminal(
        &rebound,
        next["job_id"]
            .as_str()
            .context("post-recovery job is absent")?,
    )
    .await?;
    ensure!(
        after["state"] == "completed" && after["stdout"] == "wukong-worker-selftest\n",
        "cleanup did not release native admission: {after}"
    );
    active
        .call("environment.lease.release", json!({"lease":current}))
        .await?;
    active.close().await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":true,"scope":"interrupted_native_process_vm","fault":profile.fault,"identity":active.identity(),"first_epoch":lease.epoch,"last_epoch":current.epoch,"original_input":uploaded,"started":started,"reconciled":status,"recovery_receipt":proof,"recovery_artifact":original,"after_recovery":after,"production_execution":false}),
    )?;
    Ok(())
}
