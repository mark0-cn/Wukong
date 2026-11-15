//! Real-adapter acceptance tests for the Linux execution worker.
//!
//! Every test here is `#[ignore]`d and demands a live configured worker: set
//! `WUKONG_TEST_DOCKER_CONTAINER` to a running worker container (see
//! `workers/runtime/README.md` and `scripts/dev/start-worker.ps1`), then run with
//! `cargo test --test runtime_adapters -- --ignored`. They exercise the real
//! GDB/MI bridge, crash replay/minimization, AFL++ QEMU campaigns and the angr
//! solver against a freshly compiled fixture. No test substitutes a mock for
//! the tool it verifies.
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use wukong::runtime::{RuntimeClient, RuntimeConfig};

fn worker_config(_root: &std::path::Path) -> Result<RuntimeConfig> {
    let container = std::env::var("WUKONG_TEST_DOCKER_CONTAINER")
        .context("set WUKONG_TEST_DOCKER_CONTAINER to a running worker container")?;
    Ok(RuntimeConfig {
        transport: wukong::runtime::TransportConfig::Docker(wukong::runtime::DockerConfig {
            remote_state_dir: format!("/tmp/wukong-adapters-{}", uuid::Uuid::new_v4().simple()),
            ..wukong::runtime::DockerConfig::new(container)
        }),
        ..Default::default()
    })
}

async fn put(client: &RuntimeClient, path: &str, bytes: &[u8]) -> Result<Value> {
    client
        .call(
            "file.put",
            json!({"path":path,"data_base64":base64::engine::general_purpose::STANDARD.encode(bytes)}),
        )
        .await
}

async fn finish(client: &RuntimeClient, job: &Value) -> Result<Value> {
    for _ in 0..400 {
        let status = client
            .call("job.status", json!({"job_id": job["job_id"]}))
            .await?;
        if ["completed", "failed", "cancelled", "timed_out", "lost"]
            .contains(&status["state"].as_str().unwrap_or_default())
        {
            return Ok(status);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("job did not reach a terminal state: {job}")
}

async fn compile(
    client: &RuntimeClient,
    source: &str,
    output: &str,
    link_static: bool,
) -> Result<Value> {
    put(client, "fixture.c", source.as_bytes()).await?;
    let mut argv = vec![
        "cc",
        "-x",
        "c",
        "-g",
        "-no-pie",
        "files/fixture.c",
        "-o",
        output,
    ];
    if link_static {
        argv.push("-static");
    }
    let job = client
        .call(
            "job.start",
            json!({
                "argv": argv,
                "timeout_secs": 60,
            }),
        )
        .await?;
    let status = finish(client, &job).await?;
    assert_eq!(
        status["state"], "completed",
        "fixture compile failed: {status}"
    );
    // Jobs write only their isolated working directory. Import the collected
    // fixture as an immutable executable input for subsequent resources.
    let artifacts = client
        .call("job.artifacts", json!({"job_id":job["job_id"]}))
        .await?;
    let suffix = format!("/{output}");
    let compiled = artifacts
        .as_array()
        .context("compile artifacts missing")?
        .iter()
        .find(|artifact| {
            artifact["path"]
                .as_str()
                .is_some_and(|path| path.ends_with(&suffix))
        })
        .context("compiled fixture artifact missing")?;
    let bytes = client
        .call("file.get", json!({"path":compiled["path"]}))
        .await?;
    client
        .call(
            "file.put",
            json!({"path":output,"data_base64":bytes["data_base64"],"executable":true}),
        )
        .await
}

/// Fixture: reads stdin; exits 0 and prints `kept` exactly when the first
/// four bytes are KEEP. Compiled without PIE so module-relative offsets are
/// predictable for the angr test.
const STDIN_FIXTURE: &str = r#"
#include <stdio.h>
int main(void) {
    char input[8] = {0};
    if (fread(input, 1, 4, stdin) == 4 && input[0]=='K' && input[1]=='E' && input[2]=='E' && input[3]=='P') {
        puts("kept");
        return 0;
    }
    return 1;
}
"#;

/// Fixture: aborts when the first three bytes of the input file are BAD.
const CRASH_FIXTURE: &str = r#"
#include <stdio.h>
#include <stdlib.h>
int main(int argc, char **argv) {
    if (argc < 2) return 2;
    FILE *file = fopen(argv[1], "rb");
    if (!file) return 3;
    unsigned char input[8] = {0};
    fread(input, 1, sizeof(input), file);
    fclose(file);
    if (input[0]=='B' && input[1]=='A' && input[2]=='D') abort();
    return 0;
}
"#;

#[tokio::test]
#[ignore = "requires a live configured Linux worker; set WUKONG_TEST_DOCKER_CONTAINER"]
async fn real_gdb_breakpoints_registers_and_memory_reads() -> Result<()> {
    let root = tempfile::tempdir()?;
    let client = RuntimeClient::connect(worker_config(root.path())?).await?;
    compile(&client, STDIN_FIXTURE, "gdb-fixture", false).await?;
    let session = client
        .call(
            "debug.start",
            json!({"target":"files/gdb-fixture","args":[]}),
        )
        .await?;
    let session_id = session["session_id"]
        .as_str()
        .context("session id missing")?;
    assert_eq!(session["state"], "running");
    let broke = client
        .call(
            "debug.control",
            json!({"session_id":session_id,"action":"breakpoint_add","symbol":"main","timeout_secs":30}),
        )
        .await?;
    assert!(
        broke.to_string().contains("bkpt"),
        "breakpoint result missing: {broke}"
    );
    let ran = client
        .call(
            "debug.control",
            json!({"session_id":session_id,"action":"run","timeout_secs":30}),
        )
        .await?;
    assert!(
        ran.to_string().contains("stopped") || ran.to_string().contains("running"),
        "run result missing: {ran}"
    );
    let registers = client
        .call("debug.command", json!({"session_id":session_id,"command":"-data-list-register-values x","timeout_secs":30}))
        .await?;
    assert!(
        registers.to_string().contains("register-values"),
        "register dump missing: {registers}"
    );
    let memory = client
        .call("debug.command", json!({"session_id":session_id,"command":"-data-read-memory-bytes &main 4","timeout_secs":30}))
        .await?;
    assert!(
        memory.to_string().contains("memory"),
        "memory read missing: {memory}"
    );
    let closed = client
        .call("debug.close", json!({"session_id":session_id}))
        .await?;
    assert_eq!(closed["state"], "closed");
    client.call("worker.shutdown", json!({})).await?;
    client.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a live configured Linux worker; set WUKONG_TEST_DOCKER_CONTAINER"]
async fn real_crash_replay_reproduces_and_minimizes_keeps_the_signal() -> Result<()> {
    let root = tempfile::tempdir()?;
    let client = RuntimeClient::connect(worker_config(root.path())?).await?;
    // Keep abort's crash PC inside the hashed fixture module so minimization
    // can establish strong location equivalence without system-libc evidence.
    compile(&client, CRASH_FIXTURE, "crash-fixture", true).await?;
    let input = client
        .call(
            "file.put",
            json!({"path":"inputs/crash.bin","data_base64":"QkFEMDAwMDA="}), // "BAD00000"
        )
        .await?;
    let replay = client
        .call(
            "crash.replay",
            json!({
                "target":"files/crash-fixture",
                "input":"files/inputs/crash.bin",
                "args":["@@"],
                "expected":{"signal":6},
                "trials":1,
            }),
        )
        .await?;
    let status = finish(&client, &replay).await?;
    assert_eq!(
        status["state"], "completed",
        "crash replay failed: {status}"
    );
    assert_eq!(
        status["result"]["passed"], true,
        "expected signal 6: {status}"
    );
    let minimized = client
        .call(
            "input.minimize",
            json!({
                "target":"files/crash-fixture",
                "input":"files/inputs/crash.bin",
                "args":["@@"],
                "minimize_timeout_secs":60,
                "max_attempts":64,
            }),
        )
        .await?;
    let status = finish(&client, &minimized).await?;
    assert_eq!(
        status["state"], "completed",
        "minimization failed: {status}"
    );
    assert_eq!(
        status["result"]["complete"], true,
        "minimized input lost the crash: {status}"
    );
    assert_eq!(
        status["result"]["signature_strength"],
        "location_and_frames"
    );
    let minimized_bytes = client
        .call(
            "file.get",
            json!({"path":status["result"]["artifact"]["path"]}),
        )
        .await?;
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(
            minimized_bytes["data_base64"]
                .as_str()
                .context("minimized bytes missing")?
        )?,
        b"BAD"
    );
    assert!(
        status["result"]["minimized_size"]
            .as_u64()
            .unwrap_or(u64::MAX)
            <= input["size"].as_u64().unwrap_or(0),
        "minimization must not grow the input: {status}"
    );
    client.call("worker.shutdown", json!({})).await?;
    client.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a live configured Linux worker with AFL++; set WUKONG_TEST_DOCKER_CONTAINER"]
async fn real_afl_campaign_executes_and_emits_stats() -> Result<()> {
    let root = tempfile::tempdir()?;
    let client = RuntimeClient::connect(worker_config(root.path())?).await?;
    compile(&client, CRASH_FIXTURE, "afl-fixture", true).await?;
    put(&client, "seeds/seed", b"BA").await?;
    let campaign = client
        .call(
            "fuzz.start",
            // Docker Desktop may pipe cores externally. This check establishes
            // QEMU execution/statistics, not complete crash collection.
            json!({"target":"files/afl-fixture","seeds":"files/seeds","timeout_secs":15,"exec_timeout_ms":1000,"memory_mb":256,
                "env":{"AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES":"1"}}),
        )
        .await?;
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut stats = None;
    while Instant::now() < deadline {
        let status = client
            .call("fuzz.status", json!({"job_id": campaign["job_id"]}))
            .await?;
        if status["fuzzer_stats"]
            .as_object()
            .is_some_and(|map| !map.is_empty())
        {
            stats = Some(status);
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let stats = stats.context("AFL++ never published fuzzer_stats")?;
    let execs = stats["fuzzer_stats"]
        .as_object()
        .and_then(|map| map.values().next())
        .and_then(|entry| entry["execs_done"].as_str())
        .map(|text| text.parse::<u64>().unwrap_or(0))
        .unwrap_or(0);
    assert!(execs > 0, "AFL++ executed no test cases: {stats}");
    client
        .call("job.cancel", json!({"job_id": campaign["job_id"]}))
        .await?;
    client.call("worker.shutdown", json!({})).await?;
    client.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a live configured Linux worker with angr; set WUKONG_TEST_DOCKER_CONTAINER"]
async fn real_angr_solve_reaches_a_module_relative_address() -> Result<()> {
    let root = tempfile::tempdir()?;
    let client = RuntimeClient::connect(worker_config(root.path())?).await?;
    compile(&client, STDIN_FIXTURE, "angr-fixture", false).await?;
    // Discover main's address from the unstripped fixture, then pass it to
    // the solver as the module-relative offset it demands.
    let symbols = client
        .call(
            "job.start",
            json!({"argv":["objdump","-t","files/angr-fixture"],"timeout_secs":30,"output_limit":262144}),
        )
        .await?;
    let symbols = finish(&client, &symbols).await?;
    assert_eq!(symbols["state"], "completed", "objdump failed: {symbols}");
    // A static binary has a large symbol table; status deliberately returns
    // only its bounded tail. Inspect the complete retained output artifact.
    let output = client.call("file.get", json!({"path":format!("{}/output.stdout",symbols["artifacts_dir"].as_str().context("symbol artifacts missing")?)})).await?;
    let stdout = String::from_utf8(
        base64::engine::general_purpose::STANDARD.decode(
            output["data_base64"]
                .as_str()
                .context("objdump bytes missing")?,
        )?,
    )?;
    let main = stdout
        .lines()
        .find_map(|line| {
            let parts: Vec<_> = line.split_whitespace().collect();
            (parts.contains(&".text") && parts.last() == Some(&"main"))
                .then(|| u64::from_str_radix(parts[0], 16).ok())
                .flatten()
        })
        .context("main symbol not found")?;
    // The fixture is a no-pie ET_EXEC at 0x400000; the profile image base is
    // 0x400000, so module-relative = VA - 0x400000.
    let module_relative = main - 0x400000;
    let solved = client
        .call(
            "angr.solve",
            json!({"target":"files/angr-fixture","find":[module_relative],"stdin_length":4,"timeout_secs":600}),
        )
        .await?;
    let status = finish(&client, &solved).await?;
    assert_eq!(status["state"], "completed", "angr job failed: {status}");
    let stdout = status["result"]["stdout"]
        .as_str()
        .context("angr stdout missing")?;
    let payload: Value = serde_json::from_str(stdout).context("angr stdout is not JSON")?;
    assert_eq!(
        payload["solved"], true,
        "angr did not reach main: {payload}"
    );
    // The solved input must be persisted as job artifacts, not only stdout.
    let artifacts = client
        .call("job.artifacts", json!({"job_id": solved["job_id"]}))
        .await?;
    let paths: Vec<&str> = artifacts
        .as_array()
        .context("artifact list missing")?
        .iter()
        .filter_map(|entry| entry["path"].as_str())
        .collect();
    assert!(
        paths.iter().any(|path| path.contains("solved-input.bin")),
        "solved input not persisted as an artifact: {paths:?}"
    );
    client.call("worker.shutdown", json!({})).await?;
    client.close().await?;
    Ok(())
}
