//! Trusted CPython fixtures through the installed native resource lifecycle.
use super::{
    debugger_acceptance::Calls,
    launch_acceptance::{bytes, original, upload},
    process_acceptance::{Probe, terminal},
    process_fault_acceptance::publish,
    tests::{enrolled_fixture_address, read_fixture_profile},
};
use crate::runtime::{RuntimeClient, RuntimeConfig, protocol::EnvironmentLease};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    config: RuntimeConfig,
    manifest_sha256: String,
    python_sha256: String,
    probe: Probe,
    output: PathBuf,
}

const SOURCE: &str = r#"import hashlib, json, pathlib, subprocess, sys
import filelock
script = pathlib.Path(__file__)
target = pathlib.Path(sys.argv[-1])
denied = []
for path in (script, target):
    try:
        with path.open('r+b'):
            pass
    except PermissionError:
        denied.append(True)
    else:
        denied.append(False)
child = subprocess.run([str(target), '--echo'], capture_output=True, check=True)
print(json.dumps(dict(argv=sys.argv[1:-1], isolated=sys.flags.isolated,
    no_bytecode=sys.dont_write_bytecode, utf8=sys.flags.utf8_mode,
    source_sha256=hashlib.sha256(script.read_bytes()).hexdigest(),
    target_sha256=hashlib.sha256(target.read_bytes()).hexdigest(),
    python_sha256=hashlib.sha256(pathlib.Path(sys.executable).read_bytes()).hexdigest(),
    denied=denied, child=child.stdout.decode('utf-8'), dependency=filelock.__version__), ensure_ascii=False))
"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recovery {
    config: RuntimeConfig,
    previous: Probe,
    output: PathBuf,
}

#[tokio::test]
#[ignore = "requires completed Python acceptance and an independently observed dedicated VM service restart"]
async fn remote_native_completed_python_recovery_acceptance() -> Result<()> {
    let profile: Recovery = read_fixture_profile("WUKONG_REMOTE_WORKER_PYTHON_RECOVERY_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    ensure!(
        profile.output.is_absolute() && !profile.output.try_exists()?,
        "new absolute Python recovery report required"
    );
    let prior: Value = serde_json::from_slice(&bytes(&profile.previous)?)?;
    ensure!(
        prior["passed"] == true && prior["scope"] == "native_python_controller_vm",
        "prior Python acceptance differs"
    );
    let cases = prior["cases"]
        .as_array()
        .context("prior Python cases absent")?;
    ensure!(cases.len() == 5, "prior Python coverage differs");
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        serde_json::to_value(client.identity())? == prior["identity"],
        "Python restart changed worker binding"
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
        "Python restart did not advance the lease"
    );
    let bound = client.with_lease(lease.clone());
    let mut cached = Vec::new();
    for case in cases {
        let status = bound.call("py.run", case["request"].clone()).await?;
        ensure!(
            status == case["status"],
            "completed Python source was replayed or changed"
        );
        let (artifact, data) = original(
            &bound,
            case["artifact"]["path"]
                .as_str()
                .context("prior receipt path absent")?,
        )
        .await?;
        ensure!(
            artifact["sha256"] == case["artifact"]["sha256"]
                && serde_json::from_slice::<Value>(&data)? == case["receipt"],
            "original Python receipt changed after restart"
        );
        cached.push(status);
    }
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":true,"scope":"completed_native_python_after_observed_service_restart","prior_sha256":profile.previous.sha256,"identity":client.identity(),"epoch":lease.epoch,"cached":cached,"production_execution":false}),
    )?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a dedicated enrolled Windows VM with native Python routing and a pinned trusted probe"]
async fn remote_installed_windows_worker_python_acceptance() -> Result<()> {
    let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_PYTHON_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    ensure!(
        profile.output.is_absolute() && !profile.output.try_exists()?,
        "new absolute Python report required"
    );
    let client = RuntimeClient::connect(profile.config.clone()).await?;
    ensure!(
        client.identity().package_sha256 == profile.manifest_sha256,
        "Python fixture package differs"
    );
    super::inventory_acceptance::qualified(&client).await?;
    ensure!(
        client
            .call(
                "py.run",
                json!({"source":"pass","request_id":"unleased-python"})
            )
            .await
            .is_err(),
        "unleased Python source was admitted"
    );
    let run = format!(
        "native-python-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call(
                "environment.lease.acquire",
                json!({"run_id":run,"owner_id":"python-controller-fixture"}),
            )
            .await?,
    )?;
    lease.validate(&run, "python-controller-fixture", client.identity())?;
    let bound = client.with_lease(lease.clone());
    let target = upload(
        &bound,
        "python-fixture/target.exe",
        &bytes(&profile.probe)?,
        true,
    )
    .await?;
    let mut calls = Calls {
        client: &bound,
        log: fs::File::create_new(profile.output.with_extension("jsonl"))?,
        count: 0,
    };
    let mut cases = Vec::new();
    for (name, source, timeout, limit, expected, cancel) in [
        ("inspection", SOURCE, 30, 65536, "completed", false),
        ("syntax", "def broken(:\n", 10, 4096, "failed", false),
        (
            "deadline",
            "import time\nprint('python-started', flush=True)\ntime.sleep(120)\n",
            10,
            4096,
            "timed_out",
            false,
        ),
        (
            "output",
            "import sys\nsys.stdout.write('x' * 65536)\nsys.stdout.flush()\n",
            10,
            1024,
            "failed",
            false,
        ),
        (
            "cancel",
            "import time\nprint('python-started', flush=True)\ntime.sleep(120)\n",
            120,
            4096,
            "cancelled",
            true,
        ),
    ] {
        let mut request = json!({"source":source,"timeout_secs":timeout,"output_limit":limit,"request_id":format!("python-{name}")});
        if name == "inspection" {
            request["argv"] = json!(["-c", "literal argument", "你好"]);
            request["target"] = target["path"].clone();
        }
        let started = calls.call("py.run", request.clone()).await?;
        let id = started["job_id"]
            .as_str()
            .context("Python resource absent")?;
        if cancel {
            tokio::time::sleep(Duration::from_secs(5)).await;
            calls.call("job.cancel", json!({"job_id":id})).await?;
        }
        let status = terminal(&bound, id).await?;
        ensure!(
            status["state"] == expected && status["cleanup_pending"] != true,
            "Python {name} did not finish with confirmed cleanup: {status}"
        );
        let (artifact, raw) = original(&bound, &format!("native-results/{id}/receipt")).await?;
        let receipt: Value = serde_json::from_slice(&raw)?;
        ensure!(
            receipt["interpreter"] == "python"
                && receipt["program"]["executable"] == false
                && receipt["program"]["sha256"] == hex::encode(Sha256::digest(source.as_bytes()))
                && receipt["execution_started"] == true
                && receipt["jobs_empty"] == true
                && receipt["profiles_removed"] == true
                && receipt["scratch_removed"] == true,
            "Python {name} source/execution/cleanup receipt differs"
        );
        let mut streams = json!({});
        for stream in ["stdout", "stderr"] {
            let (metadata, data) =
                original(&bound, &format!("native-results/{id}/{stream}")).await?;
            ensure!(
                metadata["sha256"] == receipt[format!("{stream}_sha256")],
                "Python output receipt differs"
            );
            streams[stream] = json!({"artifact":metadata,"text":String::from_utf8_lossy(&data)});
        }
        match name {
            "inspection" => {
                let observed: Value =
                    serde_json::from_str(streams["stdout"]["text"].as_str().unwrap())?;
                ensure!(
                    observed["argv"] == request["argv"]
                        && observed["isolated"] == 1
                        && observed["no_bytecode"] == true
                        && observed["utf8"] == 1
                        && observed["source_sha256"] == receipt["program"]["sha256"]
                        && observed["target_sha256"] == profile.probe.sha256
                        && observed["python_sha256"] == profile.python_sha256
                        && observed["denied"] == json!([true, true])
                        && observed["child"] == "wukong-worker-selftest\n"
                        && observed["dependency"].is_string(),
                    "Python interpreter/argument/file/child observation differs: {observed}"
                );
            }
            "syntax" => ensure!(
                streams["stderr"]["text"]
                    .as_str()
                    .unwrap()
                    .contains("SyntaxError"),
                "CPython syntax error missing"
            ),
            "deadline" | "cancel" => ensure!(
                streams["stdout"]["text"]
                    .as_str()
                    .unwrap()
                    .contains("python-started"),
                "Python never reached its wait"
            ),
            "output" => ensure!(
                receipt["stop_reason"] == "output_limit",
                "Python output limit did not terminate the Job"
            ),
            _ => unreachable!(),
        }
        ensure!(
            calls.call("py.run", request.clone()).await? == status,
            "Python retry changed completed work"
        );
        let mut conflict = request.clone();
        conflict["source"] = json!("print('different-source')");
        ensure!(
            calls.call("py.run", conflict).await.is_err(),
            "Python request ID accepted changed source"
        );
        cases.push(json!({"name":name,"request":request,"status":status,"receipt":receipt,"artifact":artifact,"streams":streams}));
    }
    let listing = calls.call("resource.list", json!({"limit":64})).await?;
    let resources = listing["resources"]
        .as_array()
        .context("Python resource listing absent")?;
    ensure!(
        resources.len() == cases.len() && resources.iter().all(|row| row["phase"] == "closed"),
        "Python resource inventory differs"
    );
    let filter = upload(
        &bound,
        "analysis/filter.bpf",
        &[6, 0, 0, 0, 0, 0, 255, 127],
        false,
    )
    .await?;
    let analysis_request = json!({"filter":filter["path"],"endianness":"little",
        "probes":[{"arch":3221225534u64,"nr":1}],"request_id":"analysis-filter"});
    let analysis = calls
        .call("seccomp.analyze", analysis_request.clone())
        .await?;
    ensure!(
        analysis["supported"] == true
            && analysis["probes"][0]["decision"]["action"] == "ALLOW"
            && analysis["filter_sha256"] == filter["sha256"]
            && analysis["execution_evidence"]["native_owner"]["epoch"] == lease.epoch,
        "isolated analysis did not return the input-bound result"
    );
    ensure!(
        calls.call("seccomp.analyze", analysis_request).await? == analysis,
        "analysis retry changed the retained original result"
    );
    let (_, analysis_receipt) = original(
        &bound,
        analysis["execution_evidence"]["receipt"]["path"]
            .as_str()
            .context("analysis receipt missing")?,
    )
    .await?;
    let analysis_receipt: Value = serde_json::from_slice(&analysis_receipt)?;
    ensure!(
        analysis_receipt["jobs_empty"] == true
            && analysis_receipt["profiles_removed"] == true
            && analysis_receipt["scratch_removed"] == true,
        "analysis helper did not confirm resource cleanup"
    );
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":true,"scope":"native_python_controller_vm","identity":client.identity(),"epoch":lease.epoch,"cases":cases,"analysis":analysis,"calls":calls.count,"production_execution":false}),
    )?;
    Ok(())
}
