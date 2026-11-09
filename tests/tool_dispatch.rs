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
use std::{collections::BTreeSet, fs, path::Path, time::Duration};

fn child(coordinator: &Coordinator, root: &str, role: Role, scope: &str) -> Result<AgentSnapshot> {
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
                dependency_policy: Default::default(),
                contract: Default::default(),
            },
        )?
        .agent)
}
fn fixture(path: &Path) -> Result<(Store, Coordinator, RunRecord, AgentSnapshot)> {
    let store = Store::open(path.join("state"))?;
    let coordinator = Coordinator::new(store.clone());
    let target = path.join("sample.bin");
    fs::write(
        &target,
        b"fixture parser string\0another printable string\0",
    )?;
    let run = coordinator.create_run(&target, "inspect", Budget::default(), json!({}))?;
    let agent = child(&coordinator, &run.root_id, Role::Analyst, "recon")?;
    Ok((store, coordinator, run, agent))
}
fn result(outcome: &ToolOutcome) -> &Value {
    &outcome.value["result"]
}

#[tokio::test]
async fn shared_context_is_scoped_bounded_and_exactly_retrievable() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, coordinator, run, agent) = fixture(temp.path())?;
    let unrelated = child(
        &coordinator,
        &run.root_id,
        Role::Analyst,
        "unrelated cryptography",
    )?;
    let before = re_agent::agents::preamble(&agent, &run, &store, None, None)?.len();
    let mut wanted = None;
    for index in 0..30 {
        let own = index < 3;
        let document = SharedDocument {
            id: format!("context-{index}"),
            run_id: run.id.clone(),
            agent_id: if own {
                agent.id.clone()
            } else {
                unrelated.id.clone()
            },
            kind: "note".into(),
            title: format!("{} {index}", if own { "parser" } else { "crypto" }),
            body: format!("{} END_OF_DOCUMENT", "字段😀\n\"\\".repeat(200)),
            amendments: vec![json!({"body":"late correction: marker-amendment"})],
            created_at: now(),
            updated_at: now(),
        };
        if index == 0 {
            wanted = Some(document.clone());
        }
        store.put("note", &document.id, &run.id, &document)?;
    }
    let prompt = re_agent::agents::preamble(&agent, &run, &store, None, None)?;
    let context: Value =
        serde_json::from_str(prompt.split("Assessment context (data):\n").nth(1).unwrap())?;
    let shared = &context["shared_context"];
    assert_eq!(shared["counts"]["note"], 30);
    assert_eq!(shared["selected"].as_array().unwrap().len(), 3);
    assert_eq!(shared["omitted"], 27);
    assert!(
        shared["selected"]
            .as_array()
            .unwrap()
            .iter()
            .all(|doc| doc["agent_id"] == agent.id)
    );
    assert!(!prompt.contains("END_OF_DOCUMENT"));
    assert!(prompt.len() < before + 15000);

    let mut config = AppConfig::default();
    config.tools.max_output_bytes = 1024;
    let service = ToolService::new(config, store.clone()).await?;
    let mut offset = 0;
    let mut found = std::collections::BTreeSet::new();
    loop {
        let page = service.execute(&agent.id, &format!("search-{offset}"), "context_search",
            json!({"query":"marker-amendment","kind":"note","scope":"recon","agent_id":agent.id,"offset":offset,"limit":2})).await?;
        assert!(
            !page.is_error && page.value["truncated"] == false,
            "{page:?}"
        );
        assert_eq!(result(&page)["total"], 3);
        for doc in result(&page)["documents"].as_array().unwrap() {
            assert!(found.insert(doc["id"].as_str().unwrap().to_owned()));
        }
        let Some(next) = result(&page)["next_offset"].as_u64() else {
            break;
        };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(found.len(), 3);
    let wanted = wanted.unwrap();
    let mut offset = 0;
    let mut content = String::new();
    loop {
        let page = service
            .execute(
                &agent.id,
                &format!("read-{offset}"),
                "context_read",
                json!({"id":wanted.id,"kind":"note","offset":offset,"max_chars":12000}),
            )
            .await?;
        assert!(
            !page.is_error && page.value["truncated"] == false,
            "{page:?}"
        );
        content.push_str(result(&page)["content"]["text"].as_str().unwrap());
        let Some(next) = result(&page)["content"]["next_offset"].as_u64() else {
            break;
        };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(
        serde_json::from_str::<Value>(&content)?,
        serde_json::to_value(&wanted)?
    );
    let other_run =
        coordinator.create_run(&run.target_path, "other run", Budget::default(), json!({}))?;
    let denied = service
        .execute(
            &other_run.root_id,
            "context-cross-run",
            "context_read",
            json!({"id":wanted.id,"kind":"note"}),
        )
        .await?;
    assert!(denied.is_error);
    let bad_range = service
        .execute(
            &agent.id,
            "context-invalid-offset",
            "context_read",
            json!({"id":wanted.id,"kind":"note","offset":content.chars().count()+1}),
        )
        .await?;
    assert!(bad_range.is_error);
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn role_catalog_enforces_coordinator_boundary() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, _, run, _agent) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    // Strix-style visibility: the whole catalog is attached from the first
    // request; no model turn is spent discovering or loading tools.
    let analyst: BTreeSet<String> = service
        .definitions(Role::Analyst)
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    for expected in [
        "function_decompile",
        "stage_target",
        "file_put",
        "job_start",
        "job_status",
        "debug_start",
        "interactive_start",
        "fuzz_start",
        "crash_replay",
        "poc_verify",
        "verification_record",
        "coverage_record",
        "finding_file",
        "threat_model_save",
        "angr_solve",
        "binary_strings",
    ] {
        assert!(analyst.contains(expected), "{expected} missing");
    }
    // Lifecycle and target operations follow the role boundary.
    assert!(analyst.contains("agent_finish"));
    assert!(!analyst.contains("finish_scan"));
    let root: BTreeSet<String> = service
        .definitions(Role::Root)
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert!(root.contains("finish_scan"));
    assert!(!root.contains("agent_finish"));
    // Root delegates target operations.
    assert!(!root.contains("binary_inspect"));
    assert!(!root.contains("poc_verify"));
    // (create_agent exclusion from the model toolset is covered by the
    // native kernel test, which inspects the actual request tools.)
    let _ = run;
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn capabilities_preserve_role_lifecycle_permissions_and_total_limit() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, coordinator, run, _) = fixture(temp.path())?;
    let mut request = ChildRequest {
        name: "boundary analyst".into(),
        role: Role::Analyst,
        task: "investigate the parser boundary".into(),
        scope: "parser boundary".into(),
        skills: vec!["reverse".into(), "hunter".into(), "crash".into()],
        inherit_context: false,
        wait_for_predecessors: vec![],
        dependency_policy: Default::default(),
        contract: Default::default(),
    };
    let analyst = coordinator
        .create_child(&run.root_id, request.clone())?
        .agent;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    for (index, name) in ["exploit", "recovery", "recovery"].into_iter().enumerate() {
        let loaded = service
            .execute(
                &analyst.id,
                &format!("cap-{index}"),
                "load_skill",
                json!({"name":name}),
            )
            .await?;
        assert!(!loaded.is_error, "{loaded:?}");
    }
    let current: AgentSnapshot = store.require("agent", &analyst.id)?;
    assert_eq!(current.role, Role::Analyst);
    assert_eq!(current.status, analyst.status);
    assert_eq!(current.active_skills.len(), 2);
    for name in ["root", "analyst", "verifier", "reporter"] {
        let denied = service
            .execute(
                &analyst.id,
                &format!("denied-{name}"),
                "load_skill",
                json!({"name":name}),
            )
            .await?;
        assert!(denied.is_error, "{name}");
    }
    assert_eq!(
        store
            .require::<AgentSnapshot>("agent", &analyst.id)?
            .active_skills
            .len(),
        2
    );
    let denied = service
        .execute(&analyst.id, "self-certify", "poc_verify", json!({}))
        .await?;
    assert!(denied.is_error);
    assert!(
        !result(&denied)["error"]
            .as_str()
            .unwrap()
            .contains("verifier role"),
        "poc_verify is no longer role-gated"
    );

    let report = service
        .execute(
            &run.root_id,
            "report-cap",
            "load_skill",
            json!({"name":"reporter"}),
        )
        .await?;
    assert!(!report.is_error, "{report:?}");
    let root: AgentSnapshot = store.require("agent", &run.root_id)?;
    assert_eq!(root.role, Role::Root);
    assert_eq!(root.status, Status::Queued);
    let root_tools = service
        .definitions(root.role)
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect::<Vec<_>>();
    assert!(root_tools.iter().any(|name| name == "binary_inspect"));
    assert!(!root_tools.iter().any(|name| name == "agent_finish"));
    assert!(root_tools.iter().any(|name| name == "finish_scan"));
    let outcome = service
        .execute(&root.id, "report-target", "binary_inspect", json!({}))
        .await?;
    if outcome.is_error {
        assert!(
            !result(&outcome)["error"]
                .as_str()
                .unwrap()
                .contains("unavailable"),
            "binary_inspect is no longer role-gated"
        );
    }

    request.scope = "independent check".into();
    request.role = Role::Verifier;
    request.skills.clear();
    let verifier = coordinator.create_child(&run.root_id, request)?.agent;
    assert!(
        service
            .definitions(verifier.role)
            .iter()
            .any(|tool| tool.name.as_str() == "poc_verify")
    );
    for old_role in [
        "recon", "reverse", "hunter", "fuzz", "crash", "exploit", "reporter",
    ] {
        let denied = service
            .execute(
                &run.root_id,
                &format!("old-role-{old_role}"),
                "create_agent",
                json!({"name":"invalid","role":old_role,"task":"inspect","scope":"invalid"}),
            )
            .await?;
        assert!(denied.is_error, "{old_role}");
    }
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn finish_result_accepts_a_differing_free_text_summary() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, _, _run, agent) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    // A result plus a different free-text summary must succeed: result.answer
    // is canonical and equality checks only burned retry turns.
    let finished = service
        .execute(
            &agent.id,
            "finish-differing",
            "agent_finish",
            json!({"result":{"outcome":"success","answer":"canonical answer"},"summary":"a different free text summary"}),
        )
        .await?;
    assert!(!finished.is_error, "{finished:?}");
    let recorded: AgentSnapshot = store.require("agent", &agent.id)?;
    assert_eq!(recorded.summary.as_deref(), Some("canonical answer"));
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn exec_command_routes_to_the_runtime_instead_of_unknown_tool() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, _, _run, agent) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    // The catalog advertises exec_command; dispatch must reach the runtime
    // layer (which fails on the disabled backend here) rather than bail
    // with "unknown tool".
    let outcome = service
        .execute(
            &agent.id,
            "shell-route",
            "exec_command",
            json!({"command":"echo hi"}),
        )
        .await?;
    assert!(outcome.is_error);
    let error = result(&outcome)["error"].as_str().unwrap().to_owned();
    assert!(
        error.contains("runtime is disabled") || error.contains("trusted_local"),
        "exec_command did not reach the runtime layer: {error}"
    );
    assert!(!error.contains("unknown tool"), "{error}");
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn poc_verify_budget_enforces_closure_after_two_jobs() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, coordinator, run, agent) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    let verifier = child(&coordinator, &run.root_id, Role::Verifier, "closure check")?;
    let evidence = store.artifact(
        &run.id,
        &agent.id,
        "closure evidence",
        "fixture",
        b"observed behavior",
        "text/plain",
        json!({}),
    )?;
    let filed = service
        .execute(
            &agent.id,
            "closure-finding",
            "finding_file",
            json!({"title":"closure fixture","description":"bounded verification","stage":"suspected","evidence_ids":[evidence.id]}),
        )
        .await?;
    assert!(!filed.is_error);
    let finding_id = result(&filed)["finding"]["id"].as_str().unwrap().to_owned();
    // Two recorded replay jobs exhaust the structural budget.
    for index in 0..2 {
        store.put(
            "job",
            &format!("closure-job-{index}"),
            &run.id,
            &json!({"id":format!("closure-job-{index}"),"run_id":run.id,"agent_id":verifier.id,"worker_id":"fixture","owner_generation":verifier.generation,"call_id":format!("closure-call-{index}"),"worker_request_id":format!("request-{index}"),"parameters":{},"target_sha256":run.target_sha256,"created_at":now(),"updated_at":now(),"tool_name":"poc_verify","finding_id":finding_id,"status":{"job_id":format!("closure-job-{index}"),"state":"completed"}}),
        )?;
    }
    let third = service
        .execute(
            &verifier.id,
            "closure-third",
            "poc_verify",
            json!({"finding_id":finding_id,"target":"files/x","input":"files/y"}),
        )
        .await?;
    assert!(third.is_error);
    assert!(
        result(&third)["error"]
            .as_str()
            .unwrap()
            .contains("exhausted"),
        "third replay must be rejected with closure guidance"
    );
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn activated_skills_are_exact_durable_snapshots_with_idempotent_receipts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, _, run, agent) = fixture(temp.path())?;
    let skills = temp.path().join("skills");
    fs::create_dir(&skills)?;
    let original = "# Custom method\nPreserve this precise instruction.\n".repeat(300);
    fs::write(skills.join("method.md"), &original)?;
    let config = AppConfig {
        skills_dir: Some(skills.clone()),
        ..AppConfig::default()
    };
    let service = ToolService::new(config, store.clone()).await?;
    let activated = service
        .execute(
            &agent.id,
            "activate",
            "load_skill",
            json!({"name":"method"}),
        )
        .await?;
    assert!(!activated.is_error, "{activated:?}");
    assert_eq!(result(&activated)["active"], true);
    assert!(
        activated.value.to_string().len() < 2000,
        "activation must not expose a summarizable copy of the instructions"
    );
    let snapshot = store
        .require::<AgentSnapshot>("agent", &agent.id)?
        .active_skills
        .remove(0);
    assert_eq!(snapshot.content, original);
    assert_eq!(
        store.read_evidence(&run.id, &snapshot.evidence_id)?,
        original.as_bytes()
    );
    fs::write(skills.join("method.md"), "changed on disk")?;
    let replay = service
        .execute(
            &agent.id,
            "activate",
            "load_skill",
            json!({"name":"method"}),
        )
        .await?;
    assert_eq!(activated.value, replay.value);
    let reopened = Store::open(temp.path().join("state"))?;
    assert_eq!(
        reopened
            .require::<AgentSnapshot>("agent", &agent.id)?
            .active_skills[0]
            .content,
        original
    );
    let denied = service
        .execute(&agent.id, "root-role", "load_skill", json!({"name":"root"}))
        .await?;
    assert!(denied.is_error);
    fs::write(skills.join("oversize.md"), "x".repeat(65537))?;
    let oversize = service
        .execute(
            &agent.id,
            "oversize",
            "load_skill",
            json!({"name":"oversize"}),
        )
        .await?;
    assert!(oversize.is_error);
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn model_coordination_checks_dependencies_and_reopens_completed_children() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, coordinator, run, agent) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    let invalid = service
        .execute(
            &run.root_id,
            "cycle",
            "create_agent",
            json!({
                "name":"blocked","role":"analyst","task":"inspect","scope":"new scope",
                "wait_for_predecessors":[run.root_id]
            }),
        )
        .await?;
    assert!(invalid.is_error);
    assert!(
        result(&invalid)["error"]
            .as_str()
            .unwrap()
            .contains("deadlock")
    );
    coordinator.finish(&agent.id, "initial result")?;
    let info = service
        .execute(
            &run.root_id,
            "ordinary-info",
            "send_message_to_agent",
            json!({"agent_id":agent.id,"message":"background context"}),
        )
        .await?;
    assert!(!info.is_error);
    assert_eq!(
        store.require::<AgentSnapshot>("agent", &agent.id)?.status,
        Status::Completed
    );
    let status = service
        .execute(
            &run.root_id,
            "info-status",
            "message_status",
            json!({"message_id":result(&info)["message_id"]}),
        )
        .await?;
    assert!(!status.is_error);
    assert_eq!(result(&status)["envelope"]["delivery"], "queued");
    let args = json!({"agent_id":agent.id,"message":"clarify the root cause"});
    let sent = service
        .execute(&run.root_id, "follow-up", "follow_up_agent", args.clone())
        .await?;
    assert!(!sent.is_error, "{sent:?}");
    service
        .execute(&run.root_id, "follow-up", "follow_up_agent", args)
        .await?;
    let reopened: AgentSnapshot = store.require("agent", &agent.id)?;
    assert_eq!(reopened.status, Status::Queued);
    assert_eq!(reopened.generation, 1);
    assert_eq!(
        store.inbox(&agent.id)?.len(),
        1,
        "replayed message must not reopen twice"
    );
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn closing_lost_or_closed_sessions_is_idempotent_and_reconciles() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, _coordinator, run, owner) = fixture(temp.path())?;
    let mut config = AppConfig::default();
    config.runtime.backend = RuntimeBackend::TrustedLocal;
    config.runtime.state_dir = temp.path().join("worker");
    let service = ToolService::new(config, store.clone()).await?;
    let program = "import sys\nfor line in sys.stdin:\n print('echo:'+line.strip(),flush=True)\n";
    let started = service
        .execute(
            &owner.id,
            "idem:start",
            "interactive_start",
            json!({"argv":["python","-u","-c",program]}),
        )
        .await?;
    assert!(!started.is_error, "{started:?}");
    let session_id = result(&started)["session_id"].as_str().unwrap().to_owned();
    let closed = service
        .execute(
            &owner.id,
            "idem:close",
            "interactive_close",
            json!({"session_id":session_id}),
        )
        .await?;
    assert!(!closed.is_error, "{closed:?}");
    // Closing the already-closed record succeeds idempotently.
    let again = service
        .execute(
            &owner.id,
            "idem:close-again",
            "interactive_close",
            json!({"session_id":session_id}),
        )
        .await?;
    assert!(!again.is_error, "{again:?}");
    let record: Value = store.require("interact_session", &session_id)?;
    assert_eq!(record["state"], "closed");
    // A session the worker lost (daemon restart): the local record says
    // running but the bridge knows nothing; close must reconcile it to lost
    // instead of stranding agent_finish.
    let real: Value = store.require("interact_session", &session_id)?;
    store.put(
        "interact_session",
        "ghost-session",
        &run.id,
        &json!({"id":"ghost-session","run_id":run.id,"agent_id":owner.id,"state":"running","worker_id":real["worker_id"],"created_at":now(),"updated_at":now()}),
    )?;
    let ghost = service
        .execute(
            &owner.id,
            "idem:ghost",
            "interactive_close",
            json!({"session_id":"ghost-session"}),
        )
        .await?;
    assert!(!ghost.is_error, "{ghost:?}");
    let record: Value = store.require("interact_session", "ghost-session")?;
    assert_eq!(record["state"], "lost");
    store.put("debug_session", "ghost-debug", &run.id,
        &json!({"id":"ghost-debug","run_id":run.id,"agent_id":owner.id,"state":"running","worker_id":real["worker_id"],"created_at":now(),"updated_at":now()}))?;
    let debug = service
        .execute(
            &owner.id,
            "idem:ghost-debug",
            "debug_close",
            json!({"session_id":"ghost-debug"}),
        )
        .await?;
    assert!(!debug.is_error, "{debug:?}");
    assert_eq!(
        store.require::<Value>("debug_session", "ghost-debug")?["state"],
        "lost"
    );
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn interactive_tools_execute_and_terminal_owners_can_be_cleaned_up() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, coordinator, run, owner) = fixture(temp.path())?;
    let cleaner = child(&coordinator, &run.root_id, Role::Analyst, "session cleanup")?;
    let mut config = AppConfig::default();
    config.runtime.backend = RuntimeBackend::TrustedLocal;
    config.runtime.state_dir = temp.path().join("worker");
    let runtime_config = config.runtime.clone();
    let service = ToolService::new(config, store.clone()).await?;
    let program = "import sys\nfor line in sys.stdin:\n print('echo:'+line.strip(),flush=True)\n";
    let started = service
        .execute(
            &owner.id,
            "interactive:start",
            "interactive_start",
            json!({"argv":["python","-u","-c",program]}),
        )
        .await?;
    assert!(!started.is_error, "{started:?}");
    let session_id = result(&started)["session_id"].as_str().unwrap();
    let graph = service
        .execute(
            &run.root_id,
            "sessions:graph",
            "view_agent_graph",
            json!({}),
        )
        .await?;
    assert!(
        result(&graph)["open_sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == session_id)
    );
    let sent = service
        .execute(
            &owner.id,
            "interactive:send",
            "interactive_send",
            json!({"session_id":session_id,"data_base64":"aGVsbG8K","read_timeout_secs":3}),
        )
        .await?;
    assert!(!sent.is_error, "{sent:?}");
    assert!(
        result(&sent)["captured"]
            .as_str()
            .unwrap()
            .contains("echo:hello")
    );
    let denied = service
        .execute(
            &cleaner.id,
            "foreign:close",
            "interactive_close",
            json!({"session_id":session_id}),
        )
        .await?;
    assert!(
        denied.is_error,
        "another active agent's process must remain protected"
    );
    assert!(coordinator.finish(&owner.id, "premature").is_err());
    coordinator.stop(&owner.id, "branch stopped")?;
    let denied = service
        .execute(
            &cleaner.id,
            "foreign:send",
            "interactive_send",
            json!({"session_id":session_id,"data_base64":"eA=="}),
        )
        .await?;
    assert!(denied.is_error, "cleanup must not permit sending commands");
    let closed = service
        .execute(
            &cleaner.id,
            "cleanup:close",
            "interactive_close",
            json!({"session_id":session_id}),
        )
        .await?;
    assert!(!closed.is_error, "{closed:?}");
    assert_eq!(
        store.require::<Value>("interact_session", session_id)?["state"],
        "closed"
    );
    let started = service
        .execute(
            &cleaner.id,
            "exited:start",
            "interactive_start",
            json!({"argv":["python","-c","pass"]}),
        )
        .await?;
    assert!(!started.is_error);
    let exited_id = result(&started)["session_id"].as_str().unwrap();
    let mut exited = false;
    for index in 0..20 {
        let output = service
            .execute(
                &cleaner.id,
                &format!("exited:observe:{index}"),
                "interactive_send",
                json!({"session_id":exited_id,"data_base64":"","read_timeout_secs":1}),
            )
            .await?;
        assert!(!output.is_error, "{output:?}");
        if result(&output)["state"] == "exited" {
            exited = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(exited);
    assert!(
        coordinator
            .finish(&cleaner.id, "close even exited leaders")
            .is_err()
    );
    let closed = service
        .execute(
            &cleaner.id,
            "exited:close",
            "interactive_close",
            json!({"session_id":exited_id}),
        )
        .await?;
    assert!(!closed.is_error, "{closed:?}");
    coordinator.finish(&cleaner.id, "sessions closed")?;
    let runtime = re_agent::runtime::RuntimeClient::connect(runtime_config).await?;
    runtime.call("worker.shutdown", json!({})).await?;
    runtime.close().await?;
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn lifecycle_split_is_the_only_role_difference_and_failures_are_evidence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, _, run, agent) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    let root_names = service
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
        "finish_scan",
    ] {
        assert!(
            root_names.iter().any(|entry| entry == name),
            "{name} missing for root"
        );
    }
    assert!(!root_names.iter().any(|entry| entry == "agent_finish"));
    let analyst_names = service
        .definitions(Role::Analyst)
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<Vec<_>>();
    assert!(analyst_names.iter().any(|entry| entry == "agent_finish"));
    assert!(!analyst_names.iter().any(|entry| entry == "finish_scan"));
    // Failure paths still record evidence, whatever the caller's role.
    let failed = service
        .execute(&run.root_id, "root-strings", "binary_strings", json!({}))
        .await?;
    if failed.is_error {
        assert!(
            !result(&failed)["error"]
                .as_str()
                .unwrap()
                .contains("unavailable"),
            "binary_strings is no longer role-gated"
        );
        assert!(
            !store
                .read_evidence(&run.id, failed.value["evidence_id"].as_str().unwrap())?
                .is_empty()
        );
    }
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
        assert!(!error.contains("expected an integer"), "{error}");
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
            .contains("expected an integer")
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
            .contains("at least")
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
    let recipient = child(&coordinator, &run.root_id, Role::Analyst, "reverse")?;
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
    let spawn = json!({"name":"Hunter","role":"analyst","task":"Inspect boundary","scope":"boundary","inherit_context":false});
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
    assert_eq!(store.list::<AgentSnapshot>("agent", &run.id)?.len(), 4);
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
            .contains("not allowed")
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
    let producer_verify = service
        .execute(
            &producer.id,
            "producer-verifies",
            "poc_verify",
            json!({"finding_id":id,"target":"files/x","input":"files/y"}),
        )
        .await?;
    assert!(producer_verify.is_error);
    assert!(
        !result(&producer_verify)["error"]
            .as_str()
            .unwrap()
            .contains("verifier role"),
        "poc_verify is no longer role-gated"
    );
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
async fn mcp_readonly_hints_do_not_authorize_replaying_unknown_side_effects() -> Result<()> {
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
    elif method=='tools/list': result={'tools':[{'name':'mutate','description':'increment once','inputSchema':{'type':'object','properties':{}},'annotations':{'readOnlyHint':True}}]}
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
    let producer = child(&coordinator, &run.root_id, Role::Analyst, "producer")?;
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
async fn think_has_an_agent_scoped_audit_record_and_severity_is_gated_by_stage() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, _, run, agent) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    // Audit records have an agent owner and a small acknowledgment. The
    // original tool-call arguments still belong to the model's history.
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

#[tokio::test]
async fn contracted_tool_tasks_deliver_structured_results_without_vulnerability_findings()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, coordinator, run, _) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    let created=service.execute(&run.root_id,"contract-create","create_agent",json!({"name":"format","role":"analyst","task":"recover header","scope":"contract-format","contract":{"constraints":["static only"],"deliverables":["table"],"completion_conditions":["fields covered"]}})).await?;
    assert!(!created.is_error, "{created:?}");
    let child_id = result(&created)["agent"]["id"].as_str().unwrap();
    let child: AgentSnapshot = store.require("agent", child_id)?;
    let prompt = re_agent::agents::preamble(&child, &run, &store, None, None)?;
    assert!(prompt.contains("static only") && prompt.contains("fields covered"));
    let invalid = service
        .execute(
            child_id,
            "missing-table",
            "agent_finish",
            json!({"summary":"done"}),
        )
        .await?;
    assert!(invalid.is_error);
    let evidence = store.artifact(
        &run.id,
        child_id,
        "contract-table",
        "test.table",
        b"test-only table",
        "text/plain",
        json!({}),
    )?;
    let args = json!({"result":{"outcome":"success","answer":"header recovered","artifacts":[{"name":"table","evidence_id":evidence.id}],"completed_conditions":["fields covered"]}});
    let finished = service
        .execute(child_id, "contract-finish", "agent_finish", args.clone())
        .await?;
    assert!(!finished.is_error, "{finished:?}");
    assert_eq!(
        service
            .execute(child_id, "contract-finish", "agent_finish", args)
            .await?
            .value,
        finished.value
    );
    assert!(store.list::<Finding>("finding", &run.id)?.is_empty());
    assert_eq!(
        store
            .require::<AgentSnapshot>("agent", child_id)?
            .metadata
            .task_result
            .unwrap()
            .answer,
        "header recovered"
    );
    drop(coordinator);
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn conditional_wait_receipts_survive_a_wakeup_without_reparking() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, coordinator, run, agent) = fixture(temp.path())?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    let args = json!({"agent_ids":[agent.id],"policy":"any_success","timeout_seconds":60});
    let parked = service
        .execute(
            &run.root_id,
            "conditional-wait",
            "wait_for_agents",
            args.clone(),
        )
        .await?;
    assert!(!parked.is_error, "{parked:?}");
    assert_eq!(result(&parked)["parked"], true);
    coordinator.finish_result(
        &agent.id,
        TaskResult {
            outcome: TaskOutcome::Inconclusive,
            answer: "not established".into(),
            unknowns: vec!["required observation".into()],
            ..Default::default()
        },
    )?;
    coordinator.wake_waiters(&run.id)?;
    let wake = store
        .inbox(&run.root_id)?
        .into_iter()
        .find(|message| message.sender == "coordinator")
        .unwrap();
    let wake: serde_json::Value = serde_json::from_str(&wake.content)?;
    assert_eq!(wake["condition"]["state"], "blocked");
    let mut call: ToolCallRecord = store.require("call", "conditional-wait")?;
    call.state = CallState::Running;
    call.result = None;
    store.put("call", &call.id, &run.id, &call)?;
    let replay = service
        .execute(&run.root_id, "conditional-wait", "wait_for_agents", args)
        .await?;
    // An incomplete outer ledger rebuilds its evidence envelope, while the
    // transactional business result and wake deadline must stay identical.
    assert_eq!(result(&replay), result(&parked));
    assert_eq!(
        store
            .require::<AgentSnapshot>("agent", &run.root_id)?
            .status,
        Status::Queued
    );
    assert_eq!(
        store
            .events(&run.id, 0, 100)?
            .iter()
            .filter(|event| event.kind == "agent.waiting")
            .count(),
        1
    );
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn general_verification_requires_independent_owned_evidence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, coordinator, run, producer) = fixture(temp.path())?;
    let verifier = child(&coordinator, &run.root_id, Role::Verifier, "check-layout")?;
    let service = ToolService::new(AppConfig::default(), store.clone()).await?;
    let observation = store.artifact(&run.id, &verifier.id, "observe", "test-vector", b"observed 8", "text/plain", json!({}))?;
    let args = json!({"producer_id":producer.id,"claim":"header has 8 bytes","scope":"header", "expected":"8","observed":"8","verdict":"supported","evidence_ids":[observation.id],"counterexamples":[]});
    assert!(service.execute(&producer.id,"invalid-verifier","analysis_verify",args.clone()).await?.is_error);
    let first = service.execute(&verifier.id,"verify-layout","analysis_verify",args.clone()).await?;
    assert!(!first.is_error, "{first:?}");
    let replay = service.execute(&verifier.id,"verify-layout","analysis_verify",args).await?;
    assert_eq!(result(&first),result(&replay));
    let records: Vec<Value> = store.list("analysis_verification", &run.id)?;
    assert_eq!(records.len(),1);
    assert_eq!(records[0]["target_sha256"],run.target_sha256);
    service.close().await;
    Ok(())
}
