use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};
use std::{fs, time::Duration};
use wukong::{
    config::AppConfig,
    coordinator::{ChildRequest, Coordinator},
    domain::*,
    store::Store,
    tools::{ToolOutcome, ToolService},
};

fn worker_config(path: &std::path::Path) -> AppConfig {
    AppConfig {
        default_environment: Some("fixture".into()),
        environments: std::collections::BTreeMap::from([(
            "fixture".into(),
            wukong::environments::EnvironmentConfig {
                name: "Trusted Linux trace fixture".into(),
                platform: wukong::environments::Platform::current(),
                installation_id: None,
                runtime: wukong::runtime::RuntimeConfig {
                    transport: wukong::runtime::TransportConfig::TrustedLocal(
                        wukong::runtime::TrustedLocalConfig {
                            state_dir: path.join("worker"),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                },
            },
        )]),
        ..Default::default()
    }
}

fn result(outcome: &ToolOutcome) -> &Value {
    &outcome.value["result"]
}

fn compile(source: &std::path::Path, target: &std::path::Path, variant: bool) -> Result<()> {
    let mut command = std::process::Command::new("cc");
    command
        .arg("-std=c11")
        .arg("-O0")
        .arg("-g")
        .arg("-fno-omit-frame-pointer")
        .arg("-fPIE")
        .arg("-pie");
    if variant {
        command.arg("-DVARIANT");
    }
    let output = command.arg(source).arg("-o").arg(target).output()?;
    anyhow::ensure!(
        output.status.success(),
        "trace fixture compilation failed: {output:?}"
    );
    Ok(())
}

fn spec(source_sha256: &str) -> Value {
    json!({
        "schema_version":1,
        "abi":"sysv_amd64",
        "points":[{
            "kind":"function","point_id":"leaf",
            "location":{"kind":"symbol","module_sha256":source_sha256,"symbol":"leaf"},
            "phases":["entry","return"],
            "capture":{
                "arguments":[{"name":"length","index":1,"value_type":"u64"}],
                "buffers":[
                    {"name":"input","address_argument":0,"length":{"kind":"argument","index":1}},
                    {"name":"output","address_argument":2,"length":{"kind":"argument","index":1}}
                ],
                "return_value":"u64"
            }
        }],
        "limits":{
            "max_events":1024,"max_point_hits":256,"max_buffer_bytes":4096,
            "max_event_bytes":65536,"max_events_bytes":8388608,"stop_wait_ms":10000
        }
    })
}

fn evidence_pair(job: &JobRecord) -> Result<(String, String)> {
    let artifacts = job
        .imported_artifacts
        .as_ref()
        .and_then(|value| value["artifacts"].as_array())
        .context("trace artifacts were not imported")?;
    let find = |suffix: &str| {
        artifacts
            .iter()
            .find(|artifact| {
                artifact["artifact"]["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with(suffix))
            })
            .and_then(|artifact| artifact["evidence_id"].as_str().map(str::to_owned))
            .context("trace artifact evidence missing")
    };
    Ok((find("/trace/run.json")?, find("/trace/events.jsonl")?))
}

async fn run_trace(
    service: &ToolService,
    store: &Store,
    agent: &AgentMetadata,
    launch: &str,
    input: &str,
    spec: Value,
    call: &str,
) -> Result<(String, String)> {
    let started = service
        .execute(
            &agent.id,
            call,
            "trace_run",
            json!({"launch_spec":launch,"input":input,"spec":spec}),
        )
        .await?;
    anyhow::ensure!(!started.is_error, "trace_run failed: {started:?}");
    let job_id = result(&started)["job_id"]
        .as_str()
        .context("trace job id missing")?
        .to_owned();
    for attempt in 0..600 {
        service
            .execute(
                &agent.id,
                &format!("{call}:status:{attempt}"),
                "job_status",
                json!({"job_id":job_id}),
            )
            .await?;
        let job = store.load::<JobRecord>(&job_id)?;
        if job.status.settled() {
            if job.status.state != JobState::Completed {
                anyhow::bail!("trace job failed before completion: {job:?}");
            }
            let imported = service
                .execute(
                    &agent.id,
                    &format!("{call}:artifacts:{attempt}"),
                    "job_artifacts",
                    json!({"job_id":job_id}),
                )
                .await?;
            anyhow::ensure!(!imported.is_error, "trace import failed: {imported:?}");
            let job = store.load::<JobRecord>(&job_id)?;
            if job.evidence_ready() {
                anyhow::ensure!(
                    job.status
                        .result
                        .as_ref()
                        .is_some_and(|result| result.extra["trace"]["complete"] == true),
                    "trace is incomplete: {job:?}"
                );
                return evidence_pair(&job);
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let job = store.load::<JobRecord>(&job_id)?;
    anyhow::bail!("trace job did not become settled and evidence-ready: {job:?}")
}

#[tokio::test]
#[ignore = "requires a real Linux worker with GDB and ptrace support"]
async fn controller_executes_and_compares_real_traces() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let config = worker_config(temp.path());
    let source = std::path::Path::new("fixtures/pwn/src/trace.c");
    let left_binary = temp.path().join("left");
    let right_binary = temp.path().join("right");
    compile(source, &left_binary, false)?;
    compile(source, &right_binary, true)?;
    let right_bytes = fs::read(&right_binary)?;
    let coordinator = Coordinator::new(Store::open(temp.path().join("state"))?);
    let run = coordinator.create_run(
        &left_binary,
        "real dynamic trace",
        Budget::default(),
        serde_json::to_value(&config)?,
    )?;
    let agent = coordinator
        .create_child(
            &run.root_id,
            ChildRequest {
                name: "real trace analyst".into(),
                role: Role::Analyst,
                task: "execute and compare real function traces".into(),
                scope: "trace".into(),
                environment_id: None,
                skills: vec!["dynamic_trace".into()],
                inherit_context: false,
                wait_for_predecessors: vec![],
                dependency_policy: Default::default(),
                contract: Default::default(),
            },
        )?
        .agent
        .metadata;
    let store = coordinator.store.clone();
    let service = ToolService::new(config, store.clone()).await?;
    let staged = service
        .execute(&agent.id, "stage-left", "stage_target", json!({}))
        .await?;
    anyhow::ensure!(!staged.is_error, "stage target failed: {staged:?}");
    let left_target = result(&staged)["path"].as_str().unwrap().to_owned();
    let right = service
        .execute(
            &agent.id,
            "put-right",
            "file_put",
            json!({"path":"right","data_base64":BASE64.encode(right_bytes),"executable":true}),
        )
        .await?;
    anyhow::ensure!(!right.is_error, "stage right failed: {right:?}");
    let right_target = result(&right)["file"]["path"].as_str().unwrap().to_owned();
    let right_sha256 = result(&right)["file"]["sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    let input = service
        .execute(
            &agent.id,
            "put-input",
            "file_put",
            json!({"path":"input.bin","text":"ABC"}),
        )
        .await?;
    anyhow::ensure!(!input.is_error, "stage input failed: {input:?}");
    let input_path = result(&input)["file"]["path"].as_str().unwrap().to_owned();
    let prepare = |target: String| {
        json!({
            "source":{"kind":"staged","target":target,"dependencies":[]},
            "args":["basic","@@"],"cwd":".","env":{},"input_mode":"argv_file","timeout_secs":10
        })
    };
    let left_launch = result(
        &service
            .execute(
                &agent.id,
                "prepare-left",
                "launch_prepare",
                prepare(left_target),
            )
            .await?,
    )["launch_spec"]
        .as_str()
        .unwrap()
        .to_owned();
    let right_launch = result(
        &service
            .execute(
                &agent.id,
                "prepare-right",
                "launch_prepare",
                prepare(right_target),
            )
            .await?,
    )["launch_spec"]
        .as_str()
        .unwrap()
        .to_owned();
    let left_first = run_trace(
        &service,
        &store,
        &agent,
        &left_launch,
        &input_path,
        spec(&run.target_sha256),
        "left-first",
    )
    .await?;
    let left_second = run_trace(
        &service,
        &store,
        &agent,
        &left_launch,
        &input_path,
        spec(&run.target_sha256),
        "left-second",
    )
    .await?;
    let same = service
        .execute(
            &agent.id,
            "compare-same",
            "trace_compare",
            json!({"left":{"run_evidence_id":left_first.0,"events_evidence_id":left_first.1},"right":{"run_evidence_id":left_second.0,"events_evidence_id":left_second.1},"comparison":{"max_differences":16}}),
        )
        .await?;
    anyhow::ensure!(!same.is_error, "same-target comparison failed: {same:?}");
    anyhow::ensure!(
        result(&same)["verdict"] == "match",
        "same target differs: {same:?}"
    );
    let right_trace = run_trace(
        &service,
        &store,
        &agent,
        &right_launch,
        &input_path,
        spec(&right_sha256),
        "right",
    )
    .await?;
    let different = service
        .execute(
            &agent.id,
            "compare-different",
            "trace_compare",
            json!({"left":{"run_evidence_id":left_first.0,"events_evidence_id":left_first.1},"right":{"run_evidence_id":right_trace.0,"events_evidence_id":right_trace.1},"comparison":{"max_differences":16}}),
        )
        .await?;
    anyhow::ensure!(!different.is_error, "A/B comparison failed: {different:?}");
    anyhow::ensure!(
        result(&different)["verdict"] == "mismatch",
        "A/B comparison unexpectedly matched: {different:?}"
    );
    anyhow::ensure!(
        result(&different)["differences"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "A/B comparison has no bounded difference"
    );
    service.close().await;
    Ok(())
}
