//! Installed replay acceptance uses the existing trusted probe and raw artifacts.
use super::{
    debugger_acceptance::Calls,
    inventory_acceptance::inventory,
    launch_acceptance::{bytes, original, upload},
    process_acceptance::{Probe, terminal},
    process_fault_acceptance::publish,
    tests::{enrolled_fixture_address, read_fixture_profile},
};
use crate::runtime::{RuntimeClient, RuntimeConfig, protocol::EnvironmentLease};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    config: RuntimeConfig,
    manifest_sha256: String,
    probes: BTreeMap<String, Probe>,
    output: PathBuf,
}

async fn evidence(
    client: &RuntimeClient,
    status: &Value,
    target: &Value,
    input: &Value,
) -> Result<Value> {
    let id = status["job_id"].as_str().context("replay job ID absent")?;
    let result = &status["result"];
    let rows = result["results"]
        .as_array()
        .context("replay observations absent")?;
    ensure!(
        result["trials"] == rows.len()
            && result["attempts"] == rows.len()
            && result["target"]["sha256"] == target["sha256"]
            && result["input"]["sha256"] == input["sha256"]
            && result["fresh_working_directory_per_trial"] == true,
        "replay identity/count differs"
    );
    let (artifact, raw) = original(client, &format!("native-results/{id}/receipt")).await?;
    let receipt: Value = serde_json::from_slice(&raw)?;
    ensure!(
        artifact["sha256"] == result["native_receipt_sha256"]
            && receipt["scope"] == "native_replay_host",
        "replay receipt hash/scope differs"
    );
    let mut nonces = BTreeSet::new();
    let mut logons = BTreeSet::new();
    let mut trials = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let path = row["receipt_path"]
            .as_str()
            .context("trial artifact absent")?;
        ensure!(
            path == format!("native-results/{id}/trial-{index}"),
            "trial artifact scope differs"
        );
        let (metadata, data) = original(client, path).await?;
        let proof: Value = serde_json::from_slice(&data)?;
        ensure!(
            metadata["sha256"] == row["receipt_sha256"]
                && metadata["sha256"] == receipt["trials"][index]["sha256"]
                && proof["resource_id"] == id
                && proof["owner"] == status["native_owner"]
                && proof["request_sha256"] == receipt["request_sha256"]
                && proof["trial"] == index
                && proof["program"]["sha256"] == target["sha256"]
                && row["input_sha256"] == input["sha256"]
                && proof["execution_started"] == true
                && proof["pid"].as_u64().is_some_and(|pid| pid > 0)
                && proof["jobs_empty"] == true
                && proof["profiles_removed"] == true
                && proof["scratch_removed"] == true,
            "original trial execution/cleanup binding differs"
        );
        ensure!(
            nonces.insert(
                proof["scratch_nonce"]
                    .as_str()
                    .context("trial nonce absent")?
                    .to_owned()
            ) && logons.insert(
                proof["logon_sid"]
                    .as_str()
                    .context("trial logon absent")?
                    .to_owned()
            ),
            "replay reused a prior trial directory or logon"
        );
        for stream in ["stdout", "stderr"] {
            let data = STANDARD.decode(
                proof[format!("{stream}_base64")]
                    .as_str()
                    .context("trial output absent")?,
            )?;
            ensure!(
                hex::encode(Sha256::digest(&data)) == proof[format!("{stream}_sha256")]
                    && proof[format!("{stream}_sha256")] == row[format!("{stream}_sha256")],
                "trial output hash differs"
            );
        }
        trials.push(json!({"artifact":metadata,"receipt":proof}));
    }
    let artifacts = client.call("job.artifacts", json!({"job_id":id})).await?;
    ensure!(
        artifacts
            .as_array()
            .is_some_and(|values| values.len() == rows.len() + 1),
        "trial artifact inventory differs"
    );
    Ok(json!({"artifact":artifact,"receipt":receipt,"trials":trials}))
}

#[tokio::test]
#[ignore = "requires an independently enrolled offline Windows VM with native replay admission"]
async fn remote_installed_windows_worker_replay_acceptance() -> Result<()> {
    let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_REPLAY_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    ensure!(
        profile.output.is_absolute()
            && !profile.output.try_exists()?
            && profile
                .probes
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                == ["x64", "x86"],
        "replay profile differs"
    );
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        client.identity().package_sha256 == profile.manifest_sha256,
        "replay package differs"
    );
    ensure!(
        client
            .call(
                "poc.verify",
                json!({"target":"files/missing","input":"files/missing","request_id":"unleased"})
            )
            .await
            .is_err(),
        "unleased verification was admitted"
    );
    let run = format!(
        "native-replay-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let acquire = json!({"run_id":run,"owner_id":"replay-controller-fixture"});
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call("environment.lease.acquire", acquire.clone())
            .await?,
    )?;
    lease.validate(&run, "replay-controller-fixture", client.identity())?;
    let bound = client.with_lease(lease.clone());
    let mut calls = Calls {
        client: &bound,
        log: fs::File::create_new(profile.output.with_extension("jsonl"))?,
        count: 0,
    };
    let mut cases = Vec::new();
    for (architecture, probe) in &profile.probes {
        let base = format!("replay-fixture/{architecture}");
        let target = upload(&bound, &format!("{base}/target.exe"), &bytes(probe)?, true).await?;
        let crash = upload(
            &bound,
            &format!("{base}/crash.bin"),
            b"CRASH: native owned replay",
            false,
        )
        .await?;
        let safe = upload(
            &bound,
            &format!("{base}/safe.bin"),
            b"SAFE: replay-pipe\0\xff",
            false,
        )
        .await?;
        let launch = json!({"schema_version":1,"base":format!("files/{base}"),"target":"target.exe",
            "files":{"target.exe":probe.sha256},"args":["--replay","@@"],"cwd":"work/nested","input_mode":"argv_file",
            "env":{},"timeout_secs":5,"service":{"mode":"offline"}});
        let manifest = upload(
            &bound,
            &format!("{base}/launch.json"),
            &serde_json::to_vec(&launch)?,
            false,
        )
        .await?;
        for name in ["crash", "negative", "stdin", "deadline", "cancel", "launch"] {
            let renewed: EnvironmentLease = serde_json::from_value(
                client
                    .call("environment.lease.acquire", acquire.clone())
                    .await?,
            )?;
            ensure!(
                renewed.epoch == lease.epoch && renewed.token == lease.token,
                "replay lease changed during renewal"
            );
            let input = if matches!(name, "crash" | "launch") {
                &crash
            } else {
                &safe
            };
            let method = if matches!(name, "crash" | "stdin" | "launch") {
                "poc.verify"
            } else {
                "crash.replay"
            };
            let trials = if matches!(name, "crash" | "cancel" | "launch") {
                3
            } else if name == "deadline" {
                1
            } else {
                2
            };
            let id = format!("{architecture}-{name}");
            let mut request = json!({"target":target["path"],"input":input["path"],"args":["--replay","@@"],
                "trials":trials,"timeout_secs":5,"output_limit":4096,"producer_agent_id":"fixture-producer",
                "verifier_agent_id":"fixture-verifier","request_id":id,"call_id":id});
            if name == "stdin" {
                request["args"] = json!(["--copy-stdin"]);
                request["input_mode"] = json!("stdin");
                request["expected"] = json!({"exit_code":0,"stdout_contains":"replay-pipe"});
            }
            if matches!(name, "deadline" | "cancel") {
                request["args"] = json!(["@@"]);
                request["timeout_secs"] = json!(if name == "deadline" { 1 } else { 60 });
            }
            if name == "launch" {
                request.as_object_mut().unwrap().remove("args");
                request["launch_spec"] = manifest["path"].clone();
            }
            let started = calls.call(method, request.clone()).await?;
            let job = started["job_id"].as_str().context("replay job absent")?;
            if name == "cancel" {
                tokio::time::sleep(Duration::from_secs(5)).await;
                calls.call("job.cancel", json!({"job_id":job})).await?;
            }
            let status = terminal(&bound, job).await?;
            ensure!(
                status["state"]
                    == if name == "cancel" {
                        "cancelled"
                    } else {
                        "completed"
                    }
                    && status["kind"]
                        == if method == "poc.verify" {
                            "verify"
                        } else {
                            "replay"
                        }
                    && status["result"]["passed"] == matches!(name, "crash" | "stdin" | "launch"),
                "replay {name} outcome differs: {status}"
            );
            let proof = evidence(&bound, &status, &target, input).await?;
            let observations = status["result"]["results"].as_array().unwrap();
            ensure!(
                !observations.is_empty() && (name != "cancel" || observations.len() < 3),
                "cancellation did not stop remaining trials"
            );
            if matches!(name, "crash" | "launch") {
                ensure!(
                    observations.len() == 3
                        && observations.iter().all(|r| r["exit_code"] == 0xc0000005_u32
                            && r["crash_signature"] == "exception:0xc0000005"),
                    "native exception was not reproduced"
                );
            }
            if name == "deadline" {
                ensure!(
                    observations[0]["stop_reason"] == "deadline",
                    "deadline was not observed"
                );
            }
            if name == "stdin" {
                for trial in proof["trials"].as_array().unwrap() {
                    ensure!(
                        STANDARD.decode(trial["receipt"]["stdout_base64"].as_str().unwrap())?
                            == b"SAFE: replay-pipe\0\xff"
                            && trial["receipt"]["stdin_complete"] == true,
                        "binary replay stdin differs"
                    );
                }
            }
            ensure!(
                calls.call(method, request.clone()).await? == started
                    && terminal(&bound, job).await? == status,
                "completed replay was restarted"
            );
            let mut conflict = request.clone();
            conflict["trials"] = json!(4);
            ensure!(
                calls.call(method, conflict).await.is_err(),
                "replay request ID accepted changed trials"
            );
            cases.push(json!({"architecture":architecture,"case":name,"method":method,"request":request,"started":started,"status":status,"evidence":proof}));
        }
    }
    let discovered = inventory(&bound).await?;
    ensure!(
        discovered["jobs"].as_array().is_some_and(|r| r.len() == 12)
            && discovered["resources"]
                .as_array()
                .is_some_and(|r| r.len() == 12 && r.iter().all(|v| v["phase"] == "closed")),
        "replay inventory or cleanup differs"
    );
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":true,"scope":"native_replay_controller_vm","identity":client.identity(),
        "epoch":lease.epoch,"calls":calls.count,"cases":cases,"inventory":discovered,"production_execution":false}),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recovery {
    config: RuntimeConfig,
    previous: Probe,
    output: PathBuf,
}
#[tokio::test]
#[ignore = "requires completed replay acceptance and an independently observed dedicated VM service restart"]
async fn remote_native_completed_replay_recovery_acceptance() -> Result<()> {
    let profile: Recovery = read_fixture_profile("WUKONG_REMOTE_WORKER_REPLAY_RECOVERY_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    ensure!(
        profile.output.is_absolute() && !profile.output.try_exists()?,
        "new replay recovery report required"
    );
    let prior: Value = serde_json::from_slice(&bytes(&profile.previous)?)?;
    ensure!(
        prior["passed"] == true && prior["scope"] == "native_replay_controller_vm",
        "prior replay proof differs"
    );
    let cases = prior["cases"].as_array().context("replay cases absent")?;
    ensure!(cases.len() == 12, "prior replay coverage differs");
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        serde_json::to_value(client.identity())? == prior["identity"],
        "replay restart changed worker binding"
    );
    let owner = &cases[0]["status"]["native_owner"];
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call(
                "environment.lease.acquire",
                json!({"run_id":owner["run_id"],"owner_id":owner["owner_id"]}),
            )
            .await?,
    )?;
    ensure!(
        Some(lease.epoch) > prior["epoch"].as_u64(),
        "replay restart did not advance the fence"
    );
    let bound = client.with_lease(lease.clone());
    for case in cases {
        ensure!(
            bound
                .call(case["method"].as_str().unwrap(), case["request"].clone())
                .await?
                == case["started"]
                && terminal(&bound, case["started"]["job_id"].as_str().unwrap()).await?
                    == case["status"],
            "recovery replayed an original trial"
        );
        let proof = &case["evidence"];
        for artifact in std::iter::once(&proof["artifact"]).chain(
            proof["trials"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| &v["artifact"]),
        ) {
            let (original, _) = original(&bound, artifact["path"].as_str().unwrap()).await?;
            ensure!(
                original["sha256"] == artifact["sha256"],
                "original trial bytes changed after restart"
            );
        }
    }
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":true,"scope":"completed_native_replay_after_observed_service_restart",
        "prior_sha256":profile.previous.sha256,"identity":client.identity(),"epoch":lease.epoch,"cached_jobs":cases.len(),"production_execution":false}),
    )
}
