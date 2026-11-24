use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};
use std::fs;
use wukong::{
    binary,
    config::AppConfig,
    coordinator::{ChildRequest, Coordinator},
    domain::*,
    store::Store,
    tools::{ToolOutcome, ToolService},
};
use wukong_worker_protocol::{TraceSpec, params::TraceStartArgs};

struct Fixture {
    _temp: tempfile::TempDir,
    store: Store,
    coordinator: Coordinator,
    run: RunRecord,
    agent: AgentMetadata,
    service: ToolService,
    input_path: String,
    input_sha256: String,
    input_size: u64,
    base_spec: TraceSpec,
}

struct TracePair {
    run_evidence_id: String,
    events_evidence_id: String,
}

impl TracePair {
    fn json(&self) -> Value {
        json!({"run_evidence_id":self.run_evidence_id,"events_evidence_id":self.events_evidence_id})
    }
}

impl Fixture {
    async fn new() -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("state"))?;
        let coordinator = Coordinator::new(store.clone());
        let target = temp.path().join("sample.bin");
        fs::write(&target, b"left target fixture")?;
        let run = coordinator.create_run(
            &target,
            "trace fixtures",
            Budget::default(),
            serde_json::to_value(AppConfig::default())?,
        )?;
        let agent = coordinator
            .create_child(
                &run.root_id,
                ChildRequest {
                    name: "trace owner".into(),
                    role: Role::Analyst,
                    task: "compare traces".into(),
                    scope: "trace".into(),
                    environment_id: None,
                    skills: vec![],
                    inherit_context: false,
                    wait_for_predecessors: vec![],
                    dependency_policy: Default::default(),
                    contract: Default::default(),
                },
            )?
            .agent
            .metadata;
        let service = ToolService::new(AppConfig::default(), store.clone()).await?;
        let input = b"shared input";
        let input_evidence = store.artifact(
            &run.id,
            &agent.id,
            "input",
            "fixture.input",
            input,
            "application/octet-stream",
            json!({}),
        )?;
        let input_path = "files/input.bin".to_owned();
        let input_sha256 = binary::sha256_bytes(input);
        store.save_record(&WorkerFileRecord {
            id: "input-file".into(),
            run_id: run.id.clone(),
            agent_id: agent.id.clone(),
            worker_id: "worker".into(),
            path: input_path.clone(),
            sha256: input_sha256.clone(),
            size: input.len() as u64,
            evidence_id: Some(input_evidence.id.clone()),
            created_at: now(),
        })?;
        let base_spec: TraceSpec = serde_json::from_value(json!({
            "schema_version":1,
            "abi":"sysv_amd64",
            "points":[{
                "kind":"function",
                "point_id":"f",
                "location":{"kind":"module_offset","module_sha256":run.target_sha256,"offset":4096},
                "phases":["entry","return"],
                "capture":{
                    "arguments":[{"name":"value","index":0,"value_type":"u64"}],
                    "buffers":[{"name":"bytes","address_argument":1,"length":{"kind":"constant","bytes":3}}],
                    "return_value":"i64"
                }
            }]
        }))?;
        base_spec.validate()?;
        Ok(Self {
            _temp: temp,
            store,
            coordinator,
            run,
            agent,
            service,
            input_path,
            input_sha256,
            input_size: input.len() as u64,
            base_spec,
        })
    }

    fn spec(&self, source_sha256: &str) -> TraceSpec {
        let mut spec = self.base_spec.clone();
        spec.points[0] = wukong_worker_protocol::TracePoint::Function {
            point_id: "f".into(),
            location: wukong_worker_protocol::TraceLocation::ModuleOffset {
                module_sha256: source_sha256.into(),
                offset: 4096,
            },
            phases: vec![
                wukong_worker_protocol::FunctionPhase::Entry,
                wukong_worker_protocol::FunctionPhase::Return,
            ],
            capture: self.base_spec.points[0].capture().clone(),
        };
        spec
    }

    fn add_job(
        &self,
        label: &str,
        source_sha256: &str,
        events: Vec<Value>,
        mutate: impl FnOnce(&mut Value),
    ) -> Result<TracePair> {
        self.add_job_with_target(label, source_sha256, "target.bin", events, mutate)
    }

    fn add_job_with_target(
        &self,
        label: &str,
        source_sha256: &str,
        target_member: &str,
        events: Vec<Value>,
        mutate: impl FnOnce(&mut Value),
    ) -> Result<TracePair> {
        let jsonl = events
            .iter()
            .map(|event| {
                let mut line = serde_json::to_string(event)?.into_bytes();
                line.push(b'\n');
                Ok(line)
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        self.add_job_bytes_with_target(
            label,
            source_sha256,
            target_member,
            &jsonl,
            events.len(),
            mutate,
        )
    }

    fn add_job_bytes(
        &self,
        label: &str,
        source_sha256: &str,
        jsonl: &[u8],
        events_written: usize,
        mutate: impl FnOnce(&mut Value),
    ) -> Result<TracePair> {
        self.add_job_bytes_with_target(
            label,
            source_sha256,
            "target.bin",
            jsonl,
            events_written,
            mutate,
        )
    }

    fn add_job_bytes_with_target(
        &self,
        label: &str,
        source_sha256: &str,
        target_member: &str,
        jsonl: &[u8],
        events_written: usize,
        mutate: impl FnOnce(&mut Value),
    ) -> Result<TracePair> {
        let spec = self.spec(source_sha256);
        let base = format!("files/launches/{label}");
        let launch_path = format!("files/manifests/{label}.json");
        let manifest = json!({
            "schema_version":1,
            "target":target_member,
            "base":base,
            "files":{(target_member):source_sha256,"lib.so":"c".repeat(64)},
            "args":[],
            "cwd":".",
            "env":{},
            "input_mode":"stdin_pipe",
            "timeout_secs":30
        });
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        let manifest_sha256 = binary::sha256_bytes(&manifest_bytes);
        let launch_evidence = self.store.artifact(
            &self.run.id,
            &self.agent.id,
            &format!("launch-{label}"),
            "fixture.launch",
            &manifest_bytes,
            "application/json",
            json!({}),
        )?;
        self.store.save_record(&WorkerFileRecord {
            id: format!("launch-file-{label}"),
            run_id: self.run.id.clone(),
            agent_id: self.agent.id.clone(),
            worker_id: "worker".into(),
            path: launch_path.clone(),
            sha256: manifest_sha256.clone(),
            size: manifest_bytes.len() as u64,
            evidence_id: Some(launch_evidence.id),
            created_at: now(),
        })?;
        let events_evidence = self.store.artifact(
            &self.run.id,
            &self.agent.id,
            &format!("events-{label}"),
            "runtime.file",
            jsonl,
            "application/x-ndjson",
            json!({"worker_path":format!("jobs/{label}/trace/events.jsonl")}),
        )?;
        let mut document = json!({
            "schema_version":1,
            "kind":"wukong.trace.run",
            "backend":{"engine":"gdb_mi","gdb_version":"fixture","platform":"linux","architecture":"x86_64"},
            "worker_identity":{"protocol":4,"installation_id":"fixture","boot_id":"boot","package_sha256":"d".repeat(64)},
            "spec":serde_json::to_value(&spec)?,
            "logical_plan":spec.logical_plan(),
            "plan_signature":spec.plan_signature(),
            "launch":{
                "manifest":manifest,
                "manifest_sha256":manifest_sha256,
                "launch_identity":format!("launch-{label}"),
                "target_member":target_member,
                "source_target_sha256":source_sha256,
                "executed_target_sha256":source_sha256,
                "input":{"sha256":self.input_sha256,"size":self.input_size,"mode":"stdin_pipe"},
                "environment_signature":"e".repeat(64)
            },
            "bindings":[],
            "completion":{
                "stream_closed":true,
                "complete":true,
                "reason":"target_exited",
                "events_written":events_written,
                "jsonl_bytes":jsonl.len(),
                "jsonl_sha256":binary::sha256_bytes(jsonl),
                "process":{"reason":"exited","exit_code":0},
                "limits_hit":[],
                "capture_errors":[],
                "open_call_ids":[],
                "cleanup":{"session_closed":true,"journal_closed":true,"error":null}
            }
        });
        mutate(&mut document);
        let run_bytes = serde_json::to_vec(&document)?;
        let run_evidence = self.store.artifact(
            &self.run.id,
            &self.agent.id,
            &format!("run-{label}"),
            "runtime.file",
            &run_bytes,
            "application/json",
            json!({"worker_path":format!("jobs/{label}/trace/run.json")}),
        )?;
        let parameters = serde_json::to_value(TraceStartArgs {
            launch_spec: launch_path,
            input: self.input_path.clone(),
            spec,
        })?;
        let job = JobRecord {
            id: format!("job-{label}"),
            worker_resource_id: label.into(),
            run_id: self.run.id.clone(),
            agent_id: self.agent.id.clone(),
            worker_id: "worker".into(),
            owner_generation: self.agent.generation,
            tool_name: "trace_run".into(),
            finding_id: None,
            call_id: format!("trace-{label}"),
            worker_request_id: format!("request-{label}"),
            parameters,
            target_sha256: source_sha256.into(),
            status: JobStatus {
                job_id: label.into(),
                state: JobState::Completed,
                result: None,
                stdout: String::new(),
                stderr: String::new(),
                cleanup_pending: Some(false),
                extra: Default::default(),
            },
            status_evidence_id: None,
            imported_artifacts: Some(json!({"job_id":format!("job-{label}"),"artifacts":[
                {"artifact":{"path":format!("jobs/{label}/trace/run.json"),"sha256":run_evidence.sha256,"size":run_evidence.size},"evidence_id":run_evidence.id},
                {"artifact":{"path":format!("jobs/{label}/trace/events.jsonl"),"sha256":events_evidence.sha256,"size":events_evidence.size},"evidence_id":events_evidence.id}
            ]})),
            notification_message_id: None,
            execution_notification_message_id: None,
            execution_status_evidence_id: None,
            evidence_sync: Default::default(),
            cleanup_intent: None,
            created_at: now(),
            updated_at: now(),
        };
        self.store.save_record(&job)?;
        Ok(TracePair {
            run_evidence_id: run_evidence.id,
            events_evidence_id: events_evidence.id,
        })
    }

    async fn compare(
        &self,
        call: &str,
        left: &TracePair,
        right: &TracePair,
    ) -> Result<ToolOutcome> {
        self.service
            .execute(
                &self.agent.id,
                call,
                "trace_compare",
                json!({"left":left.json(),"right":right.json(),"comparison":{"max_differences":16}}),
            )
            .await
    }
}

fn capture(sequence: u64, phase: &str) -> Value {
    json!({
        "schema_version":1,
        "event":"capture",
        "sequence":sequence,
        "point_id":"f",
        "phase":phase,
        "thread_sequence":0,
        "call_id":0,
        "parent_call_id":null,
        "arguments":[{"name":"value","index":0,"value_type":"u64","raw":"0x0000000000000001","type":"u64","value":"1"}],
        "buffers":[{
            "name":"bytes","address_argument":1,"length":{"kind":"constant","bytes":3},
            "address":{"class":"non_module","raw":"0x1000"},"length_value":3,"requested_bytes":3,
            "bytes_read":3,"status":"ok","data_base64":BASE64.encode(b"abc"),"truncated":false,"error":null
        }],
        "return_value":if phase == "return" {json!({"raw":"0xffffffffffffffff","type":"i64","value":"-1"})} else {Value::Null},
        "process":null,
        "capture_errors":[],
        "truncated":false
    })
}

fn process(sequence: u64) -> Value {
    json!({
        "schema_version":1,"event":"process","sequence":sequence,"point_id":null,"phase":null,
        "thread_sequence":0,"call_id":null,"parent_call_id":null,"arguments":[],"buffers":[],
        "return_value":null,"process":{"reason":"exited","exit_code":0,"frame":{"addr":"0x1"},"thread-id":"1"},
        "capture_errors":[],"truncated":false
    })
}

fn events() -> Vec<Value> {
    vec![capture(0, "entry"), capture(1, "return"), process(2)]
}

fn result(outcome: &ToolOutcome) -> &Value {
    &outcome.value["result"]
}

fn assert_verdict(outcome: &ToolOutcome, verdict: &str) {
    assert!(!outcome.is_error, "{outcome:?}");
    assert_eq!(result(outcome)["verdict"], verdict, "{outcome:?}");
    assert!(result(outcome)["comparison_evidence_id"].is_string());
    assert_eq!(
        result(outcome)["comparison_signature"]
            .as_str()
            .expect("comparison signature string")
            .len(),
        64
    );
}

#[tokio::test]
async fn offline_match_ignores_runtime_identities_and_replays_without_duplicate_artifacts()
-> Result<()> {
    let fixture = Fixture::new().await?;
    let left = fixture.add_job("left", &fixture.run.target_sha256, events(), |_| {})?;
    let mut right_events = events();
    right_events[0]["arguments"][0]["raw"] = json!("0xdeadbeef");
    right_events[0]["buffers"][0]["address"]["raw"] = json!("0xdeadbeef");
    right_events[2]["process"]["frame"] = json!({"addr":"0xffff"});
    right_events[2]["process"]["thread-id"] = json!("99");
    let right = fixture.add_job("right", &"b".repeat(64), right_events, |_| {})?;
    let first = fixture.compare("compare", &left, &right).await?;
    assert_verdict(&first, "match");
    assert_eq!(result(&first)["counts"]["differences"], 0);
    let before = fixture
        .store
        .records::<Evidence>(&fixture.run.id)?
        .into_iter()
        .filter(|evidence| evidence.source == "trace.comparison")
        .count();
    let replay = fixture.compare("compare", &left, &right).await?;
    assert_eq!(result(&first), result(&replay));
    let after = fixture
        .store
        .records::<Evidence>(&fixture.run.id)?
        .into_iter()
        .filter(|evidence| evidence.source == "trace.comparison")
        .count();
    assert_eq!((before, after), (1, 1));
    let mut call = fixture.store.load::<ToolCallRecord>("compare")?;
    call.state = CallState::Running;
    call.result = None;
    call.evidence_id = None;
    fixture
        .store
        .put("call", "compare", &fixture.run.id, &call)?;
    let recovered = fixture.compare("compare", &left, &right).await?;
    assert_eq!(result(&first), result(&recovered));
    let after_recovery = fixture
        .store
        .records::<Evidence>(&fixture.run.id)?
        .into_iter()
        .filter(|evidence| evidence.source == "trace.comparison")
        .count();
    assert_eq!(after_recovery, 1);
    fixture.service.close().await;
    Ok(())
}

#[tokio::test]
async fn target_member_names_do_not_create_non_target_launch_differences() -> Result<()> {
    let fixture = Fixture::new().await?;
    let left = fixture.add_job("left", &fixture.run.target_sha256, events(), |_| {})?;
    let right = fixture.add_job_with_target(
        "right",
        &fixture.run.target_sha256,
        "renamed.bin",
        events(),
        |_| {},
    )?;
    let outcome = fixture.compare("compare-renamed", &left, &right).await?;
    assert_verdict(&outcome, "match");
    fixture.service.close().await;
    Ok(())
}

#[tokio::test]
async fn evidence_reverse_lookup_rejects_invalid_cross_run_nonowner_and_same_job_inputs()
-> Result<()> {
    let fixture = Fixture::new().await?;
    let left = fixture.add_job("left", &fixture.run.target_sha256, events(), |_| {})?;
    let right = fixture.add_job("right", &fixture.run.target_sha256, events(), |_| {})?;
    assert!(fixture.compare("same", &left, &left).await?.is_error);
    let unrelated = fixture.store.artifact(
        &fixture.run.id,
        &fixture.agent.id,
        "unrelated",
        "fixture",
        b"not a trace",
        "text/plain",
        json!({}),
    )?;
    let invalid = TracePair {
        run_evidence_id: unrelated.id,
        events_evidence_id: left.events_evidence_id.clone(),
    };
    assert!(fixture.compare("invalid", &invalid, &right).await?.is_error);

    let other = fixture
        .coordinator
        .create_child(
            &fixture.run.root_id,
            ChildRequest {
                name: "other".into(),
                role: Role::Analyst,
                task: "other".into(),
                scope: "other".into(),
                environment_id: None,
                skills: vec![],
                inherit_context: false,
                wait_for_predecessors: vec![],
                dependency_policy: Default::default(),
                contract: Default::default(),
            },
        )?
        .agent
        .metadata;
    let mut left_job = fixture.store.load::<JobRecord>("job-left")?;
    left_job.agent_id = other.id.clone();
    fixture.store.save_record(&left_job)?;
    assert!(fixture.compare("nonowner", &left, &right).await?.is_error);

    let other_run = fixture.coordinator.create_run(
        &fixture.run.target_path,
        "other run",
        Budget::default(),
        json!({}),
    )?;
    let other_agent = fixture
        .coordinator
        .create_child(
            &other_run.root_id,
            ChildRequest {
                name: "cross run".into(),
                role: Role::Analyst,
                task: "cross run".into(),
                scope: "trace".into(),
                environment_id: None,
                skills: vec![],
                inherit_context: false,
                wait_for_predecessors: vec![],
                dependency_policy: Default::default(),
                contract: Default::default(),
            },
        )?
        .agent
        .metadata;
    let cross = fixture
        .service
        .execute(
            &other_agent.id,
            "cross-run",
            "trace_compare",
            json!({"left":left.json(),"right":right.json()}),
        )
        .await?;
    assert!(cross.is_error);
    fixture.service.close().await;
    Ok(())
}

#[tokio::test]
async fn incompatible_environment_and_backend_never_match() -> Result<()> {
    let fixture = Fixture::new().await?;
    let left = fixture.add_job("plan-left", &fixture.run.target_sha256, events(), |_| {})?;
    let right = fixture.add_job(
        "plan-right",
        &fixture.run.target_sha256,
        events(),
        |document| {
            document["launch"]["environment_signature"] = json!("1".repeat(64));
        },
    )?;
    let outcome = fixture.compare("environment", &left, &right).await?;
    assert_verdict(&outcome, "incomparable");

    let right = fixture.add_job(
        "backend-right",
        &fixture.run.target_sha256,
        events(),
        |document| {
            document["backend"]["gdb_version"] = json!("other");
        },
    )?;
    assert_verdict(
        &fixture.compare("backend", &left, &right).await?,
        "incomparable",
    );
    fixture.service.close().await;
    Ok(())
}

#[tokio::test]
async fn incomplete_missing_completion_limits_errors_zero_and_bad_jsonl_are_inconclusive()
-> Result<()> {
    let fixture = Fixture::new().await?;
    let left = fixture.add_job(
        "complete-left",
        &fixture.run.target_sha256,
        events(),
        |_| {},
    )?;
    for (label, mutate) in [
        (
            "missing",
            (|document: &mut Value| {
                document
                    .as_object_mut()
                    .expect("run document object")
                    .remove("completion");
            }) as fn(&mut Value),
        ),
        (
            "limit",
            (|document: &mut Value| document["completion"]["limits_hit"] = json!(["max_events"]))
                as fn(&mut Value),
        ),
        (
            "capture",
            (|document: &mut Value| {
                document["completion"]["capture_errors"] = json!(["read failed"])
            }) as fn(&mut Value),
        ),
        (
            "open",
            (|document: &mut Value| document["completion"]["open_call_ids"] = json!([0]))
                as fn(&mut Value),
        ),
        (
            "stream",
            (|document: &mut Value| document["completion"]["stream_closed"] = json!(false))
                as fn(&mut Value),
        ),
    ] {
        let right = fixture.add_job(label, &fixture.run.target_sha256, events(), mutate)?;
        assert_verdict(
            &fixture.compare(label, &left, &right).await?,
            "inconclusive",
        );
    }
    let zero = fixture.add_job_bytes("zero", &fixture.run.target_sha256, b"", 0, |_| {})?;
    assert_verdict(
        &fixture.compare("zero", &left, &zero).await?,
        "inconclusive",
    );
    let malformed =
        fixture.add_job_bytes("malformed", &fixture.run.target_sha256, b"{", 1, |_| {})?;
    assert_verdict(
        &fixture.compare("malformed", &left, &malformed).await?,
        "inconclusive",
    );
    let gap = format!("{}\n{}", capture(0, "entry"), capture(2, "return"));
    let gap =
        fixture.add_job_bytes("gap", &fixture.run.target_sha256, gap.as_bytes(), 2, |_| {})?;
    assert_verdict(&fixture.compare("gap", &left, &gap).await?, "inconclusive");
    let mut truncated = events();
    truncated[0]["truncated"] = json!(true);
    let truncated = fixture.add_job("truncated", &fixture.run.target_sha256, truncated, |_| {})?;
    assert_verdict(
        &fixture.compare("truncated", &left, &truncated).await?,
        "inconclusive",
    );
    let running = fixture.add_job("running", &fixture.run.target_sha256, events(), |_| {})?;
    let mut running_job = fixture.store.load::<JobRecord>("job-running")?;
    running_job.status.state = JobState::Running;
    running_job.status.cleanup_pending = None;
    fixture.store.save_record(&running_job)?;
    assert_verdict(
        &fixture.compare("running", &left, &running).await?,
        "inconclusive",
    );
    fixture.service.close().await;
    Ok(())
}

#[tokio::test]
async fn raw_return_process_only_and_invalid_run_json_are_classified() -> Result<()> {
    let mut fixture = Fixture::new().await?;
    if let wukong_worker_protocol::TracePoint::Function { capture, .. } =
        &mut fixture.base_spec.points[0]
    {
        capture.return_value = None;
    }
    let mut raw_events = events();
    raw_events[1]["return_value"] =
        json!({"raw":"0xffffffffffffffff","type":null,"value":"0xffffffffffffffff"});
    let left = fixture.add_job(
        "raw-left",
        &fixture.run.target_sha256,
        raw_events.clone(),
        |_| {},
    )?;
    let right = fixture.add_job("raw-right", &fixture.run.target_sha256, raw_events, |_| {})?;
    assert_verdict(&fixture.compare("raw", &left, &right).await?, "match");

    let process_only = fixture.add_job(
        "process-only",
        &fixture.run.target_sha256,
        vec![process(0)],
        |_| {},
    )?;
    assert_verdict(
        &fixture
            .compare("process-only", &left, &process_only)
            .await?,
        "inconclusive",
    );

    let invalid_run = fixture.add_job_bytes(
        "invalid-run",
        &fixture.run.target_sha256,
        b"not-json",
        1,
        |_| {},
    )?;
    assert_verdict(
        &fixture.compare("invalid-run", &left, &invalid_run).await?,
        "inconclusive",
    );
    let invalid_schema = fixture.add_job(
        "invalid-schema",
        &fixture.run.target_sha256,
        events(),
        |document| {
            document["schema_version"] = json!(2);
        },
    )?;
    assert_verdict(
        &fixture
            .compare("invalid-schema", &left, &invalid_schema)
            .await?,
        "incomparable",
    );
    fixture.service.close().await;
    Ok(())
}

#[tokio::test]
async fn integer_sign_width_and_raw_value_semantics_are_enforced() -> Result<()> {
    let fixture = Fixture::new().await?;
    for (label, left_type, left_value, right_type, right_value) in [
        ("width", "u8", "255", "u16", "255"),
        ("sign", "i8", "-1", "i16", "-1"),
        ("signedness", "i8", "-1", "u8", "255"),
    ] {
        let mut left_events = events();
        left_events[0]["arguments"][0]["type"] = json!(left_type);
        left_events[0]["arguments"][0]["value"] = json!(left_value);
        let mut right_events = left_events.clone();
        right_events[0]["arguments"][0]["type"] = json!(right_type);
        right_events[0]["arguments"][0]["value"] = json!(right_value);
        let left = fixture.add_job(
            &format!("{label}-left"),
            &fixture.run.target_sha256,
            left_events,
            |_| {},
        )?;
        let right = fixture.add_job(
            &format!("{label}-right"),
            &fixture.run.target_sha256,
            right_events,
            |_| {},
        )?;
        let outcome = fixture.compare(label, &left, &right).await?;
        assert_verdict(&outcome, "mismatch");
        assert_eq!(result(&outcome)["differences"][0]["kind"], "integer");
    }
    fixture.service.close().await;
    Ok(())
}

#[tokio::test]
async fn buffer_and_missing_event_differences_are_bounded_and_detailed() -> Result<()> {
    let fixture = Fixture::new().await?;
    let left = fixture.add_job("buffer-left", &fixture.run.target_sha256, events(), |_| {})?;
    let mut changed = events();
    changed[0]["buffers"][0]["data_base64"] = json!(BASE64.encode(b"axc!"));
    changed[0]["buffers"][0]["length_value"] = json!(4);
    changed[0]["buffers"][0]["requested_bytes"] = json!(4);
    changed[0]["buffers"][0]["bytes_read"] = json!(4);
    let right = fixture.add_job("buffer-right", &fixture.run.target_sha256, changed, |_| {})?;
    let outcome = fixture.compare("buffer", &left, &right).await?;
    assert_verdict(&outcome, "mismatch");
    let difference = &result(&outcome)["differences"][0];
    assert_eq!(difference["kind"], "buffer");
    assert_eq!(difference["first_difference_offset"], 1);
    assert_eq!(difference["different_byte_count"], 2);
    assert_eq!(difference["left_length"], 3);
    assert_eq!(difference["right_length"], 4);
    assert!(
        difference["byte_differences"]
            .as_array()
            .expect("buffer byte differences")
            .len()
            <= 8
    );
    assert!(difference["left_sha256"].is_string() && difference["right_sha256"].is_string());

    let missing_events = vec![capture(0, "entry"), process(1)];
    let missing = fixture.add_job(
        "missing-right",
        &fixture.run.target_sha256,
        missing_events,
        |_| {},
    )?;
    let outcome = fixture
        .service
        .execute(
            &fixture.agent.id,
            "missing",
            "trace_compare",
            json!({
                "left":left.json(),"right":missing.json(),"comparison":{"max_differences":1}
            }),
        )
        .await?;
    assert_verdict(&outcome, "mismatch");
    assert_eq!(
        result(&outcome)["differences"]
            .as_array()
            .expect("bounded differences")
            .len(),
        1
    );
    assert_eq!(result(&outcome)["differences_truncated"], true);
    assert!(
        result(&outcome)["counts"]["differences"]
            .as_u64()
            .expect("total difference count")
            > 1
    );
    fixture.service.close().await;
    Ok(())
}

#[tokio::test]
async fn capture_selection_ignore_and_process_semantics_are_applied() -> Result<()> {
    let fixture = Fixture::new().await?;
    let left = fixture.add_job("select-left", &fixture.run.target_sha256, events(), |_| {})?;
    let mut changed = events();
    changed[0]["arguments"][0]["value"] = json!("2");
    changed[2]["process"]["exit_code"] = json!(1);
    let right = fixture.add_job("select-right", &fixture.run.target_sha256, changed, |_| {})?;
    let ignored = fixture
        .service
        .execute(
            &fixture.agent.id,
            "ignored",
            "trace_compare",
            json!({
                "left":left.json(),"right":right.json(),
                "comparison":{"capture_ids":["f:value"],"ignore":["f:value"]}
            }),
        )
        .await?;
    assert_verdict(&ignored, "mismatch");
    assert_eq!(result(&ignored)["differences"][0]["kind"], "process");
    let selected = fixture
        .service
        .execute(
            &fixture.agent.id,
            "selected",
            "trace_compare",
            json!({
                "left":left.json(),"right":right.json(),"comparison":{"capture_ids":["f:bytes"]}
            }),
        )
        .await?;
    assert_verdict(&selected, "mismatch");
    assert!(
        result(&selected)["differences"]
            .as_array()
            .expect("selected capture differences")
            .iter()
            .all(|difference| difference["kind"] == "process")
    );
    fixture.service.close().await;
    Ok(())
}
