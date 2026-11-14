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
pub(super) struct Architecture {
    pub program: Probe,
    pub pdb: Probe,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    config: RuntimeConfig,
    manifest_sha256: String,
    probes: BTreeMap<String, Architecture>,
    output: PathBuf,
}
pub(super) struct Calls<'a> {
    pub client: &'a RuntimeClient,
    pub log: fs::File,
    pub count: u64,
}
impl Calls<'_> {
    pub(super) async fn rejected_start(&mut self, request: Value) -> Result<Value> {
        let before = self.call("resource.list", json!({"limit":64})).await?;
        let prior: Vec<_> = before["resources"]
            .as_array()
            .context("native inventory absent")?
            .iter()
            .map(|row| row["resource_id"].clone())
            .collect();
        let error = match self.call("debug.start", request.clone()).await {
            Ok(started) => {
                self.call(
                    "debug.close",
                    json!({"session_id":started["session_id"],
                    "request_id":format!("unexpected-close-{}",self.count)}),
                )
                .await?;
                anyhow::bail!("mismatched debugger input unexpectedly started a session");
            }
            Err(error) => format!("{error:#}"),
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        let closed = loop {
            let listing = self.call("resource.list", json!({"limit":64})).await?;
            let new: Vec<_> = listing["resources"]
                .as_array()
                .context("native inventory absent")?
                .iter()
                .filter(|row| !prior.contains(&row["resource_id"]))
                .collect();
            ensure!(
                new.len() == 1,
                "rejected start did not preserve exactly one original resource"
            );
            if new[0]["phase"] == "closed" {
                break new[0].clone();
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "rejected debugger did not finish owned recovery"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        ensure!(
            self.call("debug.start", request).await.is_err(),
            "rejected start was replayed"
        );
        let after = self.call("resource.list", json!({"limit":64})).await?;
        let rows = after["resources"]
            .as_array()
            .context("native inventory absent")?;
        ensure!(
            rows.len() == prior.len() + 1 && rows.contains(&closed),
            "retry created or changed a rejected resource"
        );
        let state = self
            .inspect(
                closed["resource_id"]
                    .as_str()
                    .context("rejected ID absent")?,
                "state",
            )
            .await?;
        ensure!(
            state["state"] == "lost"
                && state["cleanup_pending"] == false
                && state["result"]["outcome"] == "unknown"
                && state["native_owner"] == closed["native_owner"],
            "rejected debugger lost its durable unknown outcome"
        );
        let (artifact, bytes) = original(
            self.client,
            &format!(
                "native-results/{}/receipt",
                closed["resource_id"].as_str().unwrap()
            ),
        )
        .await?;
        let proof: Value = serde_json::from_slice(&bytes)?;
        ensure!(
            artifact["sha256"] == state["result"]["recovery_receipt_sha256"]
                && proof["resource_id"] == closed["resource_id"]
                && proof["scope"] == "owned_resource_recovery"
                && proof["owner"] == closed["native_owner"]
                && proof["jobs_empty"] == true
                && proof["profiles_removed"] == true
                && proof["scratch_removed"] == true,
            "controller recovery receipt differs from its original resource"
        );
        Ok(json!({"resource":closed,"error":error,"state":state,"receipt":artifact,"proof":proof}))
    }
    pub(super) async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
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
    pub(super) async fn inspect(&mut self, id: &str, kind: &str) -> Result<Value> {
        let value = self
            .call("debug.inspect", json!({"session_id":id,"kind":kind}))
            .await?;
        ensure!(
            value["response"]["isError"] != true,
            "MCP rejected inspection: {value}"
        );
        Ok(value)
    }
    pub(super) async fn control(
        &mut self,
        id: &str,
        action: &str,
        mut params: Value,
    ) -> Result<Value> {
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
    pub(super) async fn stopped(&mut self, id: &str) -> Result<Value> {
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
        let mut interrupt_attempts = Vec::new();
        let mut interrupted = None;
        // A pending native/WOW64 break-in can win the race before our request.
        // Reconcile its completed run handle before a new, bounded fixture run.
        for _ in 0..3 {
            self.control(id, "continue", json!({"timeout_secs":10}))
                .await?;
            let request = self.control(id, "interrupt", json!({})).await?;
            let stop = self.stopped(id).await?;
            ensure!(
                stop["stop"]["target_gone"] == false,
                "interrupt fixture exited"
            );
            interrupt_attempts.push(json!({"request":request,"stop":stop}));
            if stop["stop"]["interrupted"] == true {
                interrupted = Some(stop);
                break;
            }
            ensure!(
                request["response"]["structuredContent"]["requested"] == false
                    && stop["stop"]["timed_out"] == false,
                "owned break-in did not stop its running handle"
            );
        }
        let interrupted = interrupted.context("fixture never reached an interruptible run")?;
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
            json!({"breakpoint":breakpoint,"registers":registers,"stack":stack,"interrupted":interrupted,"interrupt_attempts":interrupt_attempts,"prefix":first}),
        )
    }
}

pub(super) async fn launch_fixture(
    client: &RuntimeClient,
    base: &str,
    program: &Probe,
    pdb: &Probe,
    timeout: u64,
    arguments: &[&str],
) -> Result<Value> {
    upload(
        client,
        &format!("{base}/wukong-probe.exe"),
        &bytes(program)?,
        true,
    )
    .await?;
    upload(
        client,
        &format!("{base}/wukong-probe.pdb"),
        &bytes(pdb)?,
        false,
    )
    .await?;
    let manifest = json!({"schema_version":1,"challenge_id":"trusted-native-debugger","base":format!("files/{base}"),"target":"wukong-probe.exe","files":{"wukong-probe.exe":program.sha256,"wukong-probe.pdb":pdb.sha256},"args":arguments,"cwd":"work","input_mode":"stdin_pipe","env":{},"timeout_secs":timeout,"service":{"mode":"offline"},"loader":null,"library_path":null,"runtime_confirmed":false});
    upload(
        client,
        &format!("{base}/launch.json"),
        &serde_json::to_vec(&manifest)?,
        false,
    )
    .await
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
    super::inventory_acceptance::qualified(&client).await?;
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
    for (architecture, fixtures) in &profile.probes {
        let renewed: EnvironmentLease = serde_json::from_value(
            client
                .call("environment.lease.acquire", acquire.clone())
                .await?,
        )?;
        ensure!(
            renewed.epoch == lease.epoch,
            "renewal replaced the native fence"
        );
        let wrong = &profile.probes[if architecture == "x64" { "x86" } else { "x64" }].pdb;
        let bad_base = format!("debugger/wrong-pdb/{}", fixtures.program.sha256);
        let bad_launch =
            launch_fixture(&bound, &bad_base, &fixtures.program, wrong, 90, &[]).await?;
        let rejected = calls
            .rejected_start(json!({"mode":"launch","engine":"dbgeng",
            "launch_spec":bad_launch["path"],"request_id":format!("wrong-pdb-{architecture}")}))
            .await?;
        let base = format!("debugger/{}", fixtures.program.sha256);
        let launch =
            launch_fixture(&bound, &base, &fixtures.program, &fixtures.pdb, 90, &[]).await?;
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
            let files = tokio::time::timeout(
                Duration::from_secs(5),
                calls.call(
                    "runtime.profile",
                    json!({"target":format!("files/{base}/wukong-probe.exe")}),
                ),
            )
            .await??;
            ensure!(
                files["files"]["target"]["sha256"] == fixtures.program.sha256
                    && files["mapped_match"] == false,
                "live-session file identity differs"
            );
            let observed = calls
                .call("debug.profile", json!({"session_id":id}))
                .await?;
            ensure!(
                observed["response"]["process"]["status"] == "observed"
                    && observed["response"]["process"]["platform"] == "windows"
                    && observed["response"]["launch_identity"]["sha256"] == fixtures.program.sha256,
                "debugger-owned process profile missing or misbound"
            );
            let identity = &started["response"]["identity"];
            ensure!(
                identity["admitted"]["image"]["sha256"] == fixtures.program.sha256
                    && identity["admitted"]["pdb"]["file"]["sha256"] == fixtures.pdb.sha256
                    && identity["observed_module"]["symbols"] == "pdb"
                    && identity["loaded_pdb_bytes_verified"] == false,
                "debugger omitted admitted image/PDB correspondence or overstated byte identity"
            );
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
            calls.exercise(id, architecture).await
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
        let terminal = calls.inspect(id, "state").await?;
        ensure!(
            terminal["state"] == "closed"
                && terminal["cleanup_pending"] == false
                && terminal["result"] == closed["response"]["cleanup"],
            "closed debugger state differs from confirmed cleanup"
        );
        ensure!(
            calls.call("debug.close", close_request).await? == closed,
            "close retry changed its original result"
        );
        ensure!(
            calls.call("debug.start", request).await? == started,
            "closed debugger start lost its original receipt"
        );
        ensure!(
            calls.inspect(id, "state").await? == terminal,
            "start retry changed the closed debugger state"
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
        ensure!(
            closed["response"]["cleanup"]["identity"]
                == started["response"]["identity"]["admitted"],
            "debugger cleanup lost admitted identity"
        );
        cases.push(json!({"architecture":architecture,"rejected_pdb":rejected,"started":started,"observations":observations,"closed":closed,"artifact":artifact}));
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
