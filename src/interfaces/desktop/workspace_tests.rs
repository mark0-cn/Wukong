use super::*;
use crate::domain::{AgentMetadata, Budget, CallState, JobRecord, JobState, ToolCallRecord, now};
use anyhow::Result;
use serde_json::json;
#[test]
fn returned_call_tracks_job_outcome_without_duplicate_rows() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("sample");
    std::fs::write(&target, b"desktop projection fixture")?;
    let store = Store::open(dir.path())?;
    let run = crate::application::coordinator::Coordinator::new(store.clone()).create_run(
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
    let frozen = AssessmentQueries::new(&store)
        .workspace_snapshot(1, &request)?
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
    let run = crate::application::coordinator::Coordinator::new(store.clone()).create_run(
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

fn stage_result_job(
    store: &Store,
    run: &crate::domain::RunRecord,
    agent: &AgentMetadata,
) -> Result<JobRecord> {
    let call = ToolCallRecord {
        id: "stage-call".into(),
        run_id: run.id.clone(),
        agent_id: agent.id.clone(),
        name: "stage".into(),
        arguments: json!({"target":"sample"}),
        state: CallState::Completed,
        side_effecting: false,
        result: Some(json!({"job_id":"stage-job"})),
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
    let job: JobRecord = serde_json::from_value(json!({
        "id":"stage-job", "run_id":run.id, "agent_id":agent.id, "worker_id":"fixture",
        "worker_resource_id":"local-stage-job", "owner_generation":agent.generation,
        "call_id":call.id, "worker_request_id":"request", "tool_name":call.name,
        "finding_id":null, "parameters":{}, "target_sha256":run.target_sha256,
        "status":{"job_id":"stage-job","state":"completed","started_at":1700000000,"finished_at":1700000002,
            "cleanup_pending":true,
            "result":{"exit_code":0,"passed":true,"attempts":1,"successes":1},
            "error":"worker still owns child processes; cleanup reconciliation is required"},
        "status_evidence_id":null, "execution_status_evidence_id":"execution-evidence",
        "imported_artifacts":null, "notification_message_id":null,
        "created_at":now(), "updated_at":now()
    }))?;
    store.save_record(&job)?;
    Ok(job)
}

fn staged_job_display(response: &serde_json::Value) -> &serde_json::Value {
    &response["transcript"][0]["content"][0]["args"]["display"]["job"]
}

#[test]
fn finished_execution_with_pending_cleanup_survives_a_persisted_refresh() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("sample");
    std::fs::write(&target, b"desktop cleanup fixture")?;
    let state = dir.path().join("state");
    let store = Store::open(&state)?;
    let run = crate::application::coordinator::Coordinator::new(store.clone()).create_run(
        &target,
        "fixture",
        Budget::default(),
        json!({}),
    )?;
    let agent: AgentMetadata = store.load(&run.root_id)?;
    let job = stage_result_job(&store, &run, &agent)?;
    let request = WorkspaceRequest {
        run: Some(run.id.clone()),
        ..Default::default()
    };
    let first = read(&store, 1, &request)?;
    let revision = first.revision.clone();
    let view = serde_json::to_value(first.data)?;
    let display = staged_job_display(&view);
    // The execution result and the unresolved cleanup are separate facts.
    assert_eq!(display["state"], "completed");
    assert_eq!(display["execution"]["finished"], true);
    assert_eq!(display["execution"]["outcome"], "known");
    assert_eq!(display["execution"]["exit_code"], 0);
    assert_eq!(display["execution"]["evidence_id"], "execution-evidence");
    assert_eq!(display["cleanup"]["status"], "pending");
    assert!(display["cleanup"]["evidence_id"].is_null());
    assert_eq!(
        display["blocking"]["reason"]["code"],
        "progress-job-blocked-cleanup"
    );
    assert!(
        display["blocking"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("cleanup reconciliation"))
    );
    assert_eq!(display["evidence"][0]["role"], "execution");
    assert_eq!(display["evidence"][0]["evidence_id"], "execution-evidence");
    drop(store);

    // A restart/refresh rebuilds the row from persisted records, not memory.
    let reopened = Store::open(&state)?;
    let refreshed = serde_json::to_value(read(&reopened, 1, &request)?.data)?;
    assert_eq!(staged_job_display(&refreshed), display);

    // Confirmed cleanup links its own final evidence while the imported
    // artifacts are still pending; the blocker changes, it does not disappear.
    let mut settled = job;
    settled.status.cleanup_pending = Some(false);
    settled.status.extra.remove("error");
    settled.status_evidence_id = Some("cleanup-evidence".into());
    settled.updated_at = now();
    reopened.save_record(&settled)?;
    let confirmed = read(
        &reopened,
        1,
        &WorkspaceRequest {
            known_revision: Some(revision),
            ..request.clone()
        },
    )?;
    let confirmed_revision = confirmed.revision.clone();
    let view = serde_json::to_value(confirmed.data)?;
    let display = staged_job_display(&view);
    assert_eq!(display["execution"]["finished"], true);
    assert_eq!(display["cleanup"]["status"], "confirmed");
    assert_eq!(display["cleanup"]["evidence_id"], "cleanup-evidence");
    assert_eq!(
        display["blocking"]["reason"]["code"],
        "progress-job-blocked-evidence-pending"
    );
    let roles = display["evidence"].as_array().expect("evidence links");
    assert!(roles.iter().any(|link| link["role"] == "execution"));
    assert!(
        roles
            .iter()
            .any(|link| link["role"] == "cleanup" && link["evidence_id"] == "cleanup-evidence")
    );

    // Once evidence is imported the row is no longer blocked and links artifacts.
    let mut imported = settled;
    imported.imported_artifacts = Some(json!({"job_id":"stage-job","artifacts":[
        {"artifact":{"path":"artifact.bin","sha256":"a".repeat(64),"size":1},"evidence_id":"artifact-evidence"}
    ]}));
    imported.updated_at = now();
    reopened.save_record(&imported)?;
    let ready = read(
        &reopened,
        1,
        &WorkspaceRequest {
            known_revision: Some(confirmed_revision),
            ..request.clone()
        },
    )?
    .data
    .expect("incremental snapshot after artifact import");
    let ready = serde_json::to_value(ready)?;
    let display = staged_job_display(&ready);
    assert_eq!(display["cleanup"]["status"], "confirmed");
    assert!(display["blocking"].is_null());
    assert!(
        display["evidence"]
            .as_array()
            .expect("evidence links")
            .iter()
            .any(|link| link["role"] == "artifact" && link["evidence_id"] == "artifact-evidence")
    );
    Ok(())
}
