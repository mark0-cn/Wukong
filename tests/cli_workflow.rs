//! Exercises the real CLI, scheduler, SQLite and Rig over a local HTTP wire.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};
use wukong::{config::AppConfig, domain::*, llm::Provider, report, store::Store};

async fn read_request(stream: &mut TcpStream) -> Result<Value> {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).await?;
        ensure!(n > 0, "connection closed before request headers");
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>())
        })
        .context("content length missing")??;
    while bytes.len() < header_end + length {
        let mut chunk = [0; 8192];
        let n = stream.read(&mut chunk).await?;
        ensure!(n > 0, "connection closed early");
        bytes.extend_from_slice(&chunk[..n]);
    }
    Ok(serde_json::from_slice(
        &bytes[header_end..header_end + length],
    )?)
}
fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})
}
fn tool_evidence(request: &Value, call_id: &str) -> Option<String> {
    request["messages"]
        .as_array()?
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == call_id)
        .and_then(|m| m["content"].as_str())
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v["evidence_id"].as_str().map(str::to_owned))
}
async fn model_server() -> Result<(
    String,
    Arc<Mutex<BTreeMap<String, usize>>>,
    tokio::task::JoinHandle<Result<()>>,
)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let counters = Arc::new(Mutex::new(BTreeMap::new()));
    let counts = counters.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await?;
            let request = read_request(&mut stream).await?;
            let system = request["messages"]
                .as_array()
                .context("messages missing")?
                .iter()
                .filter(|m| m["role"] == "system" || m["role"] == "developer")
                .map(|m| m["content"].to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let role = if system.contains("role=root") {
                "root"
            } else if system.contains("role=analyst") {
                "analyst"
            } else {
                anyhow::bail!("unrecognized agent identity")
            };
            let names = request["tools"]
                .as_array()
                .context("tools missing")?
                .iter()
                .map(|t| t["function"]["name"].as_str().unwrap_or(""))
                .collect::<Vec<_>>();
            if role == "root" {
                ensure!(
                    !names.contains(&"binary_inspect") && names.contains(&"runtime_doctor"),
                    "root must receive coordination and preflight tools only"
                );
            }
            let turn = {
                let mut counts = counts.lock().await;
                let n = counts.entry(role.to_owned()).or_default();
                let turn = *n;
                *n += 1;
                turn
            };
            let calls = match (role, turn) {
                ("root", 0) => vec![
                    call(
                        "root-model",
                        "note_save",
                        json!({"title":"Fixture model","body":"Exact local binary; parsing only. No execution requested in this test."}),
                    ),
                    call(
                        "spawn-recon",
                        "task",
                        json!({"description":"Fixture recon","role":"analyst","prompt":"Inspect exact target, record static coverage with the output evidence ID, and finish.","scope":"fixture static inspection","skills":["recon"],"inherit_context":false}),
                    ),
                ],
                ("root", 1) => vec![call("root-wait", "wait", {
                    let spawned = request["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|message| message["tool_call_id"] == "spawn-recon")
                        .unwrap()["content"]
                        .as_str()
                        .unwrap();
                    let agent = spawned.split('`').nth(1).unwrap();
                    json!({"agent":agent})
                })],
                ("root", 2) => vec![call(
                    "root-finish",
                    "finish_scan",
                    json!({"summary":"Static inspection complete; no dynamic execution or verified vulnerability claimed."}),
                )],
                ("analyst", 0) => {
                    ensure!(
                        names.contains(&"coverage_record")
                            && names.contains(&"binary_inspect")
                            && !names.contains(&"debug_start")
                            && !names.contains(&"poc_verify"),
                        "controller-only selection must expose static tools without unobserved execution tools"
                    );
                    vec![call("inspect", "binary_inspect", json!({}))]
                }
                ("analyst", 1) => {
                    ensure!(
                        names.contains(&"coverage_record") && !names.contains(&"debug_start"),
                        "later requests must retain controller-only capability filtering"
                    );
                    let evidence = tool_evidence(&request, "inspect")
                        .context("model did not receive inspect evidence")?;
                    vec![
                        call(
                            "coverage",
                            "coverage_record",
                            json!({"scope":"fixture static inspection","kind":"static","outcome":"completed","notes":"ELF metadata was parsed; no dynamic coverage claimed.","evidence_ids":[evidence]}),
                        ),
                        call(
                            "child-finish",
                            "agent_finish",
                            json!({"summary":"Exact target metadata inspected and evidence linked."}),
                        ),
                    ]
                }
                _ => anyhow::bail!(
                    "unexpected model call {role}:{turn}: {}",
                    request["messages"]
                ),
            };
            let narration = match (role, turn) {
                ("root", 0) => {
                    "I will identify the fixture and delegate static inspection. Each result will link to its recorded evidence."
                }
                ("root", 1) => {
                    "Static inspection is running in the analysis agent. I am waiting for its report."
                }
                ("analyst", 0) => {
                    "I am reading the exact target's ELF metadata before selecting further analysis methods."
                }
                ("analyst", 1) => {
                    "The fixture metadata has been recorded. This test establishes static coverage only; it makes no dynamic impact claim."
                }
                _ => {
                    "Static inspection is complete and the original evidence is linked. No independently verified vulnerability is claimed."
                }
            };
            let response=json!({"id":format!("response-{role}-{turn}"),"object":"chat.completion","created":1,"model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":narration,"tool_calls":calls},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}).to_string();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await?;
        }
    });
    Ok((format!("http://{address}/v1"), counters, task))
}
fn write_target(path: &Path) -> Result<()> {
    let mut data = vec![0_u8; 256];
    data[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    data[16..18].copy_from_slice(&2_u16.to_le_bytes());
    data[18..20].copy_from_slice(&62_u16.to_le_bytes());
    data[20..24].copy_from_slice(&1_u32.to_le_bytes());
    data[52..54].copy_from_slice(&64_u16.to_le_bytes());
    data[54..56].copy_from_slice(&56_u16.to_le_bytes());
    data[58..60].copy_from_slice(&64_u16.to_le_bytes());
    fs::write(path, data)?;
    Ok(())
}
async fn cli_with_base(
    config: &Path,
    args: &[&str],
    base: Option<&str>,
) -> Result<std::process::Output> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_wukong"));
    command
        .arg("--config")
        .arg(config)
        .args(args)
        .env("WUKONG_TEST_PROVIDER_KEY", "not-a-real-secret")
        .kill_on_drop(true);
    if let Some(base) = base {
        command.env("OPENAI_BASE_URL", base);
    }
    tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .with_context(|| {
            format!(
                "CLI workflow timed out in {args:?}, config={}",
                config.display()
            )
        })?
        .context("launch CLI")
}
async fn workflow(pause_then_resume: bool) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("sample.elf");
    write_target(&target)?;
    let (endpoint, counts, server) = model_server().await?;
    let mut config = AppConfig {
        state_dir: temp.path().join("state"),
        ..AppConfig::default()
    };
    config.model.provider = Provider::OpenAi;
    config.model.model = "test-model".into();
    config.model.base_url = None;
    config.model.api_key_env = Some("WUKONG_TEST_PROVIDER_KEY".into());
    config.execution.stream = false;
    config.execution.model_retries = 0;
    if pause_then_resume {
        config.budget.max_total_tokens = 15;
    }
    let config_path = temp.path().join("config.toml");
    fs::write(&config_path, toml::to_string_pretty(&config)?)?;
    let output = cli_with_base(
        &config_path,
        &[
            "analyze",
            target.to_str().context("target path encoding")?,
            "--task",
            "Inspect only the test fixture",
        ],
        Some(&endpoint),
    )
    .await?;
    if !output.status.success() {
        let debug_store = Store::open(&config.state_dir)?;
        let runs = debug_store.runs()?;
        let errors = if let Some(run) = runs.first() {
            debug_store
                .events(&run.id, 0, 1000)?
                .into_iter()
                .filter(|e| e.kind == "agent.error")
                .map(|e| e.data)
                .collect::<Vec<_>>()
        } else {
            vec![]
        };
        let server_error = if server.is_finished() {
            format!("{:?}", server.await)
        } else {
            "still running".into()
        };
        anyhow::bail!(
            "analyze failed: errors={errors:?} server={server_error} stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let store = Store::open(&config.state_dir)?;
    let mut run = store
        .runs()?
        .into_iter()
        .next()
        .context("run not persisted")?;
    ensure!(
        run.config["model"]["base_url"] == endpoint,
        "resolved provider endpoint was not frozen"
    );
    if pause_then_resume {
        ensure!(
            run.status == Status::BudgetPaused,
            "token cap did not pause run"
        );
        let paused_calls = counts.lock().await.get("root").copied().unwrap_or_default();
        ensure!(paused_calls == 1, "paused checkpoint repeated model work");
        let output = cli_with_base(
            &config_path,
            &["resume", &run.id, "--max-tokens", "10000"],
            Some("http://127.0.0.1:1/v1"),
        )
        .await?;
        ensure!(
            output.status.success(),
            "resume failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        run = store.require("run", &run.id)?;
    }
    ensure!(
        run.status == Status::Completed,
        "workflow status {}: {}",
        run.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let agents = store.list::<AgentSnapshot>("agent", &run.id)?;
    ensure!(
        agents.len() == 2 && agents.iter().all(|a| a.status == Status::Completed),
        "agent lifecycle mismatch"
    );
    let coverage = store.list::<Coverage>("coverage", &run.id)?;
    ensure!(
        coverage.len() == 1 && coverage[0].kind == "static" && !coverage[0].evidence_ids.is_empty(),
        "coverage/evidence missing"
    );
    ensure!(
        run.total_tokens == 75,
        "usage was lost or replayed: {}",
        run.total_tokens
    );
    let counts = counts.lock().await;
    ensure!(
        counts.get("root") == Some(&3) && counts.get("analyst") == Some(&2),
        "model work duplicated on recovery"
    );
    let calls = store.list::<ToolCallRecord>("call", &run.id)?;
    ensure!(
        calls.iter().all(|c| c.state == CallState::Completed),
        "tool errors in workflow"
    );
    let desktop = wukong::desktop::service::Service::new(
        config.clone(),
        store.clone(),
        Some(run.id.clone()),
        Some(config_path),
        None,
    );
    let envelope = desktop.read(wukong::desktop::workspace::WorkspaceRequest::default())?;
    let data = envelope
        .workspace
        .data
        .as_ref()
        .context("desktop snapshot missing")?;
    ensure!(
        data.agents.len() == 2 && data.active_tools.is_empty(),
        "desktop activity diverged from completed agents"
    );
    let replies = store.records::<PublicReply>(&run.id)?;
    ensure!(
        replies.len() == 5
            && replies
                .iter()
                .all(|reply| reply.state == ReplyState::Complete),
        "public text lost across real Rig/CLI execution"
    );
    ensure!(
        replies
            .iter()
            .all(|reply| data.transcript.iter().any(|message| message.id == reply.id)),
        "desktop transcript lost a public reply"
    );
    let inspect = data
        .transcript
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|part| match part {
            wukong::desktop::presentation::ChatPart::ToolCall {
                tool_name, args, ..
            } if tool_name == "binary_inspect" => Some(args),
            _ => None,
        })
        .context("real binary inspection did not reach the desktop")?;
    ensure!(
        inspect["display"]["target"] == "sample.elf"
            && inspect["display"]["evidence_id"].is_string(),
        "tool target/evidence lost in presentation"
    );
    // Optional, disposable capture input. It contains the actual read DTO, not a hand-built UI fixture.
    if !pause_then_resume && let Some(path) = std::env::var_os("WUKONG_CAPTURE_WORKSPACE") {
        let capture = serde_json::to_vec_pretty(
            &json!({"source":"scripted local provider; real CLI/Rig/SQLite/static inspection", "envelope":envelope,"calls":calls}),
        )?;
        ensure!(
            !String::from_utf8_lossy(&capture).contains("not-a-real-secret"),
            "credential leaked into capture"
        );
        fs::write(path, capture)?;
    }
    let exported = report::export(&store, &run.id, &temp.path().join("bundle"))?;
    let verified = report::verify_bundle(&exported.output_dir)?;
    ensure!(
        verified.target_sha256 == run.target_sha256,
        "bundle target mismatch"
    );
    server.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_runs_dynamic_agents_real_rig_and_exports_evidence() -> Result<()> {
    workflow(false).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_budget_pause_resume_preserves_model_and_tool_progress() -> Result<()> {
    workflow(true).await
}
