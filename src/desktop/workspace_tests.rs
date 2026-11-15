use super::*;
use anyhow::Result;
use serde_json::json;
#[test]
fn returned_call_tracks_job_outcome_without_duplicate_rows() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("sample");
    std::fs::write(&target, b"desktop projection fixture")?;
    let store = Store::open(dir.path())?;
    let run = crate::coordinator::Coordinator::new(store.clone()).create_run(
        &target,
        "fixture",
        Budget::default(),
        json!({}),
    )?;
    let agent: AgentMetadata = store.load(&run.root_id)?;
    let call = ToolCallRecord {
        id: "replay-call".into(),
        run_id: run.id.clone(),
        agent_id: agent.id.clone(),
        name: "crash_replay".into(),
        arguments: json!({"target":"parser.elf"}),
        state: CallState::Completed,
        side_effecting: true,
        result: Some(json!({"job_id":"replay-job"})),
        evidence_id: None,
        created_at: now(),
        updated_at: now(),
    };
    store.save_record(&call)?;
    store.event(
        &run.id,
        Some(&agent.id),
        "tool.finished",
        json!({"call_id":call.id}),
    )?;
    let mut job: JobRecord = serde_json::from_value(json!({
        "id":"replay-job", "run_id":run.id, "agent_id":agent.id, "worker_id":"fixture", "worker_resource_id":"local-replay-job",
        "owner_generation":agent.generation, "call_id":call.id, "worker_request_id":"request",
        "tool_name":call.name, "finding_id":null, "parameters":{}, "target_sha256":run.target_sha256,
        "status":{"job_id":"replay-job", "state":"running", "started_at":1700000000},
        "status_evidence_id":null, "imported_artifacts":null, "notification_message_id":null,
        "created_at":now(), "updated_at":now()
    }))?;
    store.save_record(&job)?;
    let request = WorkspaceRequest {
        run: Some(run.id.clone()),
        ..Default::default()
    };
    let first = read(&store, 1, &request)?;
    let data = first.data.unwrap();
    assert_eq!(data.active_tools.len(), 1);
    assert_eq!(data.active_tools[0].state, "running");
    assert_eq!(data.transcript.len(), 1);
    let frozen = crate::store::desktop_queries::read(&store, 1, &request)?
        .data
        .unwrap();
    // A terminal job updates the same row even though the request already completed.
    job.status.state = JobState::Completed;
    job.status.result = Some(serde_json::from_value(
        json!({"passed":false,"attempts":2,"successes":1}),
    )?);
    job.status
        .extra
        .insert("finished_at".into(), json!(1700000004.5));
    store.save_record(&job)?;
    let data = read(
        &store,
        1,
        &WorkspaceRequest {
            known_revision: Some(first.revision),
            ..request.clone()
        },
    )?
    .data
    .unwrap();
    assert!(data.active_tools.is_empty());
    assert_eq!(present(frozen).active_tools[0].state, "running");
    assert_eq!(data.transcript.len(), 1);
    let presentation::ChatPart::ToolCall { args, is_error, .. } = &data.transcript[0].content[0]
    else {
        panic!("tool missing")
    };
    assert!(*is_error);
    assert_eq!(args["display"]["duration_ms"], 4500);
    assert_eq!(args["display"]["job"]["successes"], 1);
    // Later status observations must not inflate the worker's duration.
    job.updated_at = now() + chrono::Duration::hours(1);
    store.save_record(&job)?;
    assert_eq!(
        serde_json::to_value(read(&store, 1, &request)?.data)?["transcript"][0]["content"][0]["args"]
            ["display"]["duration_ms"],
        4500
    );
    // A stale generation or mismatched target cannot become current toolbar work.
    job.status.state = JobState::Running;
    job.owner_generation += 1;
    store.save_record(&job)?;
    assert!(
        read(&store, 1, &request)?
            .data
            .unwrap()
            .active_tools
            .is_empty()
    );
    job.owner_generation = agent.generation;
    job.target_sha256 = "other-target".into();
    store.save_record(&job)?;
    assert!(
        read(&store, 1, &request)?
            .data
            .unwrap()
            .active_tools
            .is_empty()
    );
    job.target_sha256 = run.target_sha256.clone();
    job.status.state = JobState::Completed;
    for (result, expected) in [
        (json!({"exit_code":1}), "failed"),
        (json!({"exit_code":139,"passed":true}), "completed"),
    ] {
        job.status.result = Some(serde_json::from_value(result)?);
        store.save_record(&job)?;
        let data = serde_json::to_value(read(&store, 1, &request)?.data)?;
        assert_eq!(
            data["transcript"][0]["content"][0]["args"]["display"]["state"],
            expected
        );
    }
    Ok(())
}
#[test]
fn snapshot_is_scoped_incremental_and_excludes_private_configuration() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("sample");
    std::fs::write(&target, b"desktop projection fixture")?;
    let store = Store::open(dir.path())?;
    let run = crate::coordinator::Coordinator::new(store.clone()).create_run(
        &target,
        "fixture",
        Budget::default(),
        json!({"secret":"excluded"}),
    )?;
    let mut request = WorkspaceRequest {
        run: Some(run.id.clone()),
        ..Default::default()
    };
    let initial = read(&store, 1, &request)?;
    assert!(!serde_json::to_string(&initial)?.contains("secret"));
    request.known_revision = Some(initial.revision);
    assert!(read(&store, 1, &request)?.data.is_none());
    assert!(read(&store, 2, &request)?.data.is_some());
    request.agent = Some("other".into());
    assert!(read(&store, 1, &request).is_err());
    Ok(())
}
