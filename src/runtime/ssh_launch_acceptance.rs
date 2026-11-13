//! Real controller acceptance for a signed, disconnected native Windows worker.
use super::{
    process_acceptance::{Probe, terminal},
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
    io::{Read, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Architecture {
    program: Probe,
    dependency: Probe,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    config: RuntimeConfig,
    manifest_sha256: String,
    probes: BTreeMap<String, Architecture>,
    output: PathBuf,
}
fn bytes(probe: &Probe) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(&probe.path)?
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 32 * 1024 * 1024 && hex::encode(Sha256::digest(&bytes)) == probe.sha256,
        "launch fixture bytes differ"
    );
    Ok(bytes)
}
async fn upload(
    client: &RuntimeClient,
    path: &str,
    bytes: &[u8],
    executable: bool,
) -> Result<Value> {
    let hash = hex::encode(Sha256::digest(bytes));
    let value=client.call("file.put",json!({"path":path,"data_base64":STANDARD.encode(bytes),"executable":executable,"request_id":format!("launch-put-{}",hex::encode(Sha256::digest(format!("{path}:{hash}:{executable}"))))})).await?;
    ensure!(
        value["sha256"] == hash
            && value["native_input"]["identity"] == serde_json::to_value(client.identity())?,
        "launch upload lacks current native admission"
    );
    Ok(value)
}
async fn original(client: &RuntimeClient, path: &str) -> Result<(Value, Vec<u8>)> {
    let value = client.call("file.get", json!({"path":path})).await?;
    let bytes = STANDARD.decode(
        value["data_base64"]
            .as_str()
            .context("native artifact bytes missing")?,
    )?;
    ensure!(
        value["sha256"] == hex::encode(Sha256::digest(&bytes)),
        "native artifact digest differs"
    );
    Ok((value, bytes))
}

#[tokio::test]
#[ignore = "requires the independently enrolled offline Windows VM and pinned x64/x86 LaunchSpec fixtures"]
async fn remote_installed_windows_worker_launch_acceptance() -> Result<()> {
    let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_LAUNCH_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    ensure!(
        profile.output.is_absolute() && !profile.output.try_exists()?,
        "launch report must be a new absolute path"
    );
    ensure!(
        profile
            .probes
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            == ["x64", "x86"],
        "both architectures are required"
    );
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        client.identity().package_sha256 == profile.manifest_sha256,
        "launch fixture package differs"
    );
    let run = format!(
        "native-launch-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let acquire = json!({"run_id":run,"owner_id":"launch-controller-fixture"});
    let mut lease: EnvironmentLease = serde_json::from_value(
        client
            .call("environment.lease.acquire", acquire.clone())
            .await?,
    )?;
    lease.validate(&run, "launch-controller-fixture", client.identity())?;
    let data = b"launch\x00\xff\r\n";
    let peer = b"wukong-peer\n";
    let data_hash = hex::encode(Sha256::digest(data));
    let peer_hash = hex::encode(Sha256::digest(peer));
    let mut cases = Vec::new();
    for (architecture, artifacts) in profile.probes {
        let bound = client.with_lease(lease.clone());
        let base = format!("launch/{}", artifacts.program.sha256);
        let program = upload(
            &bound,
            &format!("{base}/bin/probe.exe"),
            &bytes(&artifacts.program)?,
            true,
        )
        .await?;
        let dependency = upload(
            &bound,
            &format!("{base}/bin/wukong-fixture.dll"),
            &bytes(&artifacts.dependency)?,
            true,
        )
        .await?;
        upload(&bound, &format!("{base}/data/peer.bin"), peer, false).await?;
        let input = upload(
            &bound,
            &format!("launch-input/{data_hash}.bin"),
            data,
            false,
        )
        .await?;
        let raw_request = json!({"argv":[program["path"],"--copy-stdin"],"input":input["path"],"timeout_secs":5,"request_id":format!("raw-stdin-{architecture}")});
        let started = bound.call("job.start", raw_request.clone()).await?;
        let raw_id = started["job_id"]
            .as_str()
            .context("raw stdin job is absent")?;
        let raw_status = terminal(&bound, raw_id).await?;
        let (_, raw_bytes) = original(&bound, &format!("native-results/{raw_id}/stdout")).await?;
        ensure!(
            raw_status["state"] == "completed"
                && raw_status["result"]["stdin_written"] == data.len()
                && raw_status["result"]["stdin_complete"] == true
                && raw_bytes == data,
            "raw native stdin delivery differs"
        );
        for mode in ["stdin_pipe", "argv_file"] {
            let args = if mode == "stdin_pipe" {
                json!(["--launch-stdin", "../data/peer.bin"])
            } else {
                json!(["--launch-argv", "../data/peer.bin", "@@"])
            };
            let manifest = json!({"schema_version":1,"challenge_id":"trusted-native-launch","base":format!("files/{base}"),"target":"bin/probe.exe","files":{"bin/probe.exe":artifacts.program.sha256,"bin/wukong-fixture.dll":artifacts.dependency.sha256,"data/peer.bin":peer_hash},"args":args,"cwd":"work","input_mode":mode,"env":{},"timeout_secs":5,"service":{"mode":"offline"},"loader":null,"library_path":null,"runtime_confirmed":false});
            let mut changed = manifest.clone();
            changed["files"]["bin/probe.exe"] = json!("0".repeat(64));
            let changed = upload(
                &bound,
                &format!("{base}/{mode}-changed.json"),
                &serde_json::to_vec(&changed)?,
                false,
            )
            .await?;
            ensure!(bound.call("job.start",json!({"launch_spec":changed["path"],"input":input["path"],"request_id":format!("changed-{architecture}-{mode}")})).await.is_err(),"changed LaunchSpec identity reached execution");
            let source = upload(
                &bound,
                &format!("{base}/{mode}.json"),
                &serde_json::to_vec(&manifest)?,
                false,
            )
            .await?;
            ensure!(bound.call("job.start",json!({"launch_spec":source["path"],"input":input["path"],"argv":[program["path"]],"request_id":format!("override-{architecture}-{mode}")})).await.is_err(),"per-call argv replaced a LaunchSpec");
            let request = json!({"launch_spec":source["path"],"input":input["path"],"request_id":format!("launch-{architecture}-{mode}")});
            let started = bound.call("job.start", request.clone()).await?;
            let id = started["job_id"].as_str().context("launch job is absent")?;
            let status = terminal(&bound, id).await?;
            ensure!(
                status["state"] == "completed",
                "contained launch failed: {status}"
            );
            let (output, output_bytes) =
                original(&bound, &format!("native-results/{id}/stdout")).await?;
            let (receipt, receipt_bytes) =
                original(&bound, &format!("native-results/{id}/receipt")).await?;
            let proof: Value = serde_json::from_slice(&receipt_bytes)?;
            ensure!(
                output_bytes == data
                    && proof["launch"]["manifest"]["sha256"] == source["sha256"]
                    && proof["launch"]["input_mode"] == mode
                    && proof["launch"]["cwd"] == format!("files/{base}/work")
                    && proof["input"]["sha256"] == data_hash
                    && proof["launch"]["files"]
                        .as_array()
                        .is_some_and(|files| files.len() == 2)
                    && proof["request_sha256"]
                        .as_str()
                        .is_some_and(|hash| hash.len() == 64)
                    && proof["jobs_empty"] == true
                    && proof["profiles_removed"] == true
                    && proof["scratch_removed"] == true,
                "native launch identity, delivery or cleanup differs"
            );
            ensure!(
                bound.call("job.start", request.clone()).await? == status,
                "completed LaunchSpec was replayed"
            );
            cases.push(json!({"architecture":architecture,"mode":mode,"manifest":source,"dependency":dependency,"request":request,"status":status,"output":output,"receipt":receipt}));
        }
        let renewed: EnvironmentLease = serde_json::from_value(
            client
                .call("environment.lease.acquire", acquire.clone())
                .await?,
        )?;
        renewed.validate(&run, "launch-controller-fixture", client.identity())?;
        ensure!(
            renewed.epoch == lease.epoch && renewed.token == lease.token,
            "launch suite renewal changed its fence"
        );
        lease = renewed;
    }
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    fs::File::create_new(profile.output)?.write_all(&serde_json::to_vec_pretty(&json!({"passed":true,"scope":"native_launch_spec_vm","identity":client.identity(),"cases":cases,"production_execution":false}))?)?;
    Ok(())
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
#[ignore = "requires completed LaunchSpec acceptance and an independently observed VM service restart"]
async fn remote_native_launch_recovery_acceptance() -> Result<()> {
    let profile: RecoveryProfile =
        read_fixture_profile("WUKONG_REMOTE_WORKER_LAUNCH_RECOVERY_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    let bytes = fs::read(profile.previous)?;
    ensure!(
        bytes.len() <= 512 * 1024 && hex::encode(Sha256::digest(&bytes)) == profile.previous_sha256,
        "prior launch report differs"
    );
    let previous: Value = serde_json::from_slice(&bytes)?;
    ensure!(
        previous["passed"] == true && previous["scope"] == "native_launch_spec_vm",
        "invalid prior launch report"
    );
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        serde_json::to_value(client.identity())? == previous["identity"],
        "launch restart identity changed"
    );
    let cases = previous["cases"]
        .as_array()
        .context("prior launch cases missing")?;
    ensure!(cases.len() == 4, "prior launch coverage differs");
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
        Some(lease.epoch) > owner["epoch"].as_u64(),
        "launch recovery did not advance the lease"
    );
    let bound = client.with_lease(lease.clone());
    for case in cases {
        ensure!(
            bound.call("job.start", case["request"].clone()).await? == case["status"],
            "restart replayed or changed the LaunchSpec"
        );
        let (receipt, _) = original(
            &bound,
            case["receipt"]["path"]
                .as_str()
                .context("prior receipt path missing")?,
        )
        .await?;
        ensure!(
            receipt == case["receipt"],
            "restart replaced launch receipt bytes"
        );
    }
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    fs::File::create_new(profile.output)?.write_all(&serde_json::to_vec_pretty(&json!({"passed":true,"scope":"native_launch_after_observed_restart","prior_sha256":profile.previous_sha256,"epoch":lease.epoch,"identity":client.identity(),"cases":cases.len(),"production_execution":false}))?)?;
    Ok(())
}
