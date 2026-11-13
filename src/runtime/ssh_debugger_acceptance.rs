//! Opt-in controller acceptance for owned native debugger launches, using only
//! the signed package's sealed looping probes and matching local PDB files.
use super::{
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
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Architecture {
    program: Probe,
    pdb: Probe,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    config: RuntimeConfig,
    manifest_sha256: String,
    probes: BTreeMap<String, Architecture>,
    output: PathBuf,
}
struct Calls<'a> {
    client: &'a RuntimeClient,
    log: fs::File,
    count: u64,
}
impl Calls<'_> {
    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.count += 1;
        let result = self.client.call(method, params.clone()).await;
        let record = match &result {
            Ok(value) => json!({"call":self.count,"method":method,"params":params,"result":value}),
            Err(error) => {
                json!({"call":self.count,"method":method,"params":params,"error":format!("{error:#}")})
            }
        };
        writeln!(self.log, "{}", serde_json::to_string(&record)?)?;
        self.log.sync_all()?;
        result
    }
    async fn inspect(&mut self, id: &str, kind: &str) -> Result<Value> {
        let value = self
            .call("debug.inspect", json!({"session_id":id,"kind":kind}))
            .await?;
        ensure!(
            value["response"]["isError"] != true,
            "MCP rejected inspection: {value}"
        );
        Ok(value)
    }
    async fn control(&mut self, id: &str, action: &str, mut params: Value) -> Result<Value> {
        params["session_id"] = json!(id);
        params["action"] = json!(action);
        params["request_id"] = json!(format!("control-{}", self.count + 1));
        let value = self.call("debug.control", params).await?;
        ensure!(
            value["response"]["isError"] != true,
            "MCP rejected control: {value}"
        );
        Ok(value)
    }
    async fn stopped(&mut self, id: &str) -> Result<Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        loop {
            let state = self.inspect(id, "state").await?;
            let result = &state["response"]["execution"];
            ensure!(result["isError"] != true, "MCP rejected run observation");
            if result["structuredContent"]["stop"].is_object() {
                return Ok(result["structuredContent"].clone());
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "owned run did not stop"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn exercise(&mut self, id: &str, architecture: &str) -> Result<Value> {
        let bp = self
            .control(
                id,
                "breakpoint_add",
                json!({"symbol":"wukong_probe!fixture_step"}),
            )
            .await?;
        let breakpoint = &bp["response"]["structuredContent"]["breakpoint"];
        let address = breakpoint["address"]
            .as_str()
            .context("fixture symbol unresolved")?
            .to_owned();
        let bp_id = breakpoint["id"]
            .as_u64()
            .context("breakpoint ID absent")?
            .to_string();
        for action in ["breakpoint_disable", "breakpoint_enable"] {
            self.control(id, action, json!({"breakpoint_id":bp_id}))
                .await?;
        }
        let mut hit = false;
        // The sealed fixture can stop at native/WOW64 startup notifications.
        // This bounded probe loop is never the general target execution policy.
        for _ in 0..3 {
            self.control(id, "continue", json!({"timeout_secs":3}))
                .await?;
            let stop = self.stopped(id).await?;
            ensure!(
                stop["stop"]["target_gone"] == false,
                "fixture target disappeared"
            );
            if stop["stop"]["stopped_at"] == address {
                hit = true;
                break;
            }
        }
        ensure!(hit, "owned fixture did not hit its PDB symbol");
        let stack = self.inspect(id, "stack").await?;
        ensure!(
            stack["response"]["structuredContent"]["frames"]
                .as_array()
                .is_some_and(|frames| frames
                    .first()
                    .is_some_and(|frame| frame["symbol"] == "wukong_probe!fixture_step")),
            "fixture stack did not resolve its PDB symbol"
        );
        let registers = self.inspect(id, "registers").await?;
        let ip = registers["response"]["structuredContent"]["instruction_pointer"]
            .as_str()
            .context("instruction pointer absent")?;
        let numeric = u64::from_str_radix(ip.trim_start_matches("0x"), 16)?;
        ensure!(
            numeric > 0 && (architecture != "x86" || numeric < 1 << 32),
            "wrong fixture architecture"
        );
        let memory = self
            .call(
                "debug.inspect",
                json!({"session_id":id,"kind":"memory","address":numeric,"length":16}),
            )
            .await?;
        ensure!(
            memory["response"]["structuredContent"]["read_size"] == 16,
            "owned memory read failed"
        );
        for kind in ["modules", "threads", "disassembly"] {
            self.inspect(id, kind).await?;
        }
        let no_exception = self
            .call("debug.inspect", json!({"session_id":id,"kind":"exception"}))
            .await?;
        ensure!(
            no_exception["response"]["isError"] == true
                && no_exception["response"]["structuredContent"]["error"]["category"] == "debugger"
                && no_exception["response"]["structuredContent"]["error"]["message"]
                    .as_str()
                    .is_some_and(|message| message
                        .starts_with("this target is not stopped on an exception:")),
            "non-fault fixture did not preserve the expected upstream exception refusal"
        );
        self.control(id, "step", json!({})).await?;
        let stepped = self.inspect(id, "registers").await?;
        ensure!(
            stepped["response"]["structuredContent"]["instruction_pointer"] != ip,
            "single step did not advance"
        );
        self.control(id, "finish", json!({})).await?;
        self.control(id, "next", json!({})).await?;
        self.control(id, "breakpoint_remove", json!({"breakpoint_id":bp_id}))
            .await?;
        self.control(id, "continue", json!({"timeout_secs":10}))
            .await?;
        self.control(id, "interrupt", json!({})).await?;
        let interrupted = self.stopped(id).await?;
        ensure!(
            interrupted["stop"]["interrupted"] == true,
            "owned break-in did not stop the run"
        );
        let old_path = registers["transcript"]
            .as_str()
            .context("transcript missing")?;
        let (first, first_bytes) = original(self.client, old_path).await?;
        self.inspect(id, "registers").await?;
        let (again, again_bytes) = original(self.client, old_path).await?;
        ensure!(
            first == again && first_bytes == again_bytes,
            "live prefix changed after append"
        );
        Ok(
            json!({"breakpoint":breakpoint,"registers":registers,"stack":stack,"interrupted":interrupted,"prefix":first}),
        )
    }
}

#[tokio::test]
#[ignore = "requires the enrolled dedicated Windows VM and signed package x64/x86 EXE/PDB fixtures"]
async fn remote_installed_windows_worker_debugger_acceptance() -> Result<()> {
    let profile: Profile = read_fixture_profile("WUKONG_REMOTE_WORKER_DEBUGGER_TEST")?;
    enrolled_fixture_address(&profile.config, false)?;
    ensure!(
        profile.output.is_absolute()
            && !profile.output.try_exists()?
            && profile.probes.keys().map(String::as_str).eq(["x64", "x86"]),
        "debugger fixture requires new output and both architectures"
    );
    let client = RuntimeClient::connect(profile.config).await?;
    ensure!(
        client.identity().package_sha256 == profile.manifest_sha256,
        "debugger fixture package differs"
    );
    ensure!(
        client.doctor().await?["capabilities"]["native_target_execution"] == false,
        "partial debugger qualification advertised production execution"
    );
    let run = format!(
        "native-debugger-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let acquire = json!({"run_id":run,"owner_id":"debugger-controller-fixture"});
    let lease: EnvironmentLease = serde_json::from_value(
        client
            .call("environment.lease.acquire", acquire.clone())
            .await?,
    )?;
    lease.validate(&run, "debugger-controller-fixture", client.identity())?;
    let bound = client.with_lease(lease.clone());
    let mut calls = Calls {
        client: &bound,
        log: fs::File::create_new(profile.output.with_extension("jsonl"))?,
        count: 0,
    };
    let mut cases = Vec::new();
    for (architecture, fixtures) in profile.probes {
        let renewed: EnvironmentLease = serde_json::from_value(
            client
                .call("environment.lease.acquire", acquire.clone())
                .await?,
        )?;
        ensure!(
            renewed.epoch == lease.epoch,
            "renewal replaced the native fence"
        );
        let base = format!("debugger/{}", fixtures.program.sha256);
        upload(
            &bound,
            &format!("{base}/wukong-probe.exe"),
            &bytes(&fixtures.program)?,
            true,
        )
        .await?;
        upload(
            &bound,
            &format!("{base}/wukong-probe.pdb"),
            &bytes(&fixtures.pdb)?,
            false,
        )
        .await?;
        let manifest = json!({"schema_version":1,"challenge_id":"trusted-native-debugger","base":format!("files/{base}"),"target":"wukong-probe.exe","files":{"wukong-probe.exe":fixtures.program.sha256,"wukong-probe.pdb":fixtures.pdb.sha256},"args":[],"cwd":"work","input_mode":"stdin_pipe","env":{},"timeout_secs":90,"service":{"mode":"offline"},"loader":null,"library_path":null,"runtime_confirmed":false});
        let launch = upload(
            &bound,
            &format!("{base}/launch.json"),
            &serde_json::to_vec(&manifest)?,
            false,
        )
        .await?;
        let request = json!({"mode":"launch","engine":"dbgeng","launch_spec":launch["path"],"request_id":format!("debugger-start-{architecture}")});
        let started = calls.call("debug.start", request.clone()).await?;
        let id = started["session_id"]
            .as_str()
            .context("native debugger session missing")?;
        ensure!(
            started["state"] == "running" && started["worker_session_id"] == id,
            "native debugger opener differs"
        );
        let outcome = async {
            ensure!(
                calls.call("debug.start", request.clone()).await? == started,
                "duplicate start changed resource"
            );
            ensure!(
                client
                    .call("environment.lease.release", json!({"lease":lease}))
                    .await
                    .is_err(),
                "live debugger did not block release"
            );
            calls.exercise(id, &architecture).await
        }
        .await;
        let close_request =
            json!({"session_id":id,"request_id":format!("debugger-close-{architecture}")});
        let closed = calls.call("debug.close", close_request.clone()).await?;
        ensure!(
            closed["state"] == "closed"
                && closed["response"]["cleanup"]["jobs_empty"] == true
                && closed["response"]["cleanup"]["profiles_removed"] == true
                && closed["response"]["cleanup"]["scratch_removed"] == true,
            "native debugger cleanup incomplete"
        );
        let observations = outcome?;
        ensure!(
            calls.call("debug.close", close_request).await? == closed,
            "close retry changed its original result"
        );
        ensure!(
            calls.call("debug.start", request).await.is_err(),
            "closed debugger was restarted"
        );
        let (artifact, transcript) = original(
            &bound,
            closed["transcript"]
                .as_str()
                .context("closed transcript absent")?,
        )
        .await?;
        fs::File::create_new(
            profile
                .output
                .with_extension(format!("{architecture}.mcp.jsonl")),
        )?
        .write_all(&transcript)?;
        cases.push(json!({"architecture":architecture,"started":started,"observations":observations,"closed":closed,"artifact":artifact}));
    }
    client
        .call("environment.lease.release", json!({"lease":lease}))
        .await?;
    client.close().await?;
    let mut report = fs::File::create_new(profile.output)?;
    report.write_all(&serde_json::to_vec_pretty(&json!({"passed":true,"scope":"managed_native_debugger_launch","identity":client.identity(),"cases":cases,"production_execution":false}))?)?;
    report.sync_all()?;
    Ok(())
}
