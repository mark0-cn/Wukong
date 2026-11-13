//! The independent VM operator observes the exact owned resource before any
//! fault. No PID or service mutation is exposed through a controller tool.
use super::{
    debugger_acceptance::{Architecture, Calls, launch_fixture},
    launch_acceptance::{bytes, original},
    process_fault_acceptance::{echo_after_recovery, publish, wait_for_resume},
    tests::{enrolled_fixture_address, read_fixture_profile},
};
use crate::runtime::{
    RuntimeClient, RuntimeConfig,
    protocol::{EnvironmentLease, WorkerError, WorkerErrorKind},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Fault {
    Cancel,
    Host,
    ServiceStop,
    ServiceKill,
}
impl Fault {
    fn restarts_service(self) -> bool {
        matches!(self, Self::ServiceStop | Self::ServiceKill)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    config: RuntimeConfig,
    manifest_sha256: String,
    probe: Architecture,
    fault: Fault,
    ready: PathBuf,
    resume: PathBuf,
    output: PathBuf,
}

async fn enter_wait(calls: &mut Calls<'_>, id: &str) -> Result<()> {
    let breakpoint = calls
        .control(
            id,
            "breakpoint_add",
            json!({"symbol":"wukong_probe!fixture_debug_wait"}),
        )
        .await?;
    let bp = &breakpoint["response"]["structuredContent"]["breakpoint"];
    let address = bp["address"]
        .as_str()
        .context("trusted wait symbol unresolved")?;
    let number = bp["id"]
        .as_u64()
        .context("trusted wait breakpoint missing")?
        .to_string();
    let mut hit = false;
    for _ in 0..3 {
        calls
            .control(id, "continue", json!({"timeout_secs":3}))
            .await?;
        let stop = calls.stopped(id).await?;
        ensure!(
            stop["stop"]["target_gone"] == false,
            "trusted fixture exited before fault"
        );
        if stop["stop"]["stopped_at"] == address {
            hit = true;
            break;
        }
    }
    ensure!(hit, "trusted fixture never reached its wait");
    calls
        .control(id, "breakpoint_remove", json!({"breakpoint_id":number}))
        .await?;
    Ok(())
}

async fn terminal(client: &RuntimeClient, id: &str) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        let listing = client.call("resource.list", json!({"limit":64})).await?;
        let records = listing["resources"]
            .as_array()
            .context("fault resource inventory absent")?;
        let record = records
            .iter()
            .find(|row| row["resource_id"] == id)
            .context("original fault resource absent")?;
        if record["phase"] == "closed" {
            return client
                .call("debug.inspect", json!({"session_id":id,"kind":"state"}))
                .await;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "interrupted debugger cleanup did not settle"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "requires an enrolled dedicated Windows VM, signed probe/PDB and independently coordinated fault"]
async fn remote_native_interrupted_debugger_acceptance() -> Result<()> {
    let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_DEBUGGER_FAULT_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    for path in [&profile.ready, &profile.resume, &profile.output] {
        ensure!(
            path.is_absolute() && !path.try_exists()?,
            "fault paths must be new absolute paths"
        );
    }
    let probe_bytes = bytes(&profile.probe.program)?;
    let client = RuntimeClient::connect(profile.config.clone()).await?;
    ensure!(
        client.identity().package_sha256 == profile.manifest_sha256,
        "fault package differs"
    );
    let run = format!(
        "debugger-fault-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let acquire = json!({"run_id":run,"owner_id":"debugger-fault-controller"});
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call("environment.lease.acquire", acquire.clone())
            .await?,
    )?;
    lease.validate(&run, "debugger-fault-controller", client.identity())?;
    let bound = client.with_lease(lease.clone());
    // RuntimeClient serializes one transport. Cancellation needs a second
    // authenticated connection carrying the same already admitted lease.
    let canceller = if profile.fault == Fault::Cancel {
        let connection = RuntimeClient::connect(profile.config.clone()).await?;
        ensure!(
            connection.identity() == client.identity(),
            "cancel connection identity differs"
        );
        Some(connection.with_lease(lease.clone()))
    } else {
        None
    };
    let launch = launch_fixture(
        &bound,
        &format!("debugger-fault/{}", profile.probe.program.sha256),
        &profile.probe.program,
        &profile.probe.pdb,
        120,
        &["--debug-wait"],
    )
    .await?;
    let mut calls = Calls {
        client: &bound,
        log: fs::File::create_new(profile.output.with_extension("jsonl"))?,
        count: 0,
    };
    let request = json!({"mode":"launch","engine":"dbgeng","launch_spec":launch["path"],"request_id":"fault-start"});
    let started = calls.call("debug.start", request.clone()).await?;
    let id = started["session_id"]
        .as_str()
        .context("fault debugger absent")?;
    if let Err(error) = enter_wait(&mut calls, id).await {
        calls
            .call(
                "debug.close",
                json!({"session_id":id,"request_id":"setup-failed-close"}),
            )
            .await?;
        return Err(error);
    }
    // Step over the trusted function's short prologue, then its 120-second
    // Sleep call. This uses the upstream typed tool and no instruction parser.
    // Each completed preparation is retained; only an independently observed
    // still-started mutation authorizes a fault.
    let mut preparation = Vec::new();
    let (control, pending) = loop {
        let control = json!({"session_id":id,"action":"next","timeout_secs":60,
            "request_id":format!("fault-step-{}",preparation.len())});
        let mut pending = Box::pin(bound.call("debug.control", control.clone()));
        let Ok(result) = tokio::time::timeout(Duration::from_millis(600), &mut pending).await
        else {
            break (control, pending);
        };
        drop(pending);
        let accepted = result
            .as_ref()
            .is_ok_and(|value| value["response"]["isError"] != true);
        if !accepted || preparation.len() >= 16 {
            calls
                .call(
                    "debug.close",
                    json!({"session_id":id,"request_id":"early-control-close"}),
                )
                .await?;
            anyhow::bail!("trusted wait was not reached: {result:?}");
        }
        preparation.push(json!({"params":control,"result":result?}));
    };
    publish(
        &profile.ready,
        &json!({"scope":"dedicated_native_debugger_fault_ready","fault":profile.fault,
        "identity":client.identity(),"run_id":run,"owner_id":"debugger-fault-controller",
        "started":started,"launch":launch,"preparation":preparation,"pending_control":control,"probe_sha256":profile.probe.program.sha256}),
    )?;
    wait_for_resume(&profile.resume, Duration::from_secs(180)).await?;
    let cancellation = if profile.fault == Fault::Cancel {
        let error = canceller
            .as_ref()
            .context("cancel transport absent")?
            .call(
                "debug.close",
                json!({"session_id":id,"request_id":"cancel-busy-debugger"}),
            )
            .await
            .err()
            .context("busy close fabricated a completed cleanup")?;
        canceller.as_ref().unwrap().close().await?;
        ensure!(
            error.to_string().contains("cancellation requested"),
            "busy close failed for another reason: {error:#}"
        );
        Some(format!("{error:#}"))
    } else {
        None
    };
    let failed = tokio::time::timeout(Duration::from_secs(45), pending)
        .await
        .context("interrupted control did not return")?
        .err()
        .context("interrupted control manufactured success")?;
    let active = if profile.fault.restarts_service() {
        let fresh = RuntimeClient::connect(profile.config.clone()).await?;
        ensure!(
            fresh.identity() == client.identity(),
            "service fault changed installation/OS identity"
        );
        fresh
    } else {
        client.clone()
    };
    let current: EnvironmentLease =
        serde_json::from_value(active.call("environment.lease.acquire", acquire).await?)?;
    ensure!(
        if profile.fault.restarts_service() {
            current.epoch > lease.epoch
        } else {
            current.epoch == lease.epoch
        },
        "fault lease epoch differs"
    );
    if profile.fault.restarts_service() {
        let stale = bound
            .call("debug.start", request.clone())
            .await
            .err()
            .context("old service fence admitted work")?;
        ensure!(
            stale
                .downcast_ref::<WorkerError>()
                .is_some_and(|error| error.kind == WorkerErrorKind::LeaseLost),
            "old service fence failed for an unrelated reason: {stale:#}"
        );
    }
    let rebound = active.with_lease(current.clone());
    let status = terminal(&rebound, id).await?;
    ensure!(
        status["state"] == "lost"
            && status["cleanup_pending"] == false
            && status["result"]["outcome"] == "unknown"
            && status["native_owner"] == started["native_owner"],
        "interrupted debugger did not retain its original unknown outcome"
    );
    ensure!(
        rebound.call("debug.start", request).await.is_err(),
        "interrupted start replayed"
    );
    ensure!(
        rebound
            .call("debug.control", control.clone())
            .await
            .is_err(),
        "interrupted control replayed or fabricated completion"
    );
    let (artifact, content) = original(&rebound, &format!("native-results/{id}/receipt")).await?;
    let proof: Value = serde_json::from_slice(&content)?;
    ensure!(
        artifact["sha256"] == status["result"]["recovery_receipt_sha256"]
            && proof["resource_id"] == id
            && proof["owner"] == started["native_owner"]
            && proof["scope"] == "owned_resource_recovery"
            && proof["jobs_empty"] == true
            && proof["profiles_removed"] == true
            && proof["scratch_removed"] == true,
        "interrupted debugger recovery receipt differs"
    );
    let after = echo_after_recovery(&rebound, &probe_bytes, &profile.probe.program.sha256).await?;
    active
        .call("environment.lease.release", json!({"lease":current}))
        .await?;
    active.close().await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":true,"scope":"interrupted_native_debugger_vm","fault":profile.fault,
        "identity":active.identity(),"first_epoch":lease.epoch,"last_epoch":current.epoch,"started":started,
        "preparation":preparation,"pending_control":control,
        "interrupted_control_error":format!("{failed:#}"),"cancellation":cancellation,"terminal":status,
        "recovery_artifact":artifact,"recovery_receipt":proof,"after_recovery":after,"production_execution":false}),
    )?;
    Ok(())
}
