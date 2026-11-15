//! Trusted CMD fixtures through the installed synchronous native process route.
use super::{
    debugger_acceptance::Calls,
    inventory_acceptance::inventory,
    launch_acceptance::{bytes, original, upload},
    process_acceptance::Probe,
    process_fault_acceptance::publish,
    tests::{enrolled_fixture_address, read_fixture_profile},
};
use crate::runtime::{RuntimeClient, RuntimeConfig, protocol::EnvironmentLease};
use anyhow::{Context, Result, ensure};
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recovery {
    config: RuntimeConfig,
    previous: Probe,
    output: PathBuf,
}

#[tokio::test]
#[ignore = "requires completed shell acceptance and an independently observed dedicated VM service restart"]
async fn remote_native_completed_shell_recovery_acceptance() -> Result<()> {
    let profile: Recovery = read_fixture_profile("WUKONG_REMOTE_WORKER_SHELL_RECOVERY_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    ensure!(
        profile.output.is_absolute() && !profile.output.try_exists()?,
        "new absolute shell recovery report required"
    );
    let prior: Value = serde_json::from_slice(&bytes(&profile.previous)?)?;
    ensure!(
        prior["passed"] == true && prior["scope"] == "native_shell_controller_vm",
        "prior shell qualification differs"
    );
    let cases = prior["cases"]
        .as_array()
        .context("prior shell cases absent")?;
    ensure!(cases.len() == 8, "prior shell coverage differs");
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        serde_json::to_value(client.identity())? == prior["identity"],
        "shell restart changed worker binding"
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
        "shell restart did not advance its lease"
    );
    let bound = client.with_lease(lease.clone());
    let mut cached = Vec::new();
    for case in cases {
        let status = bound.call("exec.run", case["request"].clone()).await?;
        ensure!(
            status == case["status"],
            "completed shell execution was repeated or changed"
        );
        let (artifact, raw) = original(
            &bound,
            case["artifact"]["path"]
                .as_str()
                .context("prior shell receipt path absent")?,
        )
        .await?;
        ensure!(
            artifact == case["artifact"]
                && serde_json::from_slice::<Value>(&raw)? == case["receipt"],
            "original shell receipt changed after restart"
        );
        for stream in ["stdout", "stderr"] {
            let prior_stream = &case["streams"][stream];
            let (artifact, data) = original(
                &bound,
                prior_stream["artifact"]["path"]
                    .as_str()
                    .context("prior shell stream path absent")?,
            )
            .await?;
            ensure!(
                artifact == prior_stream["artifact"]
                    && String::from_utf8_lossy(&data)
                        == prior_stream["text"]
                            .as_str()
                            .context("prior shell text absent")?,
                "shell output changed after restart"
            );
        }
        cached.push(status);
    }
    let inventory = inventory(&bound).await?;
    ensure!(
        inventory == prior["inventory"],
        "restart changed shell resource inventory"
    );
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":true,"scope":"completed_native_shell_after_observed_service_restart","prior_sha256":profile.previous.sha256,"identity":client.identity(),"epoch":lease.epoch,"cached":cached,"inventory":inventory,"production_execution":false}),
    )?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a dedicated enrolled Windows VM with native shell routing and pinned trusted probes"]
async fn remote_installed_windows_worker_shell_acceptance() -> Result<()> {
    let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_SHELL_TEST")?;
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
        "shell fixture profile differs"
    );
    let client = RuntimeClient::connect(profile.config.clone()).await?;
    // RuntimeClient serializes each transport. Observe pending execution and
    // renew its lease through a genuinely independent controller connection.
    let observer = RuntimeClient::connect(profile.config).await?;
    ensure!(
        observer.identity() == client.identity(),
        "shell observer binding differs"
    );
    ensure!(
        client.identity().package_sha256 == profile.manifest_sha256,
        "shell package differs"
    );
    super::inventory_acceptance::qualified(&client).await?;
    ensure!(
        client
            .call(
                "exec.run",
                json!({"command":"echo unleased","request_id":"unleased-shell"})
            )
            .await
            .is_err(),
        "unleased shell command admitted"
    );
    let run = format!(
        "native-shell-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let acquire = json!({"run_id":run,"owner_id":"shell-controller-fixture"});
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call("environment.lease.acquire", acquire.clone())
            .await?,
    )?;
    lease.validate(&run, "shell-controller-fixture", client.identity())?;
    let bound = client.with_lease(lease.clone());
    let observed = observer.with_lease(lease.clone());
    let mut requests = vec![
        (
            "builtin".to_owned(),
            json!({"command":"cd&&echo %AFL_TEST%&&echo %WUKONG_JOB_DIR%&&echo %WUKONG_STATE_DIR%&&echo created>marker.txt&&type marker.txt","cwd":"work/space directory","env":{"AFL_TEST":"native-environment"}}),
            "completed",
        ),
        (
            "quotes".to_owned(),
            json!({"command":"echo literal ^& value&&echo \"quoted argument\"&&echo native-error 1>&2"}),
            "completed",
        ),
        (
            "nonzero".to_owned(),
            json!({"command":"exit /b 7"}),
            "failed",
        ),
        (
            "output".to_owned(),
            json!({"command":"for /l %i in (1,1,4096) do @echo native-output","output_limit":1024}),
            "failed",
        ),
    ];
    for (architecture, probe) in &profile.probes {
        let target = upload(
            &bound,
            &format!("shell-fixture/{architecture}/target.exe"),
            &bytes(probe)?,
            true,
        )
        .await?;
        requests.push((format!("pipe-{architecture}"),json!({"command":"\"%TARGET%\" --echo&&echo shell-payload|\"%TARGET%\" --copy-stdin","target":target["path"],"cwd":"nested"}),"completed"));
        requests.push((format!("deadline-{architecture}"),json!({"command":"echo waiting&&\"%TARGET%\" --debug-wait","target":target["path"],"timeout_secs":3}),"timed_out"));
    }
    let mut calls = Calls {
        client: &bound,
        log: fs::File::create_new(profile.output.with_extension("jsonl"))?,
        count: 0,
    };
    let mut cases = Vec::new();
    for (name, mut request, expected) in requests {
        request["request_id"] = json!(format!("shell-{name}"));
        let renewed: EnvironmentLease = serde_json::from_value(
            client
                .call("environment.lease.acquire", acquire.clone())
                .await?,
        )?;
        ensure!(
            renewed.epoch == lease.epoch && renewed.token == lease.token,
            "shell lease changed during renewal"
        );
        let (status, concurrent) = if name.starts_with("deadline-") {
            let (status, observation) =
                tokio::join!(calls.call("exec.run", request.clone()), async {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let listing = observed.call("resource.list", json!({"limit":64})).await?;
                    let renewed: EnvironmentLease = serde_json::from_value(
                        observer
                            .call("environment.lease.acquire", acquire.clone())
                            .await?,
                    )?;
                    ensure!(
                        renewed.epoch == lease.epoch && renewed.token == lease.token,
                        "pending command prevented lease renewal"
                    );
                    ensure!(
                        listing["resources"]
                            .as_array()
                            .context("pending shell inventory absent")?
                            .iter()
                            .any(|row| row["phase"] != "closed"),
                        "no live command observed while synchronous reply was pending"
                    );
                    Ok::<_, anyhow::Error>(listing)
                });
            (status?, observation?)
        } else {
            (calls.call("exec.run", request.clone()).await?, Value::Null)
        };
        ensure!(
            status["state"] == expected
                && status["cleanup_pending"] == false
                && status["engine"] == "cmd"
                && status.get("job_id").is_none(),
            "shell {name} completion differs: {status}"
        );
        let id = status["resource_id"]
            .as_str()
            .context("shell resource absent")?;
        let (artifact, raw) = original(&bound, &format!("native-results/{id}/receipt")).await?;
        let receipt: Value = serde_json::from_slice(&raw)?;
        ensure!(
            receipt["interpreter"] == "python"
                && receipt["execution_started"] == true
                && receipt["jobs_empty"] == true
                && receipt["profiles_removed"] == true
                && receipt["scratch_removed"] == true
                && receipt["duration_secs"] == status["duration_secs"]
                && status["duration_secs"]
                    .as_f64()
                    .is_some_and(|duration| duration > 0.0),
            "shell execution/cleanup/duration receipt differs"
        );
        ensure!(
            receipt["program"]["sha256"]
                == hex::encode(Sha256::digest(include_bytes!(
                    "../../workers/windows/native/executors/shell_command.py"
                ))),
            "shell did not execute the sealed adapter"
        );
        let mut streams = json!({});
        for stream in ["stdout", "stderr"] {
            let (metadata, data) =
                original(&bound, &format!("native-results/{id}/{stream}")).await?;
            ensure!(
                metadata["sha256"] == receipt[format!("{stream}_sha256")]
                    && status["output_bytes_retained"][stream] == data.len(),
                "shell output identity/length differs"
            );
            streams[stream] = json!({"artifact":metadata,"text":String::from_utf8_lossy(&data)});
        }
        let stdout = streams["stdout"]["text"].as_str().unwrap();
        if name == "builtin" {
            let lines: Vec<_> = stdout.lines().map(str::trim).collect();
            ensure!(
                lines.len() == 5
                    && lines[0] == lines[2]
                    && lines[0].ends_with("\\work\\space directory")
                    && lines[1] == "native-environment"
                    && lines[0].starts_with(lines[3])
                    && lines[4] == "created",
                "shell cwd/environment/fresh file differs: {stdout}"
            );
        } else if name == "quotes" {
            ensure!(
                stdout.contains("literal & value")
                    && stdout.contains("\"quoted argument\"")
                    && streams["stderr"]["text"]
                        .as_str()
                        .unwrap()
                        .contains("native-error"),
                "CMD quote/redirection behavior differs"
            );
        } else if name == "nonzero" {
            ensure!(status["exit_code"] == 7, "shell exit code differs");
        } else if name == "output" {
            ensure!(
                receipt["stop_reason"] == "output_limit" && status["output_truncated"] == true,
                "shell output limit did not stop the owned Job"
            );
        } else if name.starts_with("pipe-") {
            ensure!(
                stdout == "wukong-worker-selftest\nshell-payload\r\n",
                "shell target/pipeline bytes differ: {stdout:?}"
            );
        } else {
            ensure!(
                receipt["stop_reason"] == "deadline" && stdout.contains("waiting"),
                "shell child deadline observation differs"
            );
        }
        ensure!(
            calls.call("exec.run", request.clone()).await? == status,
            "shell exact retry repeated or changed completed execution"
        );
        let mut conflict = request.clone();
        conflict["command"] = json!("echo changed");
        ensure!(
            calls.call("exec.run", conflict).await.is_err(),
            "shell request accepted changed command"
        );
        cases.push(json!({"name":name,"request":request,"status":status,"receipt":receipt,"artifact":artifact,"streams":streams,"concurrent":concurrent}));
    }
    let inventory = inventory(&bound).await?;
    ensure!(
        inventory["resources"]
            .as_array()
            .context("shell inventory absent")?
            .len()
            == cases.len(),
        "shell resource count differs"
    );
    observer.close().await?;
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":true,"scope":"native_shell_controller_vm","identity":client.identity(),"epoch":lease.epoch,"cases":cases,"inventory":inventory,"calls":calls.count,"production_execution":false}),
    )?;
    Ok(())
}
