//! Opt-in controller acceptance of native interactive sessions on the dedicated VM.
use super::{
    debugger_acceptance::Calls,
    inventory_acceptance::inventory_kind,
    launch_acceptance::{bytes, original, upload},
    process_acceptance::Probe,
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
    collections::BTreeMap,
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

async fn send(
    calls: &mut Calls<'_>,
    id: &str,
    data: &[u8],
    timeout: u32,
) -> Result<(Value, Value)> {
    let request = json!({"session_id":id,"request_id":format!("write-{}",calls.count),"data_base64":STANDARD.encode(data),"read_timeout_secs":timeout});
    let value = calls.call("interact.write", request.clone()).await?;
    ensure!(
        calls.call("interact.write", request.clone()).await? == value,
        "interactive write retry changed its original result"
    );
    let mut changed = request.clone();
    changed["data_base64"] = json!(STANDARD.encode(b"conflicting write"));
    ensure!(
        calls.call("interact.write", changed).await.is_err(),
        "interactive request ID admitted different bytes"
    );
    Ok((request, value))
}

async fn closed(calls: &mut Calls<'_>, id: &str) -> Result<Value> {
    let value = calls
        .call(
            "interact.close",
            json!({"session_id":id,"request_id":format!("close-{}",calls.count)}),
        )
        .await?;
    ensure!(
        matches!(value["state"].as_str(), Some("closed" | "lost")),
        "interactive resource did not close"
    );
    Ok(value)
}

#[tokio::test]
#[ignore = "requires the independently enrolled offline Windows VM and signed x64/x86 interactive fixtures"]
async fn remote_installed_windows_worker_interactive_acceptance() -> Result<()> {
    let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_INTERACTIVE_TEST")?;
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
        "interactive fixture profile differs"
    );
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        client.identity().package_sha256 == profile.manifest_sha256,
        "interactive package differs"
    );
    ensure!(
        client
            .call(
                "interact.start",
                json!({"argv":["files/missing"],"request_id":"unleased"})
            )
            .await
            .is_err(),
        "unleased interactive start was admitted"
    );
    let run = format!(
        "native-interactive-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let acquire = json!({"run_id":run,"owner_id":"interactive-controller-fixture"});
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call("environment.lease.acquire", acquire.clone())
            .await?,
    )?;
    lease.validate(&run, "interactive-controller-fixture", client.identity())?;
    let bound = client.with_lease(lease.clone());
    let mut calls = Calls {
        client: &bound,
        log: fs::File::create_new(profile.output.with_extension("jsonl"))?,
        count: 0,
    };
    let mut cases = Vec::new();
    for (architecture, probe) in &profile.probes {
        let base = format!("interactive-fixture/{architecture}");
        let target = upload(&bound, &format!("{base}/target.exe"), &bytes(probe)?, true).await?;
        let launch = json!({"schema_version":1,"base":format!("files/{base}"),"target":"target.exe",
            "files":{"target.exe":probe.sha256},"args":["--interactive-once"],"cwd":"work/nested","input_mode":"stdin_pipe",
            "env":{},"timeout_secs":30,"service":{"mode":"offline"}});
        let manifest = upload(
            &bound,
            &format!("{base}/launch.json"),
            &serde_json::to_vec(&launch)?,
            false,
        )
        .await?;
        for name in ["duplex", "persistent", "natural", "blocked", "launch"] {
            let renewed: EnvironmentLease = serde_json::from_value(
                client
                    .call("environment.lease.acquire", acquire.clone())
                    .await?,
            )?;
            ensure!(
                renewed.epoch == lease.epoch && renewed.token == lease.token,
                "interactive lease changed during renewal"
            );
            let argument = match name {
                "duplex" | "launch" => "--interactive-once",
                "persistent" => "--copy-stdin",
                "natural" => "--echo",
                _ => "--debug-wait",
            };
            let mut start = json!({"argv":[target["path"],argument],"request_id":format!("{architecture}-{name}")});
            if name == "launch" {
                start["launch_spec"] = manifest["path"].clone();
            }
            let opened = calls.call("interact.start", start.clone()).await?;
            let id = opened["session_id"]
                .as_str()
                .context("interactive session ID absent")?;
            ensure!(
                opened["state"] == "running" && opened["kind"] == "interactive",
                "interactive start lifecycle differs"
            );
            ensure!(
                calls.call("interact.start", start.clone()).await? == opened,
                "live start was replayed"
            );
            let (first_transcript, first_bytes) = original(
                &bound,
                opened["transcript"]
                    .as_str()
                    .context("opening transcript absent")?,
            )
            .await?;
            let mut writes = Vec::new();
            let mut expected = Vec::new();
            if name == "blocked" {
                let request = json!({"session_id":id,"request_id":format!("blocked-{architecture}"),"data_base64":STANDARD.encode(vec![42;1024*1024]),"read_timeout_secs":1});
                ensure!(
                    calls.call("interact.write", request.clone()).await.is_err(),
                    "blocked input unexpectedly succeeded"
                );
                let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
                loop {
                    let listing = calls.call("resource.list", json!({"limit":64})).await?;
                    let row = listing["resources"]
                        .as_array()
                        .context("resource inventory absent")?
                        .iter()
                        .find(|v| v["resource_id"] == id)
                        .context("original interactive resource disappeared")?;
                    if row["phase"] == "closed" {
                        break;
                    }
                    ensure!(
                        tokio::time::Instant::now() < deadline,
                        "blocked input cleanup remained pending"
                    );
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                ensure!(
                    calls.call("interact.write", request).await.is_err(),
                    "uncertain write was replayed"
                );
            } else {
                let payload = if name == "persistent" {
                    b"first\0\xff\n".to_vec()
                } else if name == "natural" {
                    Vec::new()
                } else {
                    (0..65536).map(|i| (i % 256) as u8).collect()
                };
                let (request, mut result) = send(&mut calls, id, &payload, 10).await?;
                writes.push(json!({"request":request,"result":result}));
                if name == "persistent" {
                    ensure!(
                        result["state"] == "running"
                            && STANDARD.decode(
                                result["captured_base64"]
                                    .as_str()
                                    .context("binary output missing")?
                            )? == payload,
                        "first interactive output differs"
                    );
                    expected.extend_from_slice(&payload);
                    let payload = "second 中文\n".as_bytes();
                    let (request, value) = send(&mut calls, id, payload, 10).await?;
                    ensure!(
                        value["state"] == "running"
                            && STANDARD.decode(
                                value["captured_base64"]
                                    .as_str()
                                    .context("second output missing")?
                            )? == payload,
                        "second interactive output differs"
                    );
                    expected.extend_from_slice(payload);
                    writes.push(json!({"request":request,"result":value}));
                    let (request, value) = send(&mut calls, id, &[], 1).await?;
                    ensure!(
                        value["state"] == "running" && value["captured_bytes"] == 0,
                        "empty poll closed stdin or replayed input"
                    );
                    writes.push(json!({"request":request,"result":value}));
                } else {
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
                    while result["state"] == "running" {
                        ensure!(
                            tokio::time::Instant::now() < deadline,
                            "interactive natural exit was not observed"
                        );
                        let (request, value) = send(&mut calls, id, &[], 1).await?;
                        result = value;
                        writes.push(json!({"request":request,"result":result}));
                    }
                    ensure!(
                        result["state"] == "exited" && result["exit_code"] == 0,
                        "interactive exit or cleanup differs"
                    );
                    if name == "natural" {
                        expected.extend_from_slice(b"wukong-worker-selftest\n");
                    } else {
                        expected.resize(65536, 0);
                        expected.extend_from_slice(&payload);
                    }
                }
            }
            let terminal = closed(&mut calls, id).await?;
            let (receipt, raw) = original(&bound, &format!("native-results/{id}/receipt")).await?;
            let proof: Value = serde_json::from_slice(&raw)?;
            ensure!(
                proof["resource_id"] == id
                    && proof["owner"] == opened["native_owner"]
                    && proof["jobs_empty"] == true
                    && proof["profiles_removed"] == true
                    && proof["scratch_removed"] == true,
                "interactive cleanup proof differs"
            );
            if name == "blocked" {
                ensure!(
                    terminal["state"] == "lost"
                        && terminal["result"]["outcome"] == "unknown"
                        && proof["scope"] == "owned_resource_recovery",
                    "blocked interactive effect lost its uncertainty"
                );
            } else {
                ensure!(
                    proof["scope"] == "native_interactive_host"
                        && proof["program"]["sha256"] == probe.sha256
                        && proof["pid"] == opened["pid"],
                    "interactive receipt scope differs"
                );
                let (metadata, raw) =
                    original(&bound, &format!("native-results/{id}/stdout")).await?;
                ensure!(
                    raw == expected && metadata["sha256"] == proof["stdout_sha256"],
                    "interactive merged binary output differs or a write repeated"
                );
            }
            let (later, raw) = original(&bound, first_transcript["path"].as_str().unwrap()).await?;
            ensure!(
                raw == first_bytes && later["sha256"] == first_transcript["sha256"],
                "later writes changed opening transcript evidence"
            );
            ensure!(
                calls.call("interact.start", start.clone()).await.is_err(),
                "closed interactive start was replayed"
            );
            cases.push(json!({"architecture":architecture,"case":name,"start":start,"opened":opened,"writes":writes,
                "terminal":terminal,"receipt_sha256":receipt["sha256"],"proof":proof,"stdout_sha256":hex::encode(Sha256::digest(&expected))}));
        }
    }
    let discovered = inventory_kind(&bound, "interactive").await?;
    ensure!(
        discovered["jobs"].as_array().is_some_and(Vec::is_empty)
            && discovered["resources"]
                .as_array()
                .is_some_and(|rows| rows.len() == 10
                    && rows
                        .iter()
                        .all(|v| v["phase"] == "closed" && v["kind"] == "interactive")),
        "interactive resource inventory differs"
    );
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":true,"scope":"native_interactive_controller_vm","identity":client.identity(),
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
#[ignore = "requires an independently enrolled offline Windows VM and signed x64/x86 fixtures"]
async fn remote_native_interactive_profile_acceptance() -> Result<()> {
    let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_INTERACTIVE_PROFILE_TEST")?;
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
        "interactive profile fixture differs"
    );
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        client.identity().package_sha256 == profile.manifest_sha256,
        "profile package differs"
    );
    let run = format!(
        "native-profile-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let acquire = json!({"run_id":run,"owner_id":"profile-controller-fixture"});
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call("environment.lease.acquire", acquire.clone())
            .await?,
    )?;
    lease.validate(&run, "profile-controller-fixture", client.identity())?;
    let bound = client.with_lease(lease.clone());
    let mut calls = Calls {
        client: &bound,
        log: fs::File::create_new(profile.output.with_extension("jsonl"))?,
        count: 0,
    };
    let mut cases = Vec::new();
    for (architecture, probe) in &profile.probes {
        let target = upload(
            &bound,
            &format!("profile-fixture/{architecture}/target.exe"),
            &bytes(probe)?,
            true,
        )
        .await?;
        let opened = calls
            .call(
                "interact.start",
                json!({"argv":[target["path"],"--copy-stdin"],
            "request_id":format!("profile-start-{architecture}")}),
            )
            .await?;
        let id = opened["session_id"]
            .as_str()
            .context("profile session ID absent")?;
        // Wait for the trusted program's actual I/O, not an assumed loader delay.
        let payload = b"profile-before\0\xff\n";
        let (_, before) = send(&mut calls, id, payload, 10).await?;
        ensure!(
            STANDARD.decode(
                before["captured_base64"]
                    .as_str()
                    .context("initial bytes absent")?
            )? == payload,
            "profile fixture did not reach its input loop"
        );
        for extra in [
            json!({"pid":1}),
            json!({"path":"C:/Windows/System32/cmd.exe"}),
        ] {
            let mut bad = json!({"session_id":id});
            bad.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            ensure!(
                calls.call("interact.profile", bad).await.is_err(),
                "profile accepted a caller-selected target"
            );
        }
        let first = calls
            .call("interact.profile", json!({"session_id":id}))
            .await?;
        ensure!(
            first["state"] == "running"
                && first["process"]["status"] == "observed"
                && first["process"]["pid"] == opened["pid"]
                && first["process"]["created_filetime"]
                    .as_u64()
                    .is_some_and(|n| n > 0)
                && first["launch_identity"]["sha256"] == probe.sha256,
            "profile identity differs from the owned target"
        );
        let (artifact, raw) = original(
            &bound,
            first["transcript"]
                .as_str()
                .context("profile transcript missing")?,
        )
        .await?;
        let events: Vec<Value> = raw
            .split(|b| *b == b'\n')
            .filter(|b| !b.is_empty())
            .map(serde_json::from_slice)
            .collect::<std::result::Result<_, _>>()?;
        let observed = &events.last().context("profile original absent")?["process"];
        ensure!(
            observed["pid"] == opened["pid"]
                && observed["maps"].as_array().is_some_and(|m| !m.is_empty())
                && observed["modules"].as_array().is_some_and(|m| !m.is_empty()
                    && m.iter().any(|v| v["mapped_file"]["path"]
                        .as_str()
                        .is_some_and(|p| p.ends_with("target.exe")))
                    && m.iter().all(|v| v["sha256"].is_null()))
                && observed["protections"]["aslr"]["status"] == "observed"
                && observed["protections"]["cfg"]["status"] == "observed",
            "original profile lacks OS observations or invented mapped hashes"
        );
        let second = calls
            .call("interact.profile", json!({"session_id":id}))
            .await?;
        ensure!(
            first["transcript"] != second["transcript"]
                && first["process"]["created_filetime"] == second["process"]["created_filetime"],
            "fresh observation changed the owned process or reused stale evidence"
        );
        let payload = b"profile-after\0\xfe\n";
        let (_, after) = send(&mut calls, id, payload, 10).await?;
        ensure!(
            STANDARD.decode(
                after["captured_base64"]
                    .as_str()
                    .context("later bytes absent")?
            )? == payload,
            "profile disturbed interactive I/O"
        );
        let terminal = closed(&mut calls, id).await?;
        ensure!(
            calls
                .call("interact.profile", json!({"session_id":id}))
                .await
                .is_err(),
            "closed target admitted a new profile"
        );
        let (again, later) = original(&bound, artifact["path"].as_str().unwrap()).await?;
        ensure!(
            later == raw && again["sha256"] == artifact["sha256"],
            "profile original changed after closure"
        );
        cases.push(
            json!({"architecture":architecture,"opened":opened,"first":first,"second":second,
            "original":artifact,"terminal":terminal}),
        );
    }
    let discovered = inventory_kind(&bound, "interactive").await?;
    ensure!(
        discovered["resources"]
            .as_array()
            .is_some_and(|rows| rows.len() == 2 && rows.iter().all(|v| v["phase"] == "closed")),
        "profile resources did not close"
    );
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":true,"scope":"native_interactive_profile_controller_vm",
        "identity":client.identity(),"epoch":lease.epoch,"calls":calls.count,"cases":cases,
        "inventory":discovered,"production_execution":false}),
    )
}

#[tokio::test]
#[ignore = "requires completed interactive acceptance and an independently observed dedicated VM service restart"]
async fn remote_native_completed_interactive_recovery_acceptance() -> Result<()> {
    let profile: Recovery = read_fixture_profile("WUKONG_REMOTE_WORKER_INTERACTIVE_RECOVERY_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    ensure!(
        profile.output.is_absolute() && !profile.output.try_exists()?,
        "new interactive recovery report required"
    );
    let prior: Value = serde_json::from_slice(&bytes(&profile.previous)?)?;
    ensure!(
        prior["passed"] == true && prior["scope"] == "native_interactive_controller_vm",
        "prior interactive proof differs"
    );
    let cases = prior["cases"]
        .as_array()
        .context("interactive cases absent")?;
    ensure!(cases.len() == 10, "prior interactive coverage differs");
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        serde_json::to_value(client.identity())? == prior["identity"],
        "interactive restart changed worker binding"
    );
    let owner = &cases[0]["opened"]["native_owner"];
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
        "interactive restart did not advance the fence"
    );
    let bound = client.with_lease(lease.clone());
    let mut cached = 0;
    let mut uncertain = 0;
    for case in cases {
        let id = case["opened"]["session_id"]
            .as_str()
            .context("prior interactive ID absent")?;
        for write in case["writes"].as_array().context("prior writes absent")? {
            ensure!(
                bound
                    .call("interact.write", write["request"].clone())
                    .await?
                    == write["result"],
                "restart replayed a completed interactive write or changed its original result"
            );
            original(
                &bound,
                write["result"]["transcript"]
                    .as_str()
                    .context("write transcript absent")?,
            )
            .await?;
            cached += 1;
        }
        ensure!(
            bound
                .call("interact.start", case["start"].clone())
                .await
                .is_err(),
            "restart reopened a completed session"
        );
        let terminal = bound
            .call(
                "interact.close",
                json!({"session_id":id,"request_id":format!("reconcile-{id}")}),
            )
            .await?;
        let (artifact, raw) = original(&bound, &format!("native-results/{id}/receipt")).await?;
        ensure!(
            artifact["sha256"] == case["receipt_sha256"]
                && serde_json::from_slice::<Value>(&raw)? == case["proof"],
            "interactive original receipt changed after restart"
        );
        if case["case"] == "blocked" {
            ensure!(
                terminal["state"] == "lost" && terminal["result"]["outcome"] == "unknown",
                "interrupted write lost its unknown outcome"
            );
            let error=bound.call("interact.write",json!({"session_id":id,"request_id":format!("blocked-{}",case["architecture"].as_str().unwrap()),"data_base64":STANDARD.encode(vec![42;1024*1024]),"read_timeout_secs":1})).await.expect_err("uncertain write was replayed");
            ensure!(
                error.to_string().contains("unknown after cleanup"),
                "uncertain write failed for another reason: {error:#}"
            );
            uncertain += 1;
        } else {
            ensure!(
                terminal["state"] == "closed",
                "known interactive closure changed"
            );
            let (artifact, _) = original(&bound, &format!("native-results/{id}/stdout")).await?;
            ensure!(
                artifact["sha256"] == case["stdout_sha256"],
                "interactive original output changed after restart"
            );
        }
    }
    let discovered = inventory_kind(&bound, "interactive").await?;
    ensure!(
        discovered == prior["inventory"] && uncertain == 2,
        "restart created or changed interactive resources"
    );
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":true,"scope":"completed_native_interactive_after_observed_service_restart",
        "prior_sha256":profile.previous.sha256,"identity":client.identity(),"epoch":lease.epoch,
        "cached_writes":cached,"uncertain_writes":uncertain,"resources":cases.len(),"production_execution":false}),
    )
}
