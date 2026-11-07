use anyhow::Result;
use re_agent::{
    config::AppConfig,
    coordinator::{ChildRequest, Coordinator},
    domain::*,
    mcp::McpServerConfig,
    runtime::RuntimeBackend,
    store::Store,
    tools::{ToolOutcome, ToolService},
};
use serde_json::{Value, json};
use std::{fs, path::Path, time::Duration};

fn child(coordinator: &Coordinator, root: &str, role: Role, scope: &str) -> Result<AgentRecord> {
    Ok(coordinator
        .create_child(
            root,
            ChildRequest {
                name: scope.into(),
                role,
                task: "investigate a trusted fixture".into(),
                scope: scope.into(),
                skills: vec![],
                inherit_context: false,
                wait_for_predecessors: vec![],
            },
        )?
        .agent)
}
fn fixture(path: &Path) -> Result<(Store, Coordinator, RunRecord, AgentRecord)> {
    let store = Store::open(path.join("state"))?;
    let coordinator = Coordinator::new(store.clone());
    let target = path.join("sample.bin");
    fs::write(
        &target,
        b"fixture parser string\0another printable string\0",
    )?;
    let run = coordinator.create_run(&target, "inspect", Budget::default(), json!({}))?;
    let agent = child(&coordinator, &run.root_id, Role::Recon, "recon")?;
    Ok((store, coordinator, run, agent))
}
fn result(outcome: &ToolOutcome) -> &Value {
    &outcome.value["result"]
}

#[tokio::test]
async fn root_tools_are_restricted_at_advertisement_and_dispatch_and_failures_are_evidence()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, _, run, agent) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    let names = service
        .definitions(Role::Root)
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<Vec<_>>();
    for name in [
        "binary_inspect",
        "binary_strings",
        "stage_target",
        "job_start",
        "poc_verify",
    ] {
        assert!(!names.iter().any(|entry| entry == name));
    }
    let denied = service
        .execute(&run.root_id, "root-denied", "binary_strings", json!({}))
        .await?;
    assert!(denied.is_error);
    assert!(
        result(&denied)["error"]
            .as_str()
            .unwrap()
            .contains("unavailable")
    );
    assert!(
        !store
            .read_evidence(&run.id, denied.value["evidence_id"].as_str().unwrap())?
            .is_empty()
    );
    let allowed = service
        .execute(
            &agent.id,
            "recon-strings",
            "binary_strings",
            json!({"limit":1}),
        )
        .await?;
    assert!(!allowed.is_error);
    assert_eq!(result(&allowed)["strings"].as_array().unwrap().len(), 1);
    let malformed = service
        .execute(
            &agent.id,
            "malformed",
            "binary_strings",
            json!({"limit":"not-a-number"}),
        )
        .await?;
    assert!(malformed.is_error);
    assert!(
        result(&malformed)["error"]
            .as_str()
            .unwrap()
            .contains("integer")
    );
    let unknown = service
        .execute(
            &agent.id,
            "unknown-arg",
            "binary_strings",
            json!({"command":"ignored"}),
        )
        .await?;
    assert!(unknown.is_error);
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn angr_addresses_accept_integers_and_hex_strings() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, _, _run, agent) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    // Both spellings pass schema validation and reach dispatch, which then
    // reports the unstaged worker target instead of a schema error.
    for (index, find) in [
        json!(["0x401000"]),
        json!([4198400]),
        json!(["0x401000", 4198400]),
    ]
    .into_iter()
    .enumerate()
    {
        let outcome = service
            .execute(
                &agent.id,
                &format!("angr-valid-{index}"),
                "angr_solve",
                json!({"target":"files/target.bin","find":find}),
            )
            .await?;
        assert!(outcome.is_error);
        let error = result(&outcome)["error"].as_str().unwrap().to_owned();
        assert!(!error.contains("must have type integer"), "{error}");
        assert!(error.contains("worker_file"), "{error}");
    }
    // Malformed spellings still fail schema validation before dispatch.
    let bad = service
        .execute(
            &agent.id,
            "angr-bad",
            "angr_solve",
            json!({"target":"files/target.bin","find":["nope"]}),
        )
        .await?;
    assert!(
        result(&bad)["error"]
            .as_str()
            .unwrap()
            .contains("must have type integer")
    );
    let negative = service
        .execute(
            &agent.id,
            "angr-negative",
            "angr_solve",
            json!({"target":"files/target.bin","find":[-1]}),
        )
        .await?;
    assert!(
        result(&negative)["error"]
            .as_str()
            .unwrap()
            .contains("below its minimum")
    );
    let empty = service
        .execute(
            &agent.id,
            "angr-empty",
            "angr_solve",
            json!({"target":"files/target.bin","find":[]}),
        )
        .await?;
    assert!(
        result(&empty)["error"]
            .as_str()
            .unwrap()
            .contains("at least one find address")
    );
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn lifecycle_tools_replay_receipts_after_uncertain_outcomes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, coordinator, run, agent) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    // The root stops its child; the committed effect is recorded in a
    // transactional receipt.
    let stop_args = json!({"agent_id":agent.id,"reason":"scoped stop"});
    let first = service
        .execute(
            &run.root_id,
            "stop-lifecycle",
            "stop_agent",
            stop_args.clone(),
        )
        .await?;
    assert!(!first.is_error, "{first:?}");
    assert_eq!(result(&first)["stopped"].as_array().unwrap().len(), 1);
    // Simulate a lost response: the effect committed but the envelope never
    // reached the model. The durable retry must return the receipt instead
    // of re-running the transition against an already-terminal agent.
    let mut record: ToolCallRecord = store.require("call", "stop-lifecycle")?;
    record.state = CallState::Uncertain;
    store.put("call", "stop-lifecycle", &run.id, &record)?;
    let replay = service
        .execute(&run.root_id, "stop-lifecycle", "stop_agent", stop_args)
        .await?;
    assert!(!replay.is_error, "stop replay failed: {replay:?}");
    assert_eq!(result(&replay)["stopped"].as_array().unwrap().len(), 1);

    // finish_scan runs on a fresh assessment whose root has no children and
    // no terminal notifications to read first.
    let second_target = temp.path().join("second.bin");
    fs::write(&second_target, b"second fixture")?;
    let second = coordinator.create_run(
        &second_target,
        "second assessment",
        Budget::default(),
        json!({}),
    )?;
    let finish_args = json!({"summary":"assessment complete"});
    let finish = service
        .execute(
            &second.root_id,
            "finish-lifecycle",
            "finish_scan",
            finish_args.clone(),
        )
        .await?;
    assert!(!finish.is_error, "{finish:?}");
    let mut record: ToolCallRecord = store.require("call", "finish-lifecycle")?;
    record.state = CallState::Uncertain;
    store.put("call", "finish-lifecycle", &second.id, &record)?;
    // The root is already terminal after finish_scan; the replay must come
    // from the receipt rather than fail the terminal-agent check.
    let replay = service
        .execute(
            &second.root_id,
            "finish-lifecycle",
            "finish_scan",
            finish_args,
        )
        .await?;
    assert!(!replay.is_error, "finish replay failed: {replay:?}");
    assert_eq!(
        store.require::<RunRecord>("run", &second.id)?.status,
        Status::Completed
    );
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn durable_receipts_prevent_repeated_messages_and_completed_results_replay_exactly()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, coordinator, run, agent) = fixture(temp.path())?;
    let recipient = child(&coordinator, &run.root_id, Role::Reverse, "reverse")?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    let args = json!({"agent_id":recipient.id,"message":"Inspect parser"});
    let first = service
        .execute(
            &agent.id,
            "send-once",
            "send_message_to_agent",
            args.clone(),
        )
        .await?;
    let second = service
        .execute(
            &agent.id,
            "send-once",
            "send_message_to_agent",
            args.clone(),
        )
        .await?;
    assert_eq!(first.value, second.value);
    assert_eq!(store.inbox(&recipient.id)?.len(), 1);
    // Simulate controller death after the DB effect committed but before its
    // outer tool ledger was finalized. The transactional receipt still exists.
    let mut ledger: ToolCallRecord = store.require("call", "send-once")?;
    ledger.state = CallState::Running;
    ledger.result = None;
    store.put("call", &ledger.id, &run.id, &ledger)?;
    let recovered = service
        .execute(
            &agent.id,
            "send-once",
            "send_message_to_agent",
            args.clone(),
        )
        .await?;
    assert!(!recovered.is_error);
    assert_eq!(store.inbox(&recipient.id)?.len(), 1);
    assert!(
        service
            .execute(
                &agent.id,
                "send-once",
                "send_message_to_agent",
                json!({"agent_id":recipient.id,"message":"different"})
            )
            .await
            .is_err()
    );
    let spawn = json!({"name":"Hunter","role":"hunter","task":"Inspect boundary","scope":"boundary","inherit_context":false});
    let created = service
        .execute(&run.root_id, "spawn-once", "create_agent", spawn.clone())
        .await?;
    let mut ledger: ToolCallRecord = store.require("call", "spawn-once")?;
    ledger.state = CallState::Running;
    ledger.result = None;
    store.put("call", &ledger.id, &run.id, &ledger)?;
    let recovered = service
        .execute(&run.root_id, "spawn-once", "create_agent", spawn)
        .await?;
    assert_eq!(
        result(&created)["agent"]["id"],
        result(&recovered)["agent"]["id"]
    );
    assert_eq!(store.list::<AgentRecord>("agent", &run.id)?.len(), 4);
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn large_outputs_remain_original_and_can_be_read_in_ranges() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, _, run, agent) = fixture(temp.path())?;
    let mut config = AppConfig::default();
    config.tools.max_output_bytes = 1024;
    let service = ToolService::new(config, store.clone()).await?;
    let output = service
        .execute(
            &agent.id,
            "large-note",
            "note_save",
            json!({"title":"large","body":"A".repeat(10000)}),
        )
        .await?;
    assert!(!output.is_error);
    assert_eq!(output.value["truncated"], true);
    let evidence_id = output.value["evidence_id"].as_str().unwrap();
    let raw = store.read_evidence(&run.id, evidence_id)?;
    assert!(raw.len() > 10000);
    let original: Value = serde_json::from_slice(&raw)?;
    assert_eq!(original["body"].as_str().unwrap().len(), 10000);
    let range = service
        .execute(
            &agent.id,
            "read-range",
            "evidence_read",
            json!({"evidence_id":evidence_id,"offset":0,"length":128}),
        )
        .await?;
    assert!(!range.is_error);
    assert_eq!(result(&range)["length"], 128);
    assert_eq!(result(&range)["next_offset"], 128);
    let escaped = service
        .execute(
            &agent.id,
            "escaped-note",
            "note_save",
            json!({"title":"escape-heavy","body":"\\\"\n\0".repeat(10000)}),
        )
        .await?;
    assert!(
        serde_json::to_vec(&escaped.value)?.len() <= 1024,
        "escaped preview exceeded its model-facing byte bound"
    );
    for length in [500, 700, 900] {
        let output = service
            .execute(
                &agent.id,
                &format!("boundary-{length}"),
                "note_save",
                json!({"title":"near-boundary","body":"A".repeat(length)}),
            )
            .await?;
        assert!(
            serde_json::to_vec(&output.value)?.len() <= 1024,
            "envelope overhead exceeded byte bound at length {length}"
        );
    }
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn model_boolean_or_fake_verification_cannot_promote_a_finding() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, coordinator, run, producer) = fixture(temp.path())?;
    let verifier = child(&coordinator, &run.root_id, Role::Verifier, "verify")?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    let evidence = store.artifact(
        &run.id,
        &producer.id,
        "evidence",
        "fixture",
        b"observed parser behavior",
        "text/plain",
        json!({}),
    )?;
    let filed=service.execute(&producer.id,"finding","finding_file",json!({"title":"Parser hypothesis","description":"Needs reproduction","stage":"suspected","evidence_ids":[evidence.id]})).await?;
    assert!(!filed.is_error);
    let id = result(&filed)["finding"]["id"].as_str().unwrap();
    let promotion=service.execute(&producer.id,"fake-promotion","finding_update",json!({"id":id,"stage":"impact_verified","verification_id":"not-real","impact":"arbitrary code execution"})).await?;
    assert!(promotion.is_error);
    let boolean = service
        .execute(
            &verifier.id,
            "fake-boolean",
            "verification_record",
            json!({"finding_id":id,"job_id":"not-real","passed":true}),
        )
        .await?;
    assert!(boolean.is_error);
    assert!(
        result(&boolean)["error"]
            .as_str()
            .unwrap()
            .contains("unknown argument")
    );
    let fake_job = service
        .execute(
            &verifier.id,
            "fake-job",
            "verification_record",
            json!({"finding_id":id,"job_id":"not-real"}),
        )
        .await?;
    assert!(fake_job.is_error);
    let unauthorized = service
        .execute(
            &producer.id,
            "producer-verifies",
            "poc_verify",
            json!({"finding_id":id,"target":"files/x","input":"files/y"}),
        )
        .await?;
    assert!(unauthorized.is_error);
    let finding: Finding = store.require("finding", id)?;
    assert_eq!(finding.stage, FindingStage::Suspected);
    assert!(finding.verification_id.is_none());
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn lifecycle_rejects_unread_messages_and_new_effects_after_finish_but_allows_exact_replay()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, _, run, agent) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    let finish_args = json!({"summary":"Trusted fixture inspected"});
    let finished = service
        .execute(
            &agent.id,
            "finish-child",
            "agent_finish",
            finish_args.clone(),
        )
        .await?;
    assert!(!finished.is_error);
    let late = service
        .execute(
            &agent.id,
            "after-finish",
            "note_save",
            json!({"title":"late","body":"must not execute"}),
        )
        .await?;
    assert!(late.is_error);
    assert!(store.list::<SharedDocument>("note", &run.id)?.is_empty());
    let replay = service
        .execute(&agent.id, "finish-child", "agent_finish", finish_args)
        .await?;
    assert_eq!(replay.value, finished.value);
    let root_finish = service
        .execute(
            &run.root_id,
            "finish-root-too-early",
            "finish_scan",
            json!({"summary":"done"}),
        )
        .await?;
    assert!(root_finish.is_error);
    assert!(
        result(&root_finish)["error"]
            .as_str()
            .unwrap()
            .contains("unread")
    );
    let messages = store.inbox(&run.root_id)?;
    assert!(!messages.is_empty());
    store.checkpoint_and_ack(
        &run.root_id,
        json!({}),
        &messages
            .iter()
            .map(|message| message.id)
            .collect::<Vec<_>>(),
    )?;
    let done = service
        .execute(
            &run.root_id,
            "finish-root",
            "finish_scan",
            json!({"summary":"done"}),
        )
        .await?;
    assert!(!done.is_error);
    assert_eq!(
        store.require::<RunRecord>("run", &run.id)?.status,
        Status::Completed
    );
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn unknown_mcp_side_effect_is_never_blindly_replayed() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, _, run, agent) = fixture(temp.path())?;
    let script = temp.path().join("mcp_fixture.py");
    let counter = temp.path().join("counter.txt");
    fs::write(
        &script,
        r#"import json,sys,pathlib
counter=pathlib.Path(sys.argv[1])
for line in sys.stdin:
    request=json.loads(line)
    if 'id' not in request: continue
    method=request['method']
    if method=='initialize': result={'protocolVersion':'2025-06-18','capabilities':{'tools':{}},'serverInfo':{'name':'fixture','version':'1'}}
    elif method=='tools/list': result={'tools':[{'name':'mutate','description':'increment once','inputSchema':{'type':'object','properties':{}},'annotations':{'readOnlyHint':False}}]}
    elif method=='tools/call':
        count=int(counter.read_text())+1 if counter.exists() else 1
        counter.write_text(str(count))
        result={'content':[{'type':'text','text':str(count)}],'isError':False}
    else: result={}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
"#,
    )?;
    let mut config = AppConfig::default();
    config.mcp.push(McpServerConfig {
        name: "fixture".into(),
        command: "python".into(),
        args: vec![
            script.to_string_lossy().into_owned(),
            counter.to_string_lossy().into_owned(),
        ],
        env: vec![],
        timeout_seconds: 10,
    });
    let service = ToolService::new(config, store.clone()).await?;
    let first = service
        .execute(&agent.id, "mcp-once", "mcp_fixture_mutate", json!({}))
        .await?;
    assert!(!first.is_error);
    assert_eq!(fs::read_to_string(&counter)?, "1");
    let mut ledger: ToolCallRecord = store.require("call", "mcp-once")?;
    ledger.state = CallState::Running;
    ledger.result = None;
    store.put("call", &ledger.id, &run.id, &ledger)?;
    let repeated = service
        .execute(&agent.id, "mcp-once", "mcp_fixture_mutate", json!({}))
        .await?;
    assert!(repeated.is_error);
    assert_eq!(fs::read_to_string(&counter)?, "1");
    assert_eq!(
        store.require::<ToolCallRecord>("call", "mcp-once")?.state,
        CallState::Uncertain
    );
    let resolved = service.reconcile_call(
        "mcp-once",
        json!({"observed_counter":1}),
        &[first.value["evidence_id"].as_str().unwrap().into()],
        false,
    )?;
    assert!(!resolved.is_error);
    let final_replay = service
        .execute(&agent.id, "mcp-once", "mcp_fixture_mutate", json!({}))
        .await?;
    assert_eq!(resolved.value, final_replay.value);
    assert_eq!(fs::read_to_string(&counter)?, "1");
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn runaway_mcp_stdout_is_bounded_by_the_line_cap() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let script = temp.path().join("runaway.py");
    // Streams an endless line: the client must stop reading at the 2 MiB cap
    // instead of buffering without bound.
    std::fs::write(
        &script,
        "import sys\nsys.stdout.write('x'*100000000)\nsys.stdout.flush()\nimport time\ntime.sleep(60)\n",
    )?;
    let mut config = AppConfig::default();
    config.mcp.push(McpServerConfig {
        name: "runaway".into(),
        command: "python".into(),
        args: vec![script.to_string_lossy().into_owned()],
        env: vec![],
        timeout_seconds: 20,
    });
    let error = match ToolService::new(config, Store::open(temp.path().join("state"))?).await {
        Ok(_) => panic!("an MCP server without a newline must not initialize"),
        Err(error) => error,
    };
    assert!(format!("{error:#}").contains("2 MiB"), "{error:#}");
    Ok(())
}

#[tokio::test]
#[ignore = "requires a live configured Linux Docker worker; set RE_AGENT_TEST_DOCKER_CONTAINER"]
async fn docker_independent_verification_uses_real_trials_and_limits_impact() -> Result<()> {
    let container = std::env::var("RE_AGENT_TEST_DOCKER_CONTAINER")?;
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("state"))?;
    let coordinator = Coordinator::new(store.clone());
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/build/benign");
    let run = coordinator.create_run(
        &target,
        "verify trusted fixture behavior",
        Budget::default(),
        json!({}),
    )?;
    let producer = child(&coordinator, &run.root_id, Role::Hunter, "producer")?;
    let verifier = child(&coordinator, &run.root_id, Role::Verifier, "verifier")?;
    let mut config = AppConfig::default();
    config.runtime.backend = RuntimeBackend::Docker;
    config.runtime.docker_container = Some(container);
    config.runtime.remote_state_dir = format!("/tmp/{}", id("re_agent_tool_test"));
    config.tools.timeout_seconds = 180;
    let service = ToolService::new(config, store.clone()).await?;
    let staged = service
        .execute(&producer.id, "stage", "stage_target", json!({}))
        .await?;
    assert!(!staged.is_error, "{:?}", staged.value);
    let target = result(&staged)["path"].as_str().unwrap();
    let input = service
        .execute(
            &producer.id,
            "input",
            "file_put",
            json!({"path":"inputs/number","data_base64":"Nwo="}),
        )
        .await?;
    assert!(!input.is_error);
    let input_path = result(&input)["file"]["path"].as_str().unwrap();
    let finding=service.execute(&producer.id,"finding","finding_file",json!({"title":"Trusted observed behavior","description":"Expected successful parse, no code execution claim","evidence_ids":[input.value["evidence_id"]]})).await?;
    assert!(!finding.is_error);
    let finding_id = result(&finding)["finding"]["id"].as_str().unwrap();
    let started=service.execute(&verifier.id,"verify-start","poc_verify",json!({"finding_id":finding_id,"target":target,"input":input_path,"trials":2,"expected":{"exit_code":0}})).await?;
    assert!(!started.is_error, "{:?}", started.value);
    let job_id = result(&started)["job_id"].as_str().unwrap();
    let mut completed = false;
    for index in 0..200 {
        let status = service
            .execute(
                &verifier.id,
                &format!("status-{index}"),
                "job_status",
                json!({"job_id":job_id}),
            )
            .await?;
        assert!(!status.is_error, "{:?}", status.value);
        if result(&status)["state"] == "completed" {
            completed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(completed, "live worker verification did not complete");
    let recorded = service
        .execute(
            &verifier.id,
            "record",
            "verification_record",
            json!({"finding_id":finding_id,"job_id":job_id,"impact":"arbitrary code execution"}),
        )
        .await?;
    assert!(!recorded.is_error, "{:?}", recorded.value);
    let record = result(&recorded);
    assert_eq!(record["passed"], true);
    assert_eq!(record["attempts"], 2);
    assert_eq!(record["successes"], 2);
    assert_eq!(
        record["impact"],
        "observed behavior matches the recorded expectation"
    );
    assert!(record["evidence_ids"].as_array().unwrap().len() > 1);
    let promoted=service.execute(&producer.id,"promote","finding_update",json!({"id":finding_id,"stage":"impact_verified","verification_id":record["id"],"impact":"arbitrary code execution"})).await?;
    assert!(!promoted.is_error);
    assert_eq!(
        result(&promoted)["finding"]["impact"],
        "observed behavior matches the recorded expectation"
    );
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn think_is_private_and_severity_is_gated_by_stage() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, _, run, agent) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    // Scratch reasoning is recorded for audit, acknowledged minimally, and
    // never replayed into context.
    let first = service
        .execute(
            &agent.id,
            "think-1",
            "think",
            json!({"content":"partial plan: xref the prompt string"}),
        )
        .await?;
    assert!(!first.is_error);
    assert_eq!(result(&first)["recorded"], true);
    assert_eq!(result(&first)["thoughts"], 1);
    let second = service
        .execute(
            &agent.id,
            "think-2",
            "think",
            json!({"content":"revised plan"}),
        )
        .await?;
    assert_eq!(result(&second)["thoughts"], 2);
    let thoughts = store.list::<Thought>("thought", &run.id)?;
    assert_eq!(thoughts.len(), 2);
    assert!(thoughts.iter().all(|thought| thought.agent_id == agent.id));
    // Real evidence from a static tool, so the severity gate is what we test.
    let evidence = service
        .execute(&agent.id, "evidence", "binary_strings", json!({"limit":5}))
        .await?;
    assert!(!evidence.is_error);
    let evidence_id = evidence.value["evidence_id"].as_str().unwrap().to_owned();
    // Severity is earned by evidence: critical on a bare suspicion is refused.
    let gated = service
        .execute(&agent.id, "gated", "finding_file", json!({"title":"speculation","description":"not reproduced","severity":"critical","stage":"suspected","evidence_ids":[evidence_id]}))
        .await?;
    assert!(gated.is_error);
    assert!(
        result(&gated)["error"]
            .as_str()
            .unwrap()
            .contains("reproduced-or-better")
    );
    // Medium on a reproduced finding is accepted and round-trips.
    let filed = service
        .execute(&agent.id, "filed", "finding_file", json!({"title":"real observation","description":"input reproduced","severity":"medium","severity_rationale":"bare crash only","stage":"reproduced","evidence_ids":[evidence_id]}))
        .await?;
    assert!(!filed.is_error, "{:?}", filed.value);
    assert_eq!(result(&filed)["finding"]["severity"], "medium");
    assert_eq!(
        result(&filed)["finding"]["severity_rationale"],
        "bare crash only"
    );
    service.close().await;
    Ok(())
}
