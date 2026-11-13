//! Opt-in installed acceptance using SDK dumps of the known x64/x86 probes.
//! The VM inspects these dumps; it must never launch their target image.
use super::{
    debugger_acceptance::{Architecture, Calls, launch_fixture},
    launch_acceptance::{bytes, original, upload},
    process_acceptance::Probe,
    tests::{enrolled_fixture_address, read_fixture_profile},
};
use crate::runtime::{RuntimeClient, RuntimeConfig, protocol::EnvironmentLease};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    config: RuntimeConfig,
    manifest_sha256: String,
    probes: BTreeMap<String, Architecture>,
    dumps: BTreeMap<String, Probe>,
    output: PathBuf,
}

async fn inspect_dump(calls: &mut Calls<'_>, id: &str, architecture: &str) -> Result<Value> {
    let before = calls.inspect(id, "registers").await?;
    let stack = calls.inspect(id, "stack").await?;
    let ip = before["response"]["structuredContent"]["instruction_pointer"]
        .as_str()
        .context("dump instruction pointer absent")?;
    let numeric = u64::from_str_radix(ip.trim_start_matches("0x"), 16)?;
    ensure!(
        numeric > 0 && (architecture != "x86" || numeric < 1 << 32),
        "dump architecture differs"
    );
    let memory = calls
        .call(
            "debug.inspect",
            json!({"session_id":id,"kind":"memory","address":numeric,"length":16}),
        )
        .await?;
    ensure!(
        memory["response"]["structuredContent"]["read_size"] == 16,
        "dump memory read failed"
    );
    for kind in ["modules", "threads", "disassembly", "state"] {
        calls.inspect(id, kind).await?;
    }
    let mut denied = Vec::new();
    // All actions reuse one ID. No rejected action may reserve it; close will
    // use that same ID successfully after the unchanged state is observed.
    for (action, mut arguments) in [
        ("run", json!({})),
        ("continue", json!({})),
        ("interrupt", json!({})),
        ("step", json!({})),
        ("next", json!({})),
        ("finish", json!({})),
        ("breakpoint_add", json!({"address":numeric})),
        ("breakpoint_remove", json!({"breakpoint_id":"0"})),
        ("breakpoint_enable", json!({"breakpoint_id":"0"})),
        ("breakpoint_disable", json!({"breakpoint_id":"0"})),
    ] {
        arguments["action"] = json!(action);
        arguments["session_id"] = json!(id);
        arguments["request_id"] = json!(format!("dump-denied-{architecture}"));
        let error = calls
            .call("debug.control", arguments)
            .await
            .err()
            .context("dump control was accepted")?;
        let error = format!("{error:#}");
        ensure!(
            error.contains("dump sessions permit inspection and close only"),
            "unexpected dump control refusal: {error}"
        );
        denied.push(json!({"action":action,"error":error}));
    }
    let after = calls.inspect(id, "registers").await?;
    ensure!(
        before["response"] == after["response"],
        "dump registers changed after rejected control"
    );
    Ok(json!({"registers":before,"stack":stack,"memory":memory,"denied":denied,"unchanged":after}))
}

#[tokio::test]
#[ignore = "requires the enrolled dedicated Windows VM and hash-pinned SDK dumps of trusted probes"]
async fn remote_installed_windows_worker_dump_acceptance() -> Result<()> {
    let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_DUMP_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    ensure!(
        profile.output.is_absolute()
            && !profile.output.try_exists()?
            && profile.probes.keys().map(String::as_str).eq(["x64", "x86"])
            && profile.dumps.keys().eq(profile.probes.keys()),
        "dump fixture requires new output and both architectures"
    );
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        client.identity().package_sha256 == profile.manifest_sha256,
        "dump fixture package differs"
    );
    ensure!(
        client.doctor().await?["capabilities"]["native_target_execution"] == false,
        "partial dump acceptance advertised production availability"
    );
    let acquire = json!({"run_id":format!("native-dump-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()),"owner_id":"dump-controller-fixture"});
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call("environment.lease.acquire", acquire.clone())
            .await?,
    )?;
    lease.validate(
        acquire["run_id"].as_str().unwrap(),
        "dump-controller-fixture",
        client.identity(),
    )?;
    let bound = client.with_lease(lease.clone());
    let mut calls = Calls {
        client: &bound,
        log: fs::File::create_new(profile.output.with_extension("jsonl"))?,
        count: 0,
    };
    let mut cases = Vec::new();
    for (architecture, fixture) in &profile.probes {
        let renewed: EnvironmentLease = serde_json::from_value(
            client
                .call("environment.lease.acquire", acquire.clone())
                .await?,
        )?;
        ensure!(
            renewed.epoch == lease.epoch,
            "dump renewal replaced the fence"
        );
        let base = format!("dump/{}", fixture.program.sha256);
        let launch =
            launch_fixture(&bound, &base, &fixture.program, &fixture.pdb, 120, &[]).await?;
        let dump = &profile.dumps[architecture];
        let staged = upload(
            &bound,
            &format!("{base}/snapshot.dmp"),
            &bytes(dump)?,
            false,
        )
        .await?;
        let wrong = &profile.dumps[if architecture == "x64" { "x86" } else { "x64" }];
        let wrong = upload(&bound, &format!("{base}/wrong.dmp"), &bytes(wrong)?, false).await?;
        let rejected = calls.rejected_start(json!({"mode":"dump","engine":"dbgeng","launch_spec":launch["path"],"dump":wrong["path"],"request_id":format!("wrong-dump-{architecture}")})).await?;
        let request = json!({"mode":"dump","engine":"dbgeng","launch_spec":launch["path"],"dump":staged["path"],"request_id":format!("dump-start-{architecture}")});
        let started = calls.call("debug.start", request.clone()).await?;
        let id = started["session_id"]
            .as_str()
            .context("dump session ID missing")?;
        let outcome = async {
            let identity = &started["response"]["identity"];
            ensure!(
                started["state"] == "running"
                    && started["read_only"] == true
                    && started["mode"] == "dump"
                    && identity["admitted"]["image"]["sha256"] == fixture.program.sha256
                    && identity["admitted"]["pdb"]["file"]["sha256"] == fixture.pdb.sha256
                    && identity["admitted"]["dump"]["file"]["sha256"] == dump.sha256
                    && identity["correspondence"] == "pe_pdb_and_dump_metadata"
                    && identity["loaded_pdb_bytes_verified"] == false,
                "dump identity or policy differs"
            );
            ensure!(
                calls.call("debug.start", request.clone()).await? == started,
                "duplicate dump start changed its resource"
            );
            ensure!(
                client
                    .call("environment.lease.release", json!({"lease":lease}))
                    .await
                    .is_err(),
                "live dump did not block release"
            );
            inspect_dump(&mut calls, id, architecture).await
        }
        .await;
        let close_request =
            json!({"session_id":id,"request_id":format!("dump-denied-{architecture}")});
        let closed = calls.call("debug.close", close_request.clone()).await?;
        let cleanup = &closed["response"]["cleanup"];
        ensure!(
            closed["state"] == "closed"
                && cleanup["jobs_empty"] == true
                && cleanup["profiles_removed"] == true
                && cleanup["scratch_removed"] == true
                && cleanup["target_executed"] == false
                && cleanup["read_only"] == true
                && cleanup["stdout_sha256"].is_null()
                && cleanup["stderr_sha256"].is_null()
                && cleanup["identity"] == started["response"]["identity"]["admitted"],
            "dump cleanup or non-execution proof differs"
        );
        let observations = outcome?;
        let state = calls.inspect(id, "state").await?;
        ensure!(
            state["state"] == "closed" && state["result"] == *cleanup,
            "dump terminal state differs"
        );
        ensure!(
            calls.call("debug.close", close_request).await? == closed,
            "closed dump result was replaced"
        );
        ensure!(
            calls.call("debug.start", request).await.is_err(),
            "closed dump was reopened"
        );
        let (artifact, transcript) = original(
            &bound,
            closed["transcript"]
                .as_str()
                .context("dump transcript missing")?,
        )
        .await?;
        fs::File::create_new(
            profile
                .output
                .with_extension(format!("{architecture}.mcp.jsonl")),
        )?
        .write_all(&transcript)?;
        let records: Vec<Value> = transcript
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(serde_json::from_slice)
            .collect::<std::result::Result<_, _>>()?;
        ensure!(
            records.iter().all(|record| matches!(
                record["tool"].as_str(),
                Some(
                    "open_dump"
                        | "set_symbol_path"
                        | "modules"
                        | "registers"
                        | "backtrace"
                        | "read_memory"
                        | "threads"
                        | "disassemble"
                        | "session_status"
                        | "end_session"
                )
            )) && records
                .iter()
                .filter(|record| record["tool"] == "open_dump")
                .count()
                == 1,
            "dump transcript contains target execution or mutation calls"
        );
        cases.push(json!({"architecture":architecture,"rejected_dump":rejected,"started":started,"observations":observations,"closed":closed,"artifact":artifact,"mcp_calls":records.len()}));
    }
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    let mut report = fs::File::create_new(profile.output)?;
    report.write_all(&serde_json::to_vec_pretty(&json!({"passed":true,"scope":"managed_native_debugger_dump","identity":client.identity(),"cases":cases,"calls":calls.count,"production_execution":false}))?)?;
    report.sync_all()?;
    Ok(())
}
