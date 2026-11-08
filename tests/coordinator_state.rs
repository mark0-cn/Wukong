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
        role: Role::Analyst,
        task: "inspect exact target".into(),
        scope: scope.into(),
        skills: vec!["recon".into()],
        inherit_context: true,
        wait_for_predecessors: vec![],
        dependency_policy: Default::default(),
        contract: Default::default(),
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
    let first = c
        .create_child(&run.root_id, request("predecessor"))
        .unwrap();
    let mut gated = request("gated child");
    gated.wait_for_predecessors = vec![first.agent.id.clone()];
    let created = c.create_child(&run.root_id, gated).unwrap();
    assert!(created.created);
    let stored: AgentRecord = store.require("agent", &created.agent.id).unwrap();
    assert_eq!(stored.wait_for_predecessors, vec![first.agent.metadata.id]);
    assert_eq!(stored.status, Status::Queued);
}

#[test]
fn predecessor_graph_rejects_ancestors_and_transitive_completion_cycles() {
    let (_dir, store, c, run) = fixture();
    let mut blocked = request("root dependency");
    blocked.wait_for_predecessors = vec![run.root_id.clone()];
    assert!(
        c.create_child(&run.root_id, blocked)
            .unwrap_err()
            .to_string()
            .contains("deadlock")
    );
    let parent = c
        .create_child(&run.root_id, request("parent"))
        .unwrap()
        .agent;
    let mut after_parent = request("after parent");
    after_parent.wait_for_predecessors = vec![parent.id.clone()];
    let sibling = c.create_child(&run.root_id, after_parent).unwrap().agent;
    let mut grandchild = request("indirect cycle");
    grandchild.wait_for_predecessors = vec![sibling.metadata.id];
    assert!(
        c.create_child(&parent.id, grandchild)
            .unwrap_err()
            .to_string()
            .contains("deadlock")
    );
    assert_eq!(
        store.list::<AgentRecord>("agent", &run.id).unwrap().len(),
        3
    );
}

#[test]
fn completed_children_accept_followups_and_notify_again_after_restart() {
    let (dir, store, c, run) = fixture();
    let child = c
        .create_child(&run.root_id, request("follow-up"))
        .unwrap()
        .agent;
    store
        .checkpoint_and_ack(&child.id, json!({"old":"conversation"}), &[])
        .unwrap();
    c.finish(&child.id, "first result").unwrap();
    c.follow_up(&run.root_id, &child.id, "Please clarify the input boundary")
        .unwrap();
    let reopened: AgentRecord = store.require("agent", &child.id).unwrap();
    assert_eq!(reopened.status, Status::Queued);
    assert_eq!(reopened.generation, 1);
    assert!(reopened.checkpoint.is_none());
    assert_eq!(
        reopened.inherited_context,
        Some(json!({"old":"conversation"}))
    );
    assert!(c.finish(&run.root_id, "too early").is_err());
    drop(c);
    drop(store);
    let store = Store::open(dir.path().join("state")).unwrap();
    let c = Coordinator::new(store.clone());
    let mail = store.inbox(&child.id).unwrap();
    store
        .checkpoint_and_ack(
            &child.id,
            json!({"followup":true}),
            &mail.iter().map(|m| m.id).collect::<Vec<_>>(),
        )
        .unwrap();
    c.finish(&child.id, "clarified result").unwrap();
    c.finish(&child.id, "clarified result").unwrap();
    let notices: Vec<_> = store
        .inbox(&run.root_id)
        .unwrap()
        .into_iter()
        .filter(|m| m.sender == child.id)
        .collect();
    assert_eq!(
        notices.len(),
        2,
        "one terminal notice is owed per activation"
    );
    assert!(notices[1].content.contains("clarified result"));
    let replacement = c.create_child(&run.root_id, request("follow-up")).unwrap();
    assert!(replacement.created);
    assert_ne!(replacement.agent.id, child.id);
    c.stop(&replacement.agent.id, "probe complete").unwrap();
    c.stop(&run.root_id, "assessment stopped").unwrap();
    assert!(
        c.message("operator", &child.id, "do not reopen a terminal assessment")
            .is_err()
    );
}

#[test]
fn child_and_root_completion_require_worker_sessions_to_close() {
    let (_dir, store, c, run) = fixture();
    let child = c
        .create_child(&run.root_id, request("sessions"))
        .unwrap()
        .agent;
    for kind in ["debug_session", "interact_session"] {
        let session = json!({"id":kind,"run_id":run.id,"agent_id":child.id,"state":"running"});
        store.put(kind, kind, &run.id, &session).unwrap();
    }
    assert!(
        c.finish(&child.id, "premature")
            .unwrap_err()
            .to_string()
            .contains("sessions remain")
    );
    c.stop(&child.id, "failed worker branch").unwrap();
    let inbox = store.inbox(&run.root_id).unwrap();
    store
        .checkpoint_and_ack(
            &run.root_id,
            json!({}),
            &inbox.iter().map(|m| m.id).collect::<Vec<_>>(),
        )
        .unwrap();
    assert!(
        c.finish(&run.root_id, "premature")
            .unwrap_err()
            .to_string()
            .contains("sessions remain")
    );
    for kind in ["debug_session", "interact_session"] {
        let mut session: serde_json::Value = store.require(kind, kind).unwrap();
        session["state"] = json!("closed");
        store.put(kind, kind, &run.id, &session).unwrap();
    }
    c.finish(&run.root_id, "sessions accounted for").unwrap();
}

#[test]
fn descendant_followup_reopens_completed_ancestors_and_preserves_legacy_records() {
    let (_dir, store, c, run) = fixture();
    let parent = c
        .create_child(&run.root_id, request("parent"))
        .unwrap()
        .agent;
    let child = c
        .create_child(&parent.id, request("nested follow-up"))
        .unwrap()
        .agent;
    let mut legacy = serde_json::to_value(&child).unwrap();
    legacy.as_object_mut().unwrap().remove("generation");
    assert_eq!(
        serde_json::from_value::<AgentRecord>(legacy)
            .unwrap()
            .generation,
        0
    );
    c.finish(&child.id, "initial nested result").unwrap();
    let mail = store.inbox(&parent.id).unwrap();
    store
        .checkpoint_and_ack(
            &parent.id,
            json!({}),
            &mail.iter().map(|m| m.id).collect::<Vec<_>>(),
        )
        .unwrap();
    c.finish(&parent.id, "initial parent result").unwrap();
    c.follow_up("operator", &child.id, "clarify nested result")
        .unwrap();
    for agent_id in [&parent.id, &child.id] {
        let reopened: AgentRecord = store.require("agent", agent_id).unwrap();
        assert_eq!(reopened.status, Status::Queued);
        assert_eq!(reopened.generation, 1);
        assert!(!store.inbox(agent_id).unwrap().is_empty());
    }
    assert!(
        c.finish(&parent.id, "cannot finish before follow-up child")
            .is_err()
    );
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
fn capability_changes_reuse_scope_owner_and_verification_stays_independent() {
    let (_dir, store, coordinator, run) = fixture();
    let mut analysis = request("parser boundary");
    analysis.skills = vec!["reverse".into()];
    let first = coordinator
        .create_child(&run.root_id, analysis.clone())
        .unwrap();
    analysis.skills = vec!["hunter".into(), "crash".into()];
    let reused = coordinator
        .create_child(&run.root_id, analysis.clone())
        .unwrap();
    assert!(!reused.created);
    assert_eq!(first.agent.id, reused.agent.id);

    analysis.role = Role::Verifier;
    analysis.task = "independently check the parser claim".into();
    analysis.skills.clear();
    let verifier = coordinator.create_child(&run.root_id, analysis).unwrap();
    assert!(verifier.created);
    assert_ne!(first.agent.id, verifier.agent.id);
    assert_eq!(verifier.agent.scope, first.agent.scope);

    for role_skill in ["root", "analyst", "verifier"] {
        let mut invalid = request("role injection");
        invalid.skills = vec![role_skill.into()];
        assert!(coordinator.create_child(&run.root_id, invalid).is_err());
    }
    assert_eq!(
        store.list::<AgentRecord>("agent", &run.id).unwrap().len(),
        3
    );
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

#[test]
fn information_preserves_completed_tasks_and_wait_conditions() {
    let (_dir, store, c, run) = fixture();
    let child = c
        .create_child(&run.root_id, request("information"))
        .unwrap()
        .agent;
    c.wait(&run.root_id, std::slice::from_ref(&child.id), 60)
        .unwrap();
    c.message("operator", &run.root_id, "background only")
        .unwrap();
    assert_eq!(c.wake_waiters(&run.id).unwrap(), 0);
    let root: AgentRecord = store.require("agent", &run.root_id).unwrap();
    assert_eq!(root.status, Status::Waiting);
    assert_eq!(root.wait_for, vec![child.id.clone()]);
    c.send(
        "operator",
        &run.root_id,
        "please clarify",
        MessageOptions {
            kind: MessageKind::Question,
            ..Default::default()
        },
    )
    .unwrap();
    let root: AgentRecord = store.require("agent", &run.root_id).unwrap();
    assert_eq!(root.status, Status::Queued);
    assert_eq!(root.wait_for, vec![child.id.clone()]);
    c.finish(&child.id, "first answer").unwrap();
    c.message("operator", &child.id, "background after completion")
        .unwrap();
    let completed: AgentRecord = store.require("agent", &child.id).unwrap();
    assert_eq!(completed.status, Status::Completed);
    assert_eq!(completed.generation, 0);
    c.follow_up("operator", &child.id, "new assignment")
        .unwrap();
    assert_eq!(store.inbox(&child.id).unwrap().len(), 1);
    assert!(
        store
            .transaction(|tx| Store::tx_messages(tx, &run.id))
            .unwrap()
            .iter()
            .any(|message| message.content == "background after completion"
                && message.envelope.delivery == MessageDelivery::Stale)
    );
}

#[test]
fn correlated_replies_track_delivery_and_late_generations_are_audited() {
    let (dir, store, c, run) = fixture();
    let sender = c
        .create_child(&run.root_id, request("question sender"))
        .unwrap()
        .agent;
    let recipient = c
        .create_child(&run.root_id, request("question recipient"))
        .unwrap()
        .agent;
    let question = c
        .send(
            &sender.id,
            &recipient.id,
            "which byte order?",
            MessageOptions {
                kind: MessageKind::Question,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(question.delivery, MessageDelivery::Queued);
    store
        .checkpoint_and_ack(&recipient.id, json!({}), &[question.message_id])
        .unwrap();
    let all = store
        .transaction(|tx| Store::tx_messages(tx, &run.id))
        .unwrap();
    assert_eq!(all[0].envelope.delivery, MessageDelivery::Included);
    let reply = c
        .send(
            &recipient.id,
            &sender.id,
            "little endian",
            MessageOptions {
                kind: MessageKind::Reply,
                reply_to: Some(question.message_id),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(reply.correlation_id, question.correlation_id);
    let all = store
        .transaction(|tx| Store::tx_messages(tx, &run.id))
        .unwrap();
    assert_eq!(all[0].envelope.delivery, MessageDelivery::Replied);
    assert_eq!(all[0].envelope.replied_by, Some(reply.message_id));
    store
        .checkpoint_and_ack(&sender.id, json!({}), &[reply.message_id])
        .unwrap();
    c.finish(&sender.id, "answer recorded").unwrap();
    c.follow_up("operator", &sender.id, "next question")
        .unwrap();
    let stale = c
        .send(
            &recipient.id,
            &sender.id,
            "late old answer",
            MessageOptions {
                kind: MessageKind::Reply,
                reply_to: Some(question.message_id),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(stale.delivery, MessageDelivery::Stale);
    let guarded = c
        .send(
            "operator",
            &sender.id,
            "stale assignment",
            MessageOptions {
                kind: MessageKind::FollowUp,
                recipient_generation: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(guarded.delivery, MessageDelivery::Stale);
    drop(c);
    drop(store);
    let reopened = Store::open(dir.path().join("state")).unwrap();
    assert_eq!(reopened.inbox(&sender.id).unwrap().len(), 1);
    let all = reopened
        .transaction(|tx| Store::tx_messages(tx, &run.id))
        .unwrap();
    assert_eq!(
        all.iter()
            .filter(|m| m.envelope.delivery == MessageDelivery::Stale)
            .count(),
        2
    );
}

#[test]
fn task_contract_requires_real_deliverables_and_preserves_each_generation_result() {
    let (dir, store, c, run) = fixture();
    let evidence = store
        .artifact(
            &run.id,
            &run.root_id,
            "table-fixture",
            "test.table",
            b"test-only field table",
            "text/plain",
            json!({}),
        )
        .unwrap();
    let mut task = request("contracted format");
    task.contract = TaskContract {
        objective: task.task.clone(),
        input_evidence_ids: vec![evidence.id.clone()],
        constraints: vec!["static only".into()],
        deliverables: vec!["field table".into()],
        completion_conditions: vec!["account for each field".into()],
    };
    let child = c.create_child(&run.root_id, task.clone()).unwrap().agent;
    let mut conflicting = task.clone();
    conflicting.contract.deliverables.push("decoder".into());
    assert!(c.create_child(&run.root_id, conflicting).is_err());
    assert!(
        c.finish(&child.id, "prose without required deliverable")
            .is_err()
    );
    let mut answer = TaskResult {
        answer: "fixture answer".into(),
        artifacts: vec![TaskArtifact {
            name: "field table".into(),
            evidence_id: evidence.id.clone(),
        }],
        ..Default::default()
    };
    assert!(c.finish_result(&child.id, answer.clone()).is_err());
    answer.completed_conditions = task.contract.completion_conditions;
    c.finish_result(&child.id, answer).unwrap();
    let completed: AgentRecord = store.require("agent", &child.id).unwrap();
    assert_eq!(
        completed.metadata.task_result.unwrap().outcome,
        TaskOutcome::Success
    );
    assert!(
        store.inbox(&run.root_id).unwrap()[0]
            .content
            .contains("field table")
    );
    c.follow_up("operator", &child.id, "clarify the table")
        .unwrap();
    assert!(
        store
            .require::<AgentRecord>("agent", &child.id)
            .unwrap()
            .task_result
            .is_none()
    );
    drop(c);
    drop(store);
    let reopened = Store::open(dir.path().join("state")).unwrap();
    let history = reopened
        .list::<serde_json::Value>("task_result", &run.id)
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["generation"], 0);
}

#[test]
fn non_success_task_results_explain_the_gap_and_evidence_is_integrity_checked() {
    let (_dir, store, c, run) = fixture();
    let mut task = request("bounded analysis");
    task.contract.deliverables = vec!["decoder".into()];
    let child = c.create_child(&run.root_id, task).unwrap().agent;
    let mut blocked = TaskResult {
        outcome: TaskOutcome::Blocked,
        answer: "more samples needed".into(),
        ..Default::default()
    };
    assert!(c.finish_result(&child.id, blocked.clone()).is_err());
    blocked.unknowns = vec!["variant encoding".into()];
    c.finish_result(&child.id, blocked).unwrap();
    assert_eq!(
        store
            .require::<AgentRecord>("agent", &child.id)
            .unwrap()
            .metadata
            .task_result
            .unwrap()
            .outcome,
        TaskOutcome::Blocked
    );
    let other = c
        .create_child(&run.root_id, request("integrity check"))
        .unwrap()
        .agent;
    let evidence = store
        .artifact(
            &run.id,
            &other.id,
            "tampered-result",
            "test",
            b"original",
            "text/plain",
            json!({}),
        )
        .unwrap();
    fs::write(store.root().join(&evidence.relative_path), b"changed").unwrap();
    assert!(
        c.finish_result(
            &other.id,
            TaskResult {
                answer: "answer".into(),
                evidence_ids: vec![evidence.id],
                ..Default::default()
            }
        )
        .is_err()
    );
    c.finish_result(
        &other.id,
        TaskResult {
            outcome: TaskOutcome::Cancelled,
            answer: "cancelled by scope decision".into(),
            next_actions: vec!["parent to revise scope".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        store
            .require::<AgentRecord>("agent", &other.id)
            .unwrap()
            .status,
        Status::Stopped
    );
}

#[test]
fn failed_requirements_block_successors_and_propagate_without_execution() {
    let (_dir, store, c, run) = fixture();
    let first = c
        .create_child(&run.root_id, request("inconclusive prerequisite"))
        .unwrap()
        .agent;
    let mut task = request("dependent");
    task.wait_for_predecessors = vec![first.id.clone()];
    let second = c.create_child(&run.root_id, task).unwrap().agent;
    let mut task = request("transitive dependent");
    task.wait_for_predecessors = vec![second.id.clone()];
    let third = c.create_child(&run.root_id, task).unwrap().agent;
    c.finish_result(
        &first.id,
        TaskResult {
            outcome: TaskOutcome::Inconclusive,
            answer: "input layout unresolved".into(),
            unknowns: vec!["length width".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(c.release_dependencies(&run.id).unwrap(), 2);
    for id in [&second.id, &third.id] {
        let blocked: AgentRecord = store.require("agent", id).unwrap();
        assert_eq!(blocked.status, Status::Failed);
        assert_eq!(
            blocked.metadata.task_result.unwrap().outcome,
            TaskOutcome::Blocked
        );
        assert!(blocked.checkpoint.is_none());
    }
    assert_eq!(store.inbox(&run.root_id).unwrap().len(), 3);
}

#[test]
fn dependency_policies_distinguish_success_alternatives_and_terminal_collection() {
    let (_dir, store, c, run) = fixture();
    let failed = c
        .create_child(&run.root_id, request("failed alternative"))
        .unwrap()
        .agent;
    let winner = c
        .create_child(&run.root_id, request("winning alternative"))
        .unwrap()
        .agent;
    let mut any = request("any success");
    any.wait_for_predecessors = vec![failed.id.clone(), winner.id.clone()];
    any.dependency_policy = DependencyPolicy::AnySuccess;
    let any = c.create_child(&run.root_id, any).unwrap().agent;
    let mut collect = request("collect failures");
    collect.wait_for_predecessors = vec![failed.id.clone()];
    collect.dependency_policy = DependencyPolicy::AllTerminal;
    let collect = c.create_child(&run.root_id, collect).unwrap().agent;
    c.set_status(&failed.id, Status::Failed, Some("test failure".into()))
        .unwrap();
    c.release_dependencies(&run.id).unwrap();
    assert!(
        !store
            .require::<AgentRecord>("agent", &any.id)
            .unwrap()
            .predecessors_released
    );
    assert!(
        store
            .require::<AgentRecord>("agent", &collect.id)
            .unwrap()
            .predecessors_released
    );
    c.finish(&winner.id, "alternative succeeded").unwrap();
    c.release_dependencies(&run.id).unwrap();
    assert!(
        store
            .require::<AgentRecord>("agent", &any.id)
            .unwrap()
            .predecessors_released
    );
    c.follow_up("operator", &winner.id, "refine the result")
        .unwrap();
    // Admission has consumed a recorded result; a new producer round does not
    // silently cancel an already-admitted consumer.
    c.release_dependencies(&run.id).unwrap();
    assert!(
        store
            .require::<AgentRecord>("agent", &any.id)
            .unwrap()
            .predecessors_released
    );
    let mut exhausted = request("all alternatives failed");
    exhausted.wait_for_predecessors = vec![failed.metadata.id];
    exhausted.dependency_policy = DependencyPolicy::AnySuccess;
    let exhausted = c.create_child(&run.root_id, exhausted).unwrap().agent;
    c.release_dependencies(&run.id).unwrap();
    assert_eq!(
        store
            .require::<AgentRecord>("agent", &exhausted.id)
            .unwrap()
            .status,
        Status::Failed
    );
}

#[test]
fn runtime_waits_share_cycle_detection_with_predecessor_gates() {
    let (_dir, store, c, run) = fixture();
    let a = c
        .create_child(&run.root_id, request("wait A"))
        .unwrap()
        .agent;
    let b = c
        .create_child(&run.root_id, request("wait B"))
        .unwrap()
        .agent;
    c.wait(&a.id, std::slice::from_ref(&b.id), 60).unwrap();
    assert!(
        c.wait(&b.id, std::slice::from_ref(&a.id), 60)
            .unwrap_err()
            .to_string()
            .contains("deadlock")
    );
    let unchanged: AgentRecord = store.require("agent", &b.id).unwrap();
    assert_eq!(unchanged.status, Status::Queued);
    assert!(unchanged.wait_for.is_empty());
    let mut gated = request("wait C");
    gated.wait_for_predecessors = vec![b.id.clone()];
    let gated = c.create_child(&run.root_id, gated).unwrap().agent;
    assert!(
        c.wait(&b.id, std::slice::from_ref(&gated.id), 60)
            .unwrap_err()
            .to_string()
            .contains("deadlock")
    );
}

#[test]
fn waits_report_unsatisfied_requirements_and_wake_on_any_success() {
    let (_dir, store, c, run) = fixture();
    let waiter = c
        .create_child(&run.root_id, request("conditional waiter"))
        .unwrap()
        .agent;
    let failed = c
        .create_child(&run.root_id, request("wait failed"))
        .unwrap()
        .agent;
    let winner = c
        .create_child(&run.root_id, request("wait winner"))
        .unwrap()
        .agent;
    c.set_status(&failed.id, Status::Crashed, Some("test crash".into()))
        .unwrap();
    let rejected = c
        .wait_with_policy(
            &waiter.id,
            std::slice::from_ref(&failed.id),
            60,
            DependencyPolicy::AllSuccess,
        )
        .unwrap();
    assert_eq!(rejected["parked"], false);
    assert_eq!(rejected["condition"]["state"], "blocked");
    let ids = vec![failed.metadata.id, winner.id.clone()];
    assert_eq!(
        c.wait_with_policy(&waiter.id, &ids, 60, DependencyPolicy::AnySuccess)
            .unwrap()["parked"],
        true
    );
    c.finish(&winner.id, "success").unwrap();
    assert_eq!(c.wake_waiters(&run.id).unwrap(), 1);
    let wake = store.inbox(&waiter.id).unwrap();
    let data: serde_json::Value = serde_json::from_str(&wake[0].content).unwrap();
    assert_eq!(data["condition"]["state"], "satisfied");
    assert_eq!(data["policy"], "any_success");
}

#[test]
fn a_missing_dependency_is_not_assumed_successful() {
    let (_dir, store, c, run) = fixture();
    let predecessor = c
        .create_child(&run.root_id, request("missing predecessor"))
        .unwrap()
        .agent;
    let mut task = request("missing dependency consumer");
    task.wait_for_predecessors = vec![predecessor.id.clone()];
    let consumer = c.create_child(&run.root_id, task).unwrap().agent;
    store
        .transaction(|tx| {
            tx.execute(
                "DELETE FROM entities WHERE kind='agent' AND id=?1",
                [&predecessor.id],
            )?;
            Ok(())
        })
        .unwrap();
    c.release_dependencies(&run.id).unwrap();
    let consumer: AgentRecord = store.require("agent", &consumer.id).unwrap();
    assert_eq!(consumer.status, Status::Failed);
    assert!(consumer.metadata.error.unwrap().contains("missing"));
}
