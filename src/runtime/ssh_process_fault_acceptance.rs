//! The operator injects faults only in the separately enrolled dedicated VM.
//! Coordination files contain selectors/provenance, never lease tokens or keys.
use super::{
    inventory_acceptance::inventory,
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
    OsBoot,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    config: RuntimeConfig,
    manifest_sha256: String,
    probe: Probe,
    fault: Fault,
    #[serde(default)]
    launch_spec: bool,
    #[serde(default)]
    resource_inventory: bool,
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
async fn resume(path: &Path, fault: Fault) -> Result<()> {
    let deadline = tokio::time::Instant::now()
        + Duration::from_secs(if fault == Fault::OsBoot { 360 } else { 180 });
    while !path.try_exists()? {
        ensure!(
            tokio::time::Instant::now() < deadline,
            "independent VM fault action was not observed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}
async fn echo_after_recovery(client: &RuntimeClient, bytes: &[u8], sha256: &str) -> Result<Value> {
    let next = client.call("file.put", json!({"path":format!("fault/{sha256}/after-recovery.exe"),"data_base64":STANDARD.encode(bytes),"executable":true,"request_id":format!("after-upload-{sha256}")})).await?;
    let next = client
        .call(
            "job.start",
            json!({"argv":[next["path"],"--echo"],"timeout_secs":5,"request_id":"after-recovery"}),
        )
        .await?;
    let after = terminal(
        client,
        next["job_id"]
            .as_str()
            .context("post-recovery job is absent")?,
    )
    .await?;
    ensure!(
        after["state"] == "completed" && after["stdout"] == "wukong-worker-selftest\n",
        "cleanup did not release native admission: {after}"
    );
    Ok(after)
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
#[ignore = "requires an enrolled dedicated Windows VM and independently injected preparation/host/service/OS-boot fault"]
async fn remote_native_interrupted_process_acceptance() -> Result<()> {
    let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_PROCESS_FAULT_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    ensure!(
        profile.fault != Fault::OsBoot || profile.resource_inventory,
        "OS-boot acceptance requires native resource discovery"
    );
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
    let uploaded = bound.call("file.put", json!({"path":format!("fault/{}/target.exe",profile.probe.sha256),"data_base64":STANDARD.encode(&bytes),"executable":true,"request_id":format!("fault-upload-{}",profile.probe.sha256)})).await?;
    ensure!(
        uploaded["sha256"] == profile.probe.sha256
            && uploaded["native_input"]["identity"] == serde_json::to_value(client.identity())?
            && uploaded["native_input"]["epoch"] == lease.epoch,
        "fault fixture lacks current native input provenance"
    );
    let launch = if profile.launch_spec {
        let manifest = json!({"schema_version":1,"base":format!("files/fault/{}",profile.probe.sha256),"target":"target.exe","files":{"target.exe":profile.probe.sha256},"args":["--fault-ready"],"cwd":"work/nested","input_mode":"stdin_pipe","env":{},"timeout_secs":120,"service":{"mode":"offline"}});
        Some(bound.call("file.put",json!({"path":format!("fault/{}/launch.json",profile.probe.sha256),"data_base64":STANDARD.encode(serde_json::to_vec(&manifest)?),"executable":false,"request_id":"fault-launch-upload"})).await?)
    } else {
        None
    };
    let request = if let Some(launch) = &launch {
        json!({"launch_spec":launch["path"],"request_id":"interrupted-start","call_id":"interrupted-start"})
    } else {
        json!({"argv":[uploaded["path"],"--fault-ready"],"timeout_secs":120,"request_id":"interrupted-start","call_id":"interrupted-start"})
    };
    if profile.fault == Fault::Preparation {
        publish(
            &profile.ready,
            &json!({"scope":"dedicated_native_fault_ready","fault":profile.fault,"identity":client.identity(),"run_id":run,"owner_id":"fault-controller-fixture","input":uploaded,"launch":launch}),
        )?;
        resume(&profile.resume, profile.fault).await?;
    }
    let started = bound.call("job.start", request.clone()).await?;
    let id = started["job_id"]
        .as_str()
        .context("interrupted resource ID is absent")?;
    if profile.fault != Fault::Preparation {
        publish(
            &profile.ready,
            &json!({"scope":"dedicated_native_fault_ready","fault":profile.fault,"identity":client.identity(),"run_id":run,"owner_id":"fault-controller-fixture","input":uploaded,"launch":launch,"job":started}),
        )?;
        resume(&profile.resume, profile.fault).await?;
    }
    if profile.fault == Fault::OsBoot {
        // The independent operator checks original cleanup evidence because a
        // new OS binding must never adopt an old run, even for read-only lists.
        ensure!(
            bound
                .call("job.status", json!({"job_id":id}))
                .await
                .is_err(),
            "pre-reboot transport unexpectedly survived"
        );
        let rejected = bound
            .call("job.start", request.clone())
            .await
            .err()
            .context("old OS binding admitted an interrupted start")?;
        ensure!(
            rejected
                .to_string()
                .contains("identity changed; refusing execution"),
            "old OS binding failed for an unrelated reason: {rejected:#}"
        );
        let fresh = RuntimeClient::connect(profile.config.clone()).await?;
        let before = client.identity();
        let after = fresh.identity();
        ensure!(
            before.installation_id == after.installation_id
                && before.package_sha256 == after.package_sha256
                && before.protocol == after.protocol
                && before.boot_id != after.boot_id,
            "OS reboot did not change only the boot identity"
        );
        let old_run: EnvironmentLease =
            serde_json::from_value(fresh.call("environment.lease.acquire", acquire).await?)?;
        ensure!(
            old_run.epoch > lease.epoch,
            "OS reboot reset the native fence"
        );
        let old_scope = fresh.with_lease(old_run.clone());
        for method in ["job.list", "resource.list"] {
            let denied = old_scope
                .call(method, json!({}))
                .await
                .err()
                .context("new boot adopted an old native run")?;
            ensure!(
                denied.downcast_ref::<WorkerError>().is_some()
                    && denied.to_string().contains("native run binding changed"),
                "old run failed for an unrelated reason: {denied:#}"
            );
        }
        let denied = old_scope
            .call("job.status", json!({"job_id":id}))
            .await
            .err()
            .context("new boot exposed an old resource")?;
        ensure!(
            denied.downcast_ref::<WorkerError>().is_some()
                && denied.to_string().contains("another worker binding or run"),
            "old resource failed for an unrelated reason: {denied:#}"
        );
        fresh
            .call("environment.lease.release", json!({"lease":old_run}))
            .await?;
        let current: EnvironmentLease = serde_json::from_value(fresh.call(
            "environment.lease.acquire",
            json!({"run_id":format!("{run}-after-boot"),"owner_id":"fault-controller-fixture"}),
        ).await?)?;
        ensure!(
            current.epoch > old_run.epoch,
            "new run reused its predecessor fence"
        );
        let rebound = fresh.with_lease(current.clone());
        let initial = inventory(&rebound).await?;
        ensure!(
            initial == json!({"jobs":[],"resources":[]}),
            "new boot run inherited resources"
        );
        let denied = rebound
            .call("job.start", request.clone())
            .await
            .err()
            .context("new boot reused input without new native admission")?;
        ensure!(
            denied.downcast_ref::<WorkerError>().is_some(),
            "old input failed outside worker admission: {denied:#}"
        );
        let completed = echo_after_recovery(&rebound, &bytes, &profile.probe.sha256).await?;
        let discovered = inventory(&rebound).await?;
        ensure!(
            discovered["jobs"] == json!([completed])
                && discovered["resources"][0]["phase"] == "closed",
            "post-boot native result inventory differs"
        );
        fresh
            .call("environment.lease.release", json!({"lease":current}))
            .await?;
        fresh.close().await?;
        publish(
            &profile.output,
            &json!({"passed":true,"scope":"interrupted_native_os_boot_vm","fault":profile.fault,"before":before,"after":after,"first_epoch":lease.epoch,"last_epoch":current.epoch,"original_input":uploaded,"launch":launch,"request":request,"started":started,"old_run_discovery_rejected":true,"old_resource_lookup_rejected":true,"old_start_binding_rejected":true,"old_input_reuse_rejected":true,"after_recovery":completed,"inventory":discovered,"production_execution":false}),
        )?;
        return Ok(());
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
    let recovered_inventory = if profile.resource_inventory {
        let value = inventory(&rebound).await?;
        ensure!(
            value["jobs"] == json!([status]) && value["resources"][0]["phase"] == "closed",
            "interrupted native resource is absent or reopened in discovery"
        );
        value
    } else {
        Value::Null
    };
    ensure!(
        status["result"]["outcome"] == "unknown" && status["result"].get("exit_code").is_none(),
        "interrupted execution manufactured a result"
    );
    let cached = rebound.call("job.start", request.clone()).await?;
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
    let after = echo_after_recovery(&rebound, &bytes, &profile.probe.sha256).await?;
    active
        .call("environment.lease.release", json!({"lease":current}))
        .await?;
    active.close().await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":true,"scope":"interrupted_native_process_vm","fault":profile.fault,"identity":active.identity(),"first_epoch":lease.epoch,"last_epoch":current.epoch,"original_input":uploaded,"launch":launch,"request":request,"started":started,"reconciled":status,"inventory":recovered_inventory,"recovery_receipt":proof,"recovery_artifact":original,"after_recovery":after,"production_execution":false}),
    )?;
    Ok(())
}
