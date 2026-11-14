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
