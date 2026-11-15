//! Actual installed minimization, using only the pinned development probes.
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
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    config: RuntimeConfig,
    manifest_sha256: String,
    probes: BTreeMap<String, Probe>,
    output: PathBuf,
}

async fn terminal(
    bound: &RuntimeClient,
    client: &RuntimeClient,
    acquire: &Value,
    lease: &EnvironmentLease,
    job: &str,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(150);
    let mut renewed = Instant::now();
    loop {
        if renewed.elapsed() > Duration::from_secs(5) {
            let next: EnvironmentLease = serde_json::from_value(
                client
                    .call("environment.lease.acquire", acquire.clone())
                    .await?,
            )?;
            ensure!(
                next.epoch == lease.epoch && next.token == lease.token,
                "minimization lease changed"
            );
            renewed = Instant::now();
        }
        let status = bound.call("job.status", json!({"job_id":job})).await?;
        if matches!(
            status["state"].as_str(),
            Some("completed" | "failed" | "cancelled" | "lost")
        ) && status["cleanup_pending"] != true
        {
            return Ok(status);
        }
        ensure!(
            Instant::now() < deadline,
            "minimization did not terminate: {status}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
async fn evidence(
    client: &RuntimeClient,
    status: &Value,
    target: &Value,
    source: &[u8],
) -> Result<Value> {
    let id = status["job_id"]
        .as_str()
        .context("minimization job missing")?;
    let result = &status["result"];
    if status["state"] == "lost" {
        let selector = result["recovery_artifact"]
            .as_str()
            .context("lost minimization has no recovery evidence")?;
        let (artifact, raw) = original(client, &format!("native-results/{id}/{selector}")).await?;
        let receipt: Value = serde_json::from_slice(&raw)?;
        ensure!(
            artifact["sha256"] == result["recovery_receipt_sha256"]
                && receipt["scope"] == "owned_resource_recovery"
                && receipt["resource_id"] == id
                && receipt["owner"] == status["native_owner"]
                && receipt["jobs_empty"] == true
                && receipt["profiles_removed"] == true
                && receipt["scratch_removed"] == true,
            "lost minimization lacks verified owned cleanup"
        );
        return Ok(json!({"artifact":artifact,"receipt":receipt,"outcome":"unknown"}));
    }
    let (artifact, raw) = original(client, &format!("native-results/{id}/receipt")).await?;
    ensure!(
        artifact["sha256"] == result["native_receipt_sha256"],
        "minimization receipt hash differs"
    );
    let receipt: Value = serde_json::from_slice(&raw)?;
    ensure!(
        receipt["scope"] == "native_minimize_host"
            && receipt["owner"] == status["native_owner"]
            && receipt["jobs_empty"] == true
            && receipt["profiles_removed"] == true
            && receipt["scratch_removed"] == true,
        "minimization cleanup or owner differs"
    );
    let rows = receipt["trials"]
        .as_array()
        .context("minimization trials missing")?;
    ensure!(
        result["attempts"] == rows.len() && result["original_size"] == source.len(),
        "minimization counts differ"
    );
    let mut personas = BTreeSet::new();
    let mut proofs = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let (metadata, bytes) =
            original(client, row["path"].as_str().context("trial path missing")?).await?;
        ensure!(
            metadata["sha256"] == row["sha256"],
            "trial receipt hash differs"
        );
        let proof: Value = serde_json::from_slice(&bytes)?;
        let (_, data) = original(
            client,
            row["input_path"]
                .as_str()
                .context("candidate path missing")?,
        )
        .await?;
        ensure!(
            proof["input"]["sha256"] == hex::encode(Sha256::digest(&data))
                && proof["trial"] == index
                && proof["request_sha256"] == receipt["request_sha256"]
                && proof["native"]["program"]["sha256"] == target["sha256"]
                && proof["native"]["jobs_empty"] == true
                && proof["native"]["profiles_removed"] == true
                && proof["native"]["scratch_removed"] == true,
            "candidate identity or cleanup differs"
        );
        ensure!(
            personas.insert(
                proof["native"]["scratch_nonce"]
                    .as_str()
                    .context("candidate persona missing")?
                    .to_owned()
            ),
            "candidate reused an earlier persona"
        );
        if index == 0 {
            ensure!(data == source, "baseline changed original input");
        }
        let (native_artifact, native_bytes) =
            original(client, &format!("native-results/{id}/trial-{index}-native")).await?;
        let execution: Value = serde_json::from_slice(&native_bytes)?;
        ensure!(
            native_artifact["sha256"] == proof["native_record"]["sha256"]
                && native_artifact["size"] == proof["native_record"]["size"]
                && native_artifact["path"] == proof["native_record"]["path"]
                && execution["schema"] == 1
                && execution["scope"] == "native_minimize_execution"
                && [
                    "resource_id",
                    "owner",
                    "request_sha256",
                    "trial",
                    "input",
                    "native"
                ]
                .iter()
                .all(|key| execution[*key] == proof[*key]),
            "durable native execution differs from completed trial"
        );
        if proof["observation"].is_object() && !proof["observation"]["location_signature"].is_null()
        {
            let observation = &proof["observation"];
            let first = &observation["exception"]["exception"];
            let unhandled = &observation["unhandled_exception"]["exception"];
            ensure!(
                unhandled["first_chance"] == false
                    && first["first_chance"].is_boolean()
                    && observation["pc_location"]["module_sha256"] == target["sha256"]
                    && observation["exception_code"] == proof["native"]["exit_code"]
                    && observation["stop"]["thread"]
                        .as_u64()
                        .is_some_and(|id| id > 0)
                    && observation["stop"]["thread"] == observation["unhandled_stop"]["thread"]
                    && first["address"] == observation["stop"]["stopped_at"]
                    && first["address"] == observation["registers"]["instruction_pointer"]
                    && first["address"] == observation["exception"]["frames"][0]["address"]
                    && ["code", "address", "flags", "noncontinuable", "parameters"]
                        .iter()
                        .all(|key| !first[*key].is_null() && first[*key] == unhandled[*key]),
                "strong observation did not bind an unhandled native crash to admitted content"
            );
            let (transcript, content) =
                original(client, &format!("native-results/{id}/trial-{index}-mcp")).await?;
            let calls: Vec<Value> = content
                .split(|b| *b == b'\n')
                .filter(|line| !line.is_empty())
                .map(serde_json::from_slice)
                .collect::<serde_json::Result<_>>()?;
            for (tool, field) in [
                ("go", "stop"),
                ("exception_triage", "exception"),
                ("registers", "registers"),
                ("modules", "modules"),
                ("go", "unhandled_stop"),
                ("exception_triage", "unhandled_exception"),
            ] {
                ensure!(
                    calls.iter().any(|call| call["tool"] == tool
                        && call["result"]["isError"] != true
                        && call["result"]["structuredContent"] == observation[field]),
                    "strong observation has no matching original MCP {field}"
                );
            }
            proofs.push(json!({"artifact":metadata,"receipt":proof,"native_artifact":native_artifact,"transcript":transcript}));
        } else {
            proofs.push(
                json!({"artifact":metadata,"receipt":proof,"native_artifact":native_artifact}),
            );
        }
    }
    let output = if let Some(path) = result["artifact"]["path"].as_str() {
        let (artifact, data) = original(client, path).await?;
        ensure!(
            artifact["sha256"] == result["artifact"]["sha256"],
            "minimized bytes changed"
        );
        Some((artifact, data))
    } else {
        None
    };
    Ok(json!({"artifact":artifact,"receipt":receipt,"trials":proofs,"output":output}))
}

#[tokio::test]
#[ignore = "requires the dedicated offline VM with installed native minimization and signed x64/x86 probes"]
async fn remote_installed_windows_worker_minimize_acceptance() -> Result<()> {
    let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_MINIMIZE_TEST")?;
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
        "minimization profile differs"
    );
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        client.identity().package_sha256 == profile.manifest_sha256,
        "minimization package differs"
    );
    ensure!(
        client
            .call(
                "input.minimize",
                json!({"target":"files/missing","input":"files/missing","request_id":"unleased"})
            )
            .await
            .is_err(),
        "unleased minimization was admitted"
    );
    let run = format!(
        "native-minimize-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let acquire = json!({"run_id":run,"owner_id":"minimization-controller-fixture"});
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call("environment.lease.acquire", acquire.clone())
            .await?,
    )?;
    lease.validate(&run, "minimization-controller-fixture", client.identity())?;
    let bound = client.with_lease(lease.clone());
    let mut calls = Calls {
        client: &bound,
        log: fs::File::create_new(profile.output.with_extension("jsonl"))?,
        count: 0,
    };
    let mut cases = Vec::new();
    let mut failures = Vec::new();
    for (architecture, probe) in &profile.probes {
        let base = format!("minimize-fixture/{architecture}");
        let target = upload(&bound, &format!("{base}/target.exe"), &bytes(probe)?, true).await?;
        for case in [
            "strong",
            "stdin",
            "launch",
            "empty",
            "budget",
            "weak",
            "negative",
            "unresolved",
            "cancel",
            "deadline",
        ] {
            let next: EnvironmentLease = serde_json::from_value(
                client
                    .call("environment.lease.acquire", acquire.clone())
                    .await?,
            )?;
            ensure!(
                next.epoch == lease.epoch && next.token == lease.token,
                "lease renewal differs"
            );
            let source: &[u8] = match case {
                "negative" => b"safe",
                "unresolved" => b"CRASH-padding",
                _ => &[0x5a, 0xa5],
            };
            let input = upload(&bound, &format!("{base}/{case}.bin"), source, false).await?;
            let id = format!("{architecture}-{case}");
            let mut request = json!({"target":target["path"],"input":input["path"],"args":["--minimize","@@"],
                "timeout_secs":10,"minimize_timeout_secs":100,"max_attempts":64,"output_limit":4096,
                "request_id":id,"call_id":id});
            match case {
                "stdin" => {
                    request["args"] = json!(["--minimize", "-"]);
                    request["input_mode"] = json!("stdin");
                }
                "empty" => request["args"] = json!(["--minimize-empty", "@@"]),
                "budget" => request["max_attempts"] = json!(1),
                "weak" => request["allow_signal_only"] = json!(true),
                "unresolved" => request["args"] = json!(["--replay", "@@"]),
                "cancel" | "deadline" => {
                    request["args"] = json!(["@@"]);
                    request["timeout_secs"] = json!(30);
                    request["minimize_timeout_secs"] =
                        json!(if case == "deadline" { 1 } else { 100 });
                }
                "launch" => {
                    let spec = json!({"schema_version":1,"base":format!("files/{base}"),"target":"target.exe",
                        "files":{"target.exe":probe.sha256},"args":["--minimize","@@"],"cwd":"work/nested",
                        "input_mode":"argv_file","env":{},"timeout_secs":10,"service":{"mode":"offline"}});
                    request["launch_spec"] = upload(
                        &bound,
                        &format!("{base}/launch.json"),
                        &serde_json::to_vec(&spec)?,
                        false,
                    )
                    .await?["path"]
                        .clone();
                    request.as_object_mut().unwrap().remove("args");
                }
                _ => {}
            }
            let started = calls.call("input.minimize", request.clone()).await?;
            let job = started["job_id"]
                .as_str()
                .context("minimization job missing")?;
            if case == "cancel" {
                tokio::time::sleep(Duration::from_secs(2)).await;
                calls.call("job.cancel", json!({"job_id":job})).await?;
            }
            let status = terminal(&bound, &client, &acquire, &lease, job).await?;
            let failed = matches!(case, "negative" | "unresolved" | "deadline");
            let expected_state = if case == "cancel" {
                "cancelled"
            } else if failed {
                "failed"
            } else {
                "completed"
            };
            if status["state"] != expected_state {
                failures.push(json!({"architecture":architecture,"case":case,
                    "error":"minimization outcome differs","expected_state":expected_state,
                    "state":status["state"]}));
            }
            // Only verified terminal cleanup permits continuing the batch.
            let proof = evidence(&bound, &status, &target, source).await?;
            if !failed && case != "cancel" && status["state"] == "completed" {
                let expected: &[u8] = match case {
                    "budget" => source,
                    "empty" => &[],
                    "weak" => &[0x5a],
                    _ => &[0xa5],
                };
                let output: Vec<u8> = serde_json::from_value(proof["output"][1].clone())?;
                let expected_strength = if case == "weak" {
                    "signal_only"
                } else {
                    "location_and_frames"
                };
                if output != expected
                    || status["result"]["complete"] != (case != "budget")
                    || status["result"]["signature_strength"] != expected_strength
                {
                    failures.push(json!({"architecture":architecture,"case":case,
                        "error":"reduced bytes, completion or equivalence differ"}));
                }
            }
            ensure!(
                calls.call("input.minimize", request.clone()).await? == started
                    && terminal(&bound, &client, &acquire, &lease, job).await? == status,
                "completed request executed again"
            );
            let mut conflict = request.clone();
            conflict["max_attempts"] = json!(2);
            ensure!(
                calls.call("input.minimize", conflict).await.is_err(),
                "request ID admitted different minimization"
            );
            let (_, unchanged) = original(&bound, input["path"].as_str().unwrap()).await?;
            ensure!(unchanged == source, "original input was modified");
            cases.push(json!({"architecture":architecture,"case":case,"request":request,"started":started,"status":status,"evidence":proof}));
        }
    }
    let owned = inventory(&bound).await?;
    ensure!(
        owned["resources"].as_array().is_some_and(
            |rows| rows.len() == cases.len() && rows.iter().all(|row| row["phase"] == "closed")
        ),
        "minimization resources remain open"
    );
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":failures.is_empty(),"failures":failures,"scope":"installed_native_input_minimization",
        "identity":client.identity(),"owner":acquire,"epoch":lease.epoch,"calls":calls.count,
        "cases":cases,"inventory":owned,"production_execution":false}),
    )?;
    ensure!(
        failures.is_empty(),
        "minimization batch failed: {failures:?}"
    );
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recovery {
    config: RuntimeConfig,
    previous: Probe,
    output: PathBuf,
}

#[tokio::test]
#[ignore = "requires completed native minimization and an independently observed dedicated VM service restart"]
async fn remote_native_completed_minimize_recovery_acceptance() -> Result<()> {
    let profile: Recovery = read_fixture_profile("WUKONG_REMOTE_WORKER_MINIMIZE_RECOVERY_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    ensure!(
        profile.output.is_absolute() && !profile.output.try_exists()?,
        "new minimization recovery report required"
    );
    let prior: Value = serde_json::from_slice(&bytes(&profile.previous)?)?;
    ensure!(
        prior["passed"] == true && prior["scope"] == "installed_native_input_minimization",
        "prior minimization proof differs"
    );
    let cases = prior["cases"]
        .as_array()
        .context("minimization cases absent")?;
    ensure!(cases.len() == 20, "prior minimization coverage differs");
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        serde_json::to_value(client.identity())? == prior["identity"],
        "minimization restart changed worker binding"
    );
    let owner = &prior["owner"];
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call("environment.lease.acquire", owner.clone())
            .await?,
    )?;
    ensure!(
        Some(lease.epoch) > prior["epoch"].as_u64(),
        "minimization restart did not advance the fence"
    );
    let bound = client.with_lease(lease.clone());
    for case in cases {
        let renewed: EnvironmentLease = serde_json::from_value(
            client
                .call("environment.lease.acquire", owner.clone())
                .await?,
        )?;
        ensure!(
            renewed.epoch == lease.epoch && renewed.token == lease.token,
            "recovery lease changed"
        );
        let started = bound
            .call("input.minimize", case["request"].clone())
            .await?;
        ensure!(
            started == case["started"],
            "recovery executed a previous candidate again"
        );
        let status = bound
            .call("job.status", json!({"job_id":started["job_id"]}))
            .await?;
        ensure!(
            status == case["status"],
            "restart changed terminal minimization status"
        );
        let first = &case["evidence"]["trials"][0]["receipt"];
        // Input uploads are epoch-scoped. Recovery must read the durable
        // baseline artifact owned by the completed job, not a successor lease's
        // interpretation of the original public input path.
        let (_, source) = original(
            &bound,
            case["evidence"]["receipt"]["trials"][0]["input_path"]
                .as_str()
                .context("durable baseline input absent")?,
        )
        .await?;
        ensure!(
            evidence(&bound, &status, &first["native"]["program"], &source).await?
                == case["evidence"],
            "recovery changed an original candidate, crash observation or minimized artifact"
        );
    }
    let owned = inventory(&bound).await?;
    ensure!(
        owned == prior["inventory"],
        "recovery changed minimization resources"
    );
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    publish(
        &profile.output,
        &json!({"passed":true,
        "scope":"completed_native_minimization_after_observed_service_restart",
        "prior_sha256":profile.previous.sha256,"identity":client.identity(),"epoch":lease.epoch,
        "cached_jobs":cases.len(),"inventory":owned,"production_execution":false}),
    )
}
