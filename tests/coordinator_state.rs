use re_agent::{
    coordinator::{ChildRequest, Coordinator},
    domain::*,
    store::Store,
};
use serde_json::json;
use std::{fs, sync::Arc};

fn fixture() -> (tempfile::TempDir, Store, Coordinator, RunRecord) {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.bin");
    fs::write(&target, b"state-machine fixture; never executed").unwrap();
    let store = Store::open(dir.path().join("state")).unwrap();
    let coordinator = Coordinator::new(store.clone());
    let run = coordinator
        .create_run(&target, "assess fixture", Budget::default(), json!({}))
        .unwrap();
    (dir, store, coordinator, run)
}
fn request(scope: &str) -> ChildRequest {
    ChildRequest {
        name: "Recon".into(),
        role: Role::Recon,
        task: "inspect exact target".into(),
        scope: scope.into(),
        skills: vec!["recon".into()],
        inherit_context: true,
        wait_for_predecessors: vec![],
    }
}

#[test]
fn wait_for_predecessors_is_validated_and_persisted() {
    let (_dir, store, c, run) = fixture();
    let mut blocked = request("gated child");
    blocked.wait_for_predecessors = vec!["agent-does-not-exist".into()];
    let error = c.create_child(&run.root_id, blocked).unwrap_err();
    assert!(
        format!("{error:#}").contains("wait_for_predecessors target missing"),
        "{error:#}"
    );
    let first = c.create_child(&run.root_id, request("predecessor")).unwrap();
    let mut gated = request("gated child");
    gated.wait_for_predecessors = vec![first.agent.id.clone()];
    let created = c.create_child(&run.root_id, gated).unwrap();
    assert!(created.created);
    let stored: AgentRecord = store.require("agent", &created.agent.id).unwrap();
    assert_eq!(stored.wait_for_predecessors, vec![first.agent.id]);
    assert_eq!(stored.status, Status::Queued);
}

#[test]
fn child_creation_is_immediate_durable_and_scope_deduplicated() {
    let (dir, store, c, run) = fixture();
    let first = c
        .create_child(&run.root_id, request("input parser"))
        .unwrap();
    assert!(first.created);
    assert_eq!(first.agent.status, Status::Queued);
    let second = c
        .create_child(&run.root_id, request("input parser"))
        .unwrap();
    assert!(!second.created);
    assert_eq!(first.agent.id, second.agent.id);
    assert!(c.finish(&run.root_id, "too early").is_err());
    drop(c);
    drop(store);
    let reopened = Store::open(dir.path().join("state")).unwrap();
    let child: AgentRecord = reopened.require("agent", &first.agent.id).unwrap();
    assert_eq!(child.parent_id.as_deref(), Some(run.root_id.as_str()));
    assert_eq!(child.scope, "input parser");
}

#[test]
fn finish_scan_waits_for_running_worker_jobs() {
    let (_dir, store, coordinator, run) = fixture();
    let job = json!({"id":"job-1","run_id":run.id,"status":{"state":"running"}});
    store.put("job", "job-1", &run.id, &job).unwrap();
    let error = coordinator.finish(&run.root_id, "done").unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("jobs remain"), "{message}");
    // Polling job_status persists terminal states; only then can the root
    // account for the job and finish.
    let mut finished_job = job;
    finished_job["status"]["state"] = json!("completed");
    store.put("job", "job-1", &run.id, &finished_job).unwrap();
    coordinator.finish(&run.root_id, "done").unwrap();
    assert_eq!(
        store.require::<RunRecord>("run", &run.id).unwrap().status,
        Status::Completed
    );
}

#[test]
fn concurrent_spawns_have_a_single_scope_owner() {
    let (_dir, store, c, run) = fixture();
    let c = Arc::new(c);
    let threads = (0..8)
        .map(|_| {
            let c = c.clone();
            let parent = run.root_id.clone();
            std::thread::spawn(move || c.create_child(&parent, request("same scope")).unwrap())
        })
        .collect::<Vec<_>>();
    let results = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.created).count(), 1);
    assert_eq!(
        store.list::<AgentRecord>("agent", &run.id).unwrap().len(),
        2
    );
}

#[test]
fn concurrent_mailbox_delivery_and_checkpoint_ack_survive_restart() {
    let (dir, store, c, run) = fixture();
    let child = c
        .create_child(&run.root_id, request("mailbox"))
        .unwrap()
        .agent;
    let c = Arc::new(c);
    let threads = (0..6)
        .map(|sender| {
            let c = c.clone();
            let recipient = child.id.clone();
            std::thread::spawn(move || {
                for n in 0..20 {
                    c.message("operator", &recipient, &format!("message {sender}:{n}"))
                        .unwrap();
                }
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }
    let inbox = store.inbox(&child.id).unwrap();
    assert_eq!(inbox.len(), 120);
    let ack = inbox.iter().take(60).map(|m| m.id).collect::<Vec<_>>();
    store
        .checkpoint_and_ack(&child.id, json!({"delivered":ack}), &ack)
        .unwrap();
    drop(c);
    drop(store);
    let reopened = Store::open(dir.path().join("state")).unwrap();
    assert_eq!(reopened.inbox(&child.id).unwrap().len(), 60);
    let child: AgentRecord = reopened.require("agent", &child.id).unwrap();
    assert_eq!(
        child.checkpoint.unwrap()["delivered"]
            .as_array()
            .unwrap()
            .len(),
        60
    );
}

#[test]
fn wait_wakes_on_terminal_notification_and_completion_deduplicates() {
    let (_dir, store, c, run) = fixture();
    let child = c.create_child(&run.root_id, request("wait")).unwrap().agent;
    // A large conversation checkpoint must never leak into the parent's
    // wake-up message.
    let mut child_record: AgentRecord = store.require("agent", &child.id).unwrap();
    child_record.checkpoint = Some(json!({"transcript": "giant-checkpoint-marker-".repeat(2000)}));
    store
        .put("agent", &child.id, &run.id, &child_record)
        .unwrap();
    assert_eq!(
        c.wait(&run.root_id, std::slice::from_ref(&child.id), 60)
            .unwrap()["parked"],
        true
    );
    c.finish(&child.id, "inspection complete").unwrap();
    c.finish(&child.id, "inspection complete").unwrap();
    assert_eq!(
        store
            .inbox(&run.root_id)
            .unwrap()
            .iter()
            .filter(|m| m.sender == child.id)
            .count(),
        1
    );
    assert_eq!(c.wake_waiters(&run.id).unwrap(), 1);
    let wake_message = store
        .inbox(&run.root_id)
        .unwrap()
        .into_iter()
        .find(|m| m.sender == "coordinator")
        .expect("wait_finished coordinator message");
    assert!(
        !wake_message.content.contains("giant-checkpoint-marker"),
        "checkpoint data leaked into the parent context: {}",
        wake_message.content.len()
    );
    let root: AgentRecord = store.require("agent", &run.root_id).unwrap();
    assert_eq!(root.status, Status::Queued);
    assert!(c.finish(&run.root_id, "unread child results").is_err());
    let messages = store.inbox(&run.root_id).unwrap();
    store
        .checkpoint_and_ack(
            &run.root_id,
            json!({"delivered":messages}),
            &messages.iter().map(|m| m.id).collect::<Vec<_>>(),
        )
        .unwrap();
    c.finish(&run.root_id, "all work reconciled").unwrap();
    assert_eq!(
        store.require::<RunRecord>("run", &run.id).unwrap().status,
        Status::Completed
    );
}

#[test]
fn waiting_for_an_ancestor_and_cross_run_messages_are_rejected() {
    let (_dir, store, c, run) = fixture();
    let child = c
        .create_child(&run.root_id, request("cycle"))
        .unwrap()
        .agent;
    assert!(
        c.wait(&child.id, std::slice::from_ref(&run.root_id), 60)
            .is_err()
    );
    let other = c
        .create_run(
            &run.target_path,
            "separate run",
            Budget::default(),
            json!({}),
        )
        .unwrap();
    assert!(c.message(&child.id, &other.root_id, "cross run").is_err());
    assert!(store.inbox(&other.root_id).unwrap().is_empty());
}

#[test]
fn stop_propagates_and_terminal_state_cannot_be_overwritten() {
    let (_dir, store, c, run) = fixture();
    let child = c
        .create_child(&run.root_id, request("child"))
        .unwrap()
        .agent;
    let grandchild = c
        .create_child(&child.id, request("grandchild"))
        .unwrap()
        .agent;
    assert!(c.finish(&child.id, "has unfinished child").is_err());
    assert_eq!(c.stop(&child.id, "operator cancellation").unwrap().len(), 2);
    assert_eq!(
        store
            .require::<AgentRecord>("agent", &grandchild.id)
            .unwrap()
            .status,
        Status::Stopped
    );
    assert!(c.set_status(&child.id, Status::Running, None).is_err());
    assert!(c.finish(&child.id, "stale worker").is_err());
}

#[test]
fn budget_pause_is_distinct_and_resumable() {
    let (_dir, store, c, run) = fixture();
    let child = c
        .create_child(&run.root_id, request("budget"))
        .unwrap()
        .agent;
    c.pause_budget(&run.id, "configured token limit").unwrap();
    assert_eq!(
        store
            .require::<AgentRecord>("agent", &child.id)
            .unwrap()
            .status,
        Status::BudgetPaused
    );
    let mut budget = run.budget.clone();
    budget.max_total_tokens += 100;
    c.resume(&run.id, Some(budget)).unwrap();
    assert_eq!(
        store
            .require::<RunRecord>("run", &run.id)
            .unwrap()
            .budget
            .max_total_tokens,
        run.budget.max_total_tokens + 100
    );
    assert_eq!(
        store
            .require::<AgentRecord>("agent", &child.id)
            .unwrap()
            .status,
        Status::Queued
    );
}

#[test]
fn unresolved_coverage_prevents_clean_finish() {
    let (_dir, store, c, run) = fixture();
    let coverage = Coverage {
        id: id("cov"),
        run_id: run.id.clone(),
        agent_id: run.root_id.clone(),
        scope: "unverified parser".into(),
        kind: "dynamic".into(),
        outcome: "needs_follow_up".into(),
        notes: "runtime unavailable".into(),
        evidence_ids: vec![],
        created_at: now(),
        updated_at: now(),
    };
    store
        .put("coverage", &coverage.id, &run.id, &coverage)
        .unwrap();
    assert!(c.finish(&run.root_id, "clean").is_err());
    let mut resolved = coverage;
    resolved.outcome = "inconclusive".into();
    store
        .put("coverage", &resolved.id, &run.id, &resolved)
        .unwrap();
    c.finish(&run.root_id, "partial; dynamic parser not tested")
        .unwrap();
}

#[test]
fn artifact_integrity_and_cross_run_references_are_checked() {
    let (_dir, store, c, run) = fixture();
    let evidence = store
        .artifact(
            &run.id,
            &run.root_id,
            "call",
            "fixture",
            b"full output",
            "text/plain",
            json!({}),
        )
        .unwrap();
    assert_eq!(
        store.read_evidence(&run.id, &evidence.id).unwrap(),
        b"full output"
    );
    let other = c
        .create_run(&run.target_path, "other", Budget::default(), json!({}))
        .unwrap();
    assert!(store.read_evidence(&other.id, &evidence.id).is_err());
    fs::write(store.root().join(&evidence.relative_path), b"tampered").unwrap();
    assert!(store.read_evidence(&run.id, &evidence.id).is_err());
}

#[test]
fn live_controller_lease_excludes_a_second_process() {
    let (dir, store, _c, run) = fixture();
    let second = Store::open(dir.path().join("state")).unwrap();
    assert!(store.acquire_lease(&run.id, "controller-one", 30).unwrap());
    assert!(!second.acquire_lease(&run.id, "controller-two", 30).unwrap());
    store.release_lease(&run.id, "controller-one").unwrap();
    assert!(second.acquire_lease(&run.id, "controller-two", 30).unwrap());
}

#[test]
fn terminal_run_status_survives_late_pause_and_stop() {
    let (_dir, store, c, run) = fixture();
    c.finish(&run.root_id, "all work complete").unwrap();
    c.pause_budget(&run.id, "late interrupt").unwrap();
    assert_eq!(
        store.require::<RunRecord>("run", &run.id).unwrap().status,
        Status::Completed
    );
    assert!(c.stop(&run.root_id, "late stop").unwrap().is_empty());
    assert_eq!(
        store.require::<RunRecord>("run", &run.id).unwrap().status,
        Status::Completed
    );
    assert!(c.resume(&run.id, None).is_err());

    let stopped = c
        .create_run(
            &run.target_path,
            "stopped fixture",
            Budget::default(),
            json!({}),
        )
        .unwrap();
    c.stop(&stopped.root_id, "operator cancellation").unwrap();
    c.pause_budget(&stopped.id, "late interrupt").unwrap();
    assert_eq!(
        store
            .require::<RunRecord>("run", &stopped.id)
            .unwrap()
            .status,
        Status::Stopped
    );
}

#[test]
fn pause_resume_preserves_an_unfinished_wait() {
    let (_dir, store, c, run) = fixture();
    let child = c
        .create_child(&run.root_id, request("paused wait"))
        .unwrap()
        .agent;
    c.wait(&run.root_id, std::slice::from_ref(&child.id), 60)
        .unwrap();
    let before: AgentRecord = store.require("agent", &run.root_id).unwrap();
    c.pause_budget(&run.id, "operator pause").unwrap();
    c.resume(&run.id, None).unwrap();
    let resumed: AgentRecord = store.require("agent", &run.root_id).unwrap();
    assert_eq!(resumed.status, Status::Waiting);
    assert_eq!(resumed.wait_for, before.wait_for);
    assert_eq!(resumed.wait_until, before.wait_until);
    assert_eq!(c.wake_waiters(&run.id).unwrap(), 0);
    c.finish(&child.id, "child complete").unwrap();
    assert_eq!(c.wake_waiters(&run.id).unwrap(), 1);
    assert_eq!(
        store
            .require::<AgentRecord>("agent", &run.root_id)
            .unwrap()
            .status,
        Status::Queued
    );
}
