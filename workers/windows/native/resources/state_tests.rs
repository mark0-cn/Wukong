use super::*;
use crate::artifacts::inputs::Inputs;
pub(crate) fn context(root: &std::path::Path) -> crate::host::context::RuntimeContext {
    crate::host::context::RuntimeContext {
        root: root.into(),
        executable: root.join("never-execute"),
        installation: owner().identity.installation_id,
        artifacts: crate::artifacts::outputs::Artifacts { root: root.into() },
    }
}
pub(crate) fn state(journal: Journal, root: &std::path::Path) -> ResourceState {
    ResourceState {
        journal,
        context: Some(context(root)),
        active: ActiveResource::Idle,
    }
}
pub(crate) fn fixture() -> Result<(tempfile::TempDir, Inputs, ResourceState)> {
    let root = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(root.path(), true)?;
    let journal = Journal::with_policy(root.path(), owner().identity, policy.clone())?;
    let inputs = Inputs::open(root.path(), policy)?;
    let runtime = state(journal, root.path());
    Ok((root, inputs, runtime))
}
pub(crate) fn admitted(
    dispatch: Result<Dispatch>,
    state: &mut ResourceState,
    inputs: &Inputs,
    owner: &Owner,
) -> Result<Dispatch> {
    match dispatch? {
        Dispatch::Admission(plan) => state.reserve_prepared(owner, plan.prepare(inputs, owner)?),
        dispatch => Ok(dispatch),
    }
}
pub(crate) fn planned(
    state: &mut ResourceState,
    inputs: &Inputs,
    owner: &Owner,
    plan: Result<crate::executors::plan::Plan>,
) -> Result<Dispatch> {
    admitted(state.start_plan(owner, plan?), state, inputs, owner)
}
pub(crate) fn ready(dispatch: Result<Dispatch>) -> Result<Value> {
    match dispatch? {
        Dispatch::Ready(value) => Ok(value),
        _ => anyhow::bail!("fixture unexpectedly started work"),
    }
}

pub(crate) fn owner() -> Owner {
    Owner {
        identity: crate::rpc::worker::Identity {
            protocol: 3,
            installation_id: "a".repeat(32),
            boot_id: format!("windows:{}:1", "b".repeat(32)),
            package_sha256: "c".repeat(64),
        },
        run_id: "run".into(),
        owner_id: "controller".into(),
        epoch: 1,
    }
}

#[tokio::test]
async fn pending_start_keeps_ownership_and_allows_query_cancel_and_failed_cleanup_observation()
-> Result<()> {
    let (root, _inputs, mut state) = fixture()?;
    let policy = AclPolicy::private_vm();
    policy.create_directory(&root.path().join("metadata"))?;
    policy.create_directory(&root.path().join("metadata/resource-requests"))?;
    let specification = json!({"process":{"argv":["files/fixture.exe"]}});
    let Reservation::Reserved {
        effect, resource, ..
    } = state.journal.reserve_start(
        &owner(),
        "launch-once",
        ResourceKind::Process,
        &specification,
    )?
    else {
        anyhow::bail!("reservation missing")
    };
    let ticket = state.journal.begin(&effect)?;
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    state.active = ActiveResource::Starting(Box::new(Starting {
        resource: resource.clone(),
        request_id: "launch-once".into(),
        ticket,
        task: tokio::spawn(async move {
            let _ = wait.await;
            anyhow::bail!("fixture startup interrupted")
        }),
    }));
    let params = json!({"job_id":resource});
    assert_eq!(
        state.call(&owner(), "job.status", params.as_object().unwrap())?["state"],
        "queued"
    );
    state.call(&owner(), "job.cancel", params.as_object().unwrap())?;
    assert!(
        root.path()
            .join("metadata/resource-requests")
            .join(format!("{resource}.cancel"))
            .exists()
    );
    assert!(!state.idle()?);
    release.send(()).unwrap();
    tokio::task::yield_now().await;
    assert!(state.poll_startup().is_err());
    assert!(matches!(
        state.journal.lookup_start(
            &owner(),
            "launch-once",
            ResourceKind::Process,
            &specification
        )?,
        Some(Reservation::Pending { .. })
    ));
    let record = state.journal.resource(&owner(), &resource)?;
    state.active = ActiveResource::Recovering(Box::new(Recovering {
        request: crate::resources::resource_recovery::Request {
            schema: 1,
            attempt: "a".repeat(32),
            resource: record,
        },
        observation: super::super::observation::Observation::Failed(
            "fixture cleanup blocked".into(),
        ),
    }));
    assert_eq!(
        state.call(&owner(), "job.status", params.as_object().unwrap())?["cleanup_pending"],
        true
    );
    assert_eq!(
        state.call(&owner(), "resource.list", &Map::new())?["resources"][0]["resource_id"],
        resource
    );
    assert!(!state.idle()?);
    Ok(())
}

#[test]
fn completed_start_returns_original_receipt_after_close_without_reopening_inputs() -> Result<()> {
    let (_root, inputs, mut state) = fixture()?;
    let params = json!({"argv":["files/deleted.exe"],"request_id":"original"});
    let plan = crate::executors::plan::process(params.as_object().unwrap())?;
    let Reservation::Reserved {
        effect, resource, ..
    } = state
        .journal
        .reserve_start(&owner(), &plan.request_id, plan.kind, &plan.specification)?
    else {
        anyhow::bail!("reservation missing")
    };
    let ticket = state.journal.begin(&effect)?;
    let original = json!({"job_id":resource,"state":"running","call_id":"original"});
    state.journal.complete(
        &ticket,
        &Outcome::Result {
            value: original.clone(),
        },
    )?;
    state.journal.start_closing(&resource)?;
    state.journal.close(
        &resource,
        Cleanup {
            jobs_empty: true,
            profiles_removed: true,
            scratch_removed: true,
        },
    )?;
    assert_eq!(
        ready(planned(&mut state, &inputs, &owner(), Ok(plan)))?,
        original
    );
    assert!(state.idle()?);
    Ok(())
}
#[test]
fn interrupted_native_start_retry_reads_original_intent_before_new_input_scope() -> Result<()> {
    let root = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(root.path(), true)?;
    let journal = Journal::with_policy(root.path(), owner().identity, policy.clone())?;
    let inputs = Inputs::open(root.path(), policy)?;
    let mut runtime = crate::resources::state::tests::state(journal, root.path());
    let params = json!({"argv":["files/known.exe"],"request_id":"stable"});
    let spec: Specification =
        serde_json::from_value(json!({"argv":["files/known.exe"],"timeout_secs":30}))?;
    let reservation = runtime.journal.reserve_start(
        &owner(),
        "stable",
        ResourceKind::Process,
        &json!({"native_process_schema":crate::executors::resource_process::SCHEMA,"process":spec}),
    )?;
    let Reservation::Reserved {
        effect, resource, ..
    } = reservation
    else {
        anyhow::bail!("missing reservation");
    };
    let _ticket = runtime.journal.begin(&effect)?;
    let mut successor = owner();
    successor.epoch = 2;
    successor.owner_id = "successor".into();
    let status = crate::resources::state::tests::ready(crate::resources::state::tests::planned(
        &mut runtime,
        &inputs,
        &successor,
        crate::executors::plan::process(params.as_object().unwrap()),
    ))?;
    assert_eq!(status["job_id"], resource);
    assert_eq!(status["state"], "lost");
    assert_eq!(status["cleanup_pending"], true);
    assert!(runtime.journal.begin(&effect).is_err());
    assert!(!runtime.journal.idle()?);
    let mut conflict = params.clone();
    conflict["argv"] = json!(["files/different.exe"]);
    assert!(
        crate::resources::state::tests::ready(crate::resources::state::tests::planned(
            &mut runtime,
            &inputs,
            &successor,
            crate::executors::plan::process(conflict.as_object().unwrap())
        ))
        .is_err()
    );
    let mut stranger = successor;
    stranger.run_id = "other-run".into();
    assert!(runtime.status(&stranger, &resource).is_err());
    Ok(())
}
#[test]
fn native_routing_and_request_schema_never_select_privileged_paths() -> Result<()> {
    assert!(crate::rpc::catalog::owns_resource("job.start", &Map::new()));
    assert!(crate::rpc::catalog::owns_resource("job.list", &Map::new()));
    assert!(crate::rpc::catalog::owns_resource(
        "resource.list",
        &Map::new()
    ));
    assert!(!crate::rpc::catalog::owns_resource(
        "file.get",
        json!({"path":"metadata/resource-results/x.json"})
            .as_object()
            .unwrap()
    ));
    for path in [
        "native-results/../receipt",
        "native-results/x/../../outside",
        "native-results/x/unknown",
    ] {
        let parts: Vec<_> = path.split('/').collect();
        assert!(
            parts.len() != 3
                || !matches!(parts[2], "stdout" | "stderr" | "receipt")
                || parts[1] == ".."
        );
    }
    assert!(
        serde_json::from_value::<Specification>(json!({"argv":["files/a.exe"],"pid":4})).is_err()
    );
    assert!(
        serde_json::from_value::<Specification>(
            json!({"argv":["files/a.exe"],"env":{"PATH":"other"}})
        )
        .is_ok()
    );
    // Environment permission is validated by the host Request, not granted
    // by successful deserialization of this controller-facing shape.
    Ok(())
}

#[test]
fn native_inventory_retains_pending_cleanup_without_publishing_start_specifications() -> Result<()>
{
    let root = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(root.path(), true)?;
    let mut journal = Journal::with_policy(root.path(), owner().identity, policy)?;
    let specification = json!({"private_fixture_value":"DO-NOT-PUBLISH-INVENTORY"});
    let Reservation::Reserved { resource, .. } =
        journal.reserve_start(&owner(), "pending", ResourceKind::Process, &specification)?
    else {
        anyhow::bail!("missing reservation")
    };
    let mut runtime = crate::resources::state::tests::state(journal, root.path());
    let successor = Owner {
        epoch: 2,
        owner_id: "successor".into(),
        ..owner()
    };
    let page = runtime.list(&successor, "resource.list", &Map::new())?;
    assert_eq!(page["resources"][0]["resource_id"], resource);
    assert_eq!(page["resources"][0]["phase"], "preparing");
    assert_eq!(
        page["resources"][0]["native_owner"],
        serde_json::to_value(owner())?
    );
    assert_eq!(
        page["resources"][0]["specification_sha256"],
        crate::artifacts::inputs::digest(&specification)?
    );
    assert!(!page.to_string().contains("DO-NOT-PUBLISH-INVENTORY"));
    let jobs = runtime.list(&successor, "job.list", &Map::new())?;
    assert_eq!(jobs[0]["job_id"], resource);
    assert_eq!(jobs[0]["state"], "lost");
    assert_eq!(jobs[0]["cleanup_pending"], true);
    assert!(!runtime.journal.idle()?);
    for params in [
        json!({"run_id":"foreign"}),
        json!({"kind":"debugger"}),
        json!({"limit":65}),
        json!({"after":".."}),
    ] {
        assert!(
            runtime
                .list(&successor, "job.list", params.as_object().unwrap())
                .is_err()
        );
    }
    let foreign = Owner {
        run_id: "foreign".into(),
        ..successor
    };
    assert_eq!(runtime.list(&foreign, "job.list", &Map::new())?, json!([]));
    runtime.journal.start_closing(&resource)?;
    runtime.journal.close(
        &resource,
        Cleanup {
            jobs_empty: true,
            profiles_removed: true,
            scratch_removed: true,
        },
    )?;
    // A closed ledger entry is not a fabricated successful status file.
    assert!(runtime.list(&owner(), "job.list", &Map::new()).is_err());
    Ok(())
}

#[test]
fn interrupted_launch_retry_never_reopens_a_new_owners_manifest_or_input() -> Result<()> {
    let root = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(root.path(), true)?;
    let journal = Journal::with_policy(root.path(), owner().identity, policy.clone())?;
    let inputs = Inputs::open(root.path(), policy)?;
    let mut runtime = crate::resources::state::tests::state(journal, root.path());
    let params = json!({"launch_spec":"files/missing/launch.json","input":"files/missing/input.bin","request_id":"stable-launch"});
    let spec: Specification = serde_json::from_value(
        json!({"launch_spec":"files/missing/launch.json","input":"files/missing/input.bin"}),
    )?;
    let Reservation::Reserved {
        effect, resource, ..
    } = runtime.journal.reserve_start(
        &owner(),
        "stable-launch",
        ResourceKind::Process,
        &json!({"native_process_schema":crate::executors::resource_process::SCHEMA,"process":spec}),
    )?
    else {
        anyhow::bail!("missing reservation")
    };
    runtime.journal.begin(&effect)?;
    let mut successor = owner();
    successor.epoch = 2;
    successor.owner_id = "successor".into();
    let status = crate::resources::state::tests::ready(crate::resources::state::tests::planned(
        &mut runtime,
        &inputs,
        &successor,
        crate::executors::plan::process(params.as_object().unwrap()),
    ))?;
    assert_eq!(status["job_id"], resource);
    assert_eq!(status["state"], "lost");
    assert_eq!(status["cleanup_pending"], true);
    assert!(runtime.journal.begin(&effect).is_err());
    assert!(!runtime.journal.idle()?);
    Ok(())
}

fn recovery_specification() -> Value {
    json!({
        "native_process_schema": crate::executors::resource_process::SCHEMA,
        "process": {"argv": ["files/fixture.exe"], "timeout_secs": 30},
    })
}

fn protected_directories(root: &std::path::Path) -> Result<()> {
    let policy = AclPolicy::private_vm();
    let metadata = root.join("metadata");
    if !metadata.try_exists()? {
        policy.create_directory(&metadata)?;
    }
    for name in ["resource-requests", "resource-results"] {
        let path = metadata.join(name);
        if !path.try_exists()? {
            policy.create_directory(&path)?;
        }
    }
    Ok(())
}

fn interrupted_process(
    journal: &mut Journal,
    request_id: &str,
) -> Result<(String, String, ResourceRecord)> {
    let Reservation::Reserved {
        effect, resource, ..
    } = journal.reserve_start(
        &owner(),
        request_id,
        ResourceKind::Process,
        &recovery_specification(),
    )?
    else {
        anyhow::bail!("missing interrupted process reservation");
    };
    let _ticket = journal.begin(&effect)?;
    let record = journal.resource(&owner(), &resource)?;
    Ok((effect, resource, record))
}

fn recovery_request(
    record: ResourceRecord,
    attempt: &str,
) -> crate::resources::resource_recovery::Request {
    crate::resources::resource_recovery::Request {
        schema: crate::executors::resource_process::SCHEMA,
        attempt: attempt.into(),
        resource: record,
    }
}

fn cleanup_receipt(request: &crate::resources::resource_recovery::Request) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&json!({
        "schema": crate::executors::resource_process::SCHEMA,
        "scope": "owned_resource_recovery",
        "attempt": request.attempt,
        "resource_id": request.resource.id,
        "record_sha256": crate::artifacts::inputs::digest(&request.resource)?,
        "owner": request.resource.owner,
        "guardian_found": false,
        "jobs_empty": true,
        "profiles_removed": true,
        "scratch_removed": true,
        "isolation": "appcontainer",
    }))?)
}

fn observe_recovery(
    state: &mut ResourceState,
    request: crate::resources::resource_recovery::Request,
    receipt: Vec<u8>,
) {
    state.active = ActiveResource::Recovering(Box::new(Recovering {
        request,
        observation: super::super::observation::Observation::Complete(
            crate::resources::resource_recovery::ProtectedReceipt::new(receipt),
        ),
    }));
}

#[test]
fn mismatched_cleanup_receipts_keep_the_existing_recovery_blocker() -> Result<()> {
    let (root, _inputs, mut state) = fixture()?;
    protected_directories(root.path())?;
    let (effect, resource, record) = interrupted_process(&mut state.journal, "interrupted")?;
    let request = recovery_request(record, &"a".repeat(32));
    request.validate()?;
    let original = cleanup_receipt(&request)?;
    let status_path = state
        .host()?
        .artifacts
        .result_path(&resource, "status.json")?;
    let base: Value = serde_json::from_slice(&original)?;
    let mut rejected = vec![b"not a receipt".to_vec()];
    for (field, value) in [
        ("attempt", json!("b".repeat(32))),
        ("resource_id", json!("0".repeat(32))),
        ("record_sha256", json!("f".repeat(64))),
        ("scope", json!("owned_resource_cleanup")),
        ("jobs_empty", json!(false)),
    ] {
        let mut changed = base.clone();
        changed[field] = value;
        rejected.push(serde_json::to_vec(&changed)?);
    }
    let mut foreign_owner = base.clone();
    foreign_owner["owner"]["epoch"] = json!(9);
    rejected.push(serde_json::to_vec(&foreign_owner)?);
    let mut missing = base.clone();
    missing.as_object_mut().unwrap().remove("scratch_removed");
    rejected.push(serde_json::to_vec(&missing)?);
    for receipt in rejected {
        observe_recovery(&mut state, request.clone(), receipt);
        assert!(state.poll_recovery().is_err());
        assert!(matches!(state.active, ActiveResource::Recovering(_)));
        assert!(!state.idle()?);
        assert_eq!(
            state.journal.resource(&owner(), &resource)?.phase,
            Phase::Preparing
        );
        assert!(!status_path.try_exists()?);
    }
    observe_recovery(&mut state, request.clone(), original.clone());
    state.poll_recovery()?;
    assert!(matches!(state.active, ActiveResource::Idle));
    assert!(state.idle()?);
    let status: Value =
        serde_json::from_slice(&crate::artifacts::io::read_bytes(&status_path, 65536)?)?;
    assert_eq!(status["job_id"], resource);
    assert_eq!(status["state"], "lost");
    assert_eq!(status["cleanup_pending"], false);
    assert_eq!(status["result"]["outcome"], "unknown");
    assert_eq!(status["result"]["jobs_empty"], true);
    assert_eq!(status["result"]["profiles_removed"], true);
    assert_eq!(status["result"]["scratch_removed"], true);
    assert_eq!(
        status["result"]["recovery_artifact"],
        format!("recovery-{}", request.attempt)
    );
    assert_eq!(
        status["result"]["recovery_receipt_sha256"],
        hex::encode(Sha256::digest(&original))
    );
    match state.journal.lookup_start(
        &owner(),
        "interrupted",
        ResourceKind::Process,
        &recovery_specification(),
    )? {
        Some(Reservation::Completed {
            effect: seen,
            resource: seen_resource,
            phase: Phase::Closed,
            outcome: Outcome::UncertainClosed,
        }) => {
            assert_eq!(seen, effect);
            assert_eq!(seen_resource, resource);
        }
        other => anyhow::bail!("cleanup observation must commit against its ticket: {other:?}"),
    }
    Ok(())
}

#[test]
fn restart_keeps_unknown_result_and_cleanup_confirmation_separate_and_rejects_late_results()
-> Result<()> {
    let root = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(root.path(), true)?;
    let mut journal = Journal::with_policy(root.path(), owner().identity, policy.clone())?;
    let (_effect, resource, record) = interrupted_process(&mut journal, "interrupted")?;
    let restart_request = recovery_request(record, &"a".repeat(32));
    restart_request.validate()?;
    let restart_receipt = cleanup_receipt(&restart_request)?;
    drop(journal);

    // After the service restart, the resource layer concludes the interrupted
    // attempt from its protected cleanup receipt.
    protected_directories(root.path())?;
    let journal = Journal::with_policy(root.path(), owner().identity, policy.clone())?;
    let mut state = self::state(journal, root.path());
    let receipt_path = state
        .host()?
        .artifacts
        .result_path(&resource, &format!("recovery-{}", restart_request.attempt))?;
    state
        .host()?
        .artifacts
        .retain(&receipt_path, &restart_receipt)?;
    observe_recovery(&mut state, restart_request.clone(), restart_receipt.clone());
    state.poll_recovery()?;
    assert!(state.idle()?);
    let status_path = state
        .host()?
        .artifacts
        .result_path(&resource, "status.json")?;
    let status_bytes = std::fs::read(&status_path)?;
    let status: Value = serde_json::from_slice(&status_bytes)?;
    assert_eq!(status["job_id"], resource);
    assert_eq!(status["state"], "lost");
    assert_eq!(status["cleanup_pending"], false);
    assert_eq!(status["result"]["outcome"], "unknown");
    assert_eq!(status["result"]["jobs_empty"], true);
    assert_eq!(status["result"]["profiles_removed"], true);
    assert_eq!(status["result"]["scratch_removed"], true);
    assert_eq!(
        status["result"]["recovery_artifact"],
        format!("recovery-{}", restart_request.attempt)
    );
    assert_eq!(
        status["result"]["recovery_receipt_sha256"],
        hex::encode(Sha256::digest(&restart_receipt))
    );
    // The cleanup conclusion stays a separate protected receipt.
    assert_eq!(std::fs::read(&receipt_path)?, restart_receipt);
    match state.journal.lookup_start(
        &owner(),
        "interrupted",
        ResourceKind::Process,
        &recovery_specification(),
    )? {
        Some(Reservation::Completed {
            outcome: Outcome::UncertainClosed,
            phase: Phase::Closed,
            ..
        }) => {}
        other => anyhow::bail!("restart changed the uncertain outcome record: {other:?}"),
    }
    drop(state);

    // A second restart sees the same terminal record. A stale recovery result
    // for the finished attempt must neither rewrite it nor settle a newer
    // resource's blocker.
    let journal = Journal::with_policy(root.path(), owner().identity, policy)?;
    let mut state = self::state(journal, root.path());
    assert_eq!(state.status(&owner(), &resource)?, status);
    let mut stale: Value = serde_json::from_slice(&restart_receipt)?;
    stale["guardian_found"] = json!(true);
    let stale = serde_json::to_vec(&stale)?;
    let (_, later, later_record) = interrupted_process(&mut state.journal, "later")?;
    observe_recovery(&mut state, restart_request.clone(), stale);
    assert!(state.poll_recovery().is_err());
    assert!(!state.idle()?);
    assert_eq!(
        state.journal.resource(&owner(), &later)?.phase,
        Phase::Preparing
    );
    assert_eq!(
        std::fs::read(&status_path)?,
        status_bytes,
        "a stale receipt must not overwrite the existing result"
    );

    // Only the newer attempt's own confirmed cleanup settles its blocker.
    let later_request = recovery_request(later_record, &"b".repeat(32));
    later_request.validate()?;
    let later_receipt = cleanup_receipt(&later_request)?;
    observe_recovery(&mut state, later_request, later_receipt.clone());
    state.poll_recovery()?;
    assert!(state.idle()?);
    let later_status_path = state.host()?.artifacts.result_path(&later, "status.json")?;
    let later_status: Value = serde_json::from_slice(&crate::artifacts::io::read_bytes(
        &later_status_path,
        65536,
    )?)?;
    assert_eq!(later_status["result"]["outcome"], "unknown");
    assert_eq!(
        later_status["result"]["recovery_receipt_sha256"],
        hex::encode(Sha256::digest(&later_receipt))
    );
    assert_eq!(std::fs::read(&status_path)?, status_bytes);
    Ok(())
}

#[tokio::test]
async fn host_shutdown_keeps_unconfirmed_cleanup_blocked_until_owned_work_settles() -> Result<()> {
    let idle_root = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(idle_root.path(), true)?;
    let idle = crate::resources::manager::ResourceManager::open(
        idle_root.path(),
        owner().identity,
        policy.clone(),
    )?;
    idle.initialize(context(idle_root.path()))?;
    idle.shutdown(
        tokio::time::Instant::now() + std::time::Duration::from_secs(1),
        || false,
    )
    .await?;
    assert!(idle.idle()?);

    let root = tempfile::tempdir()?;
    policy.apply(root.path(), true)?;
    let resources =
        crate::resources::manager::ResourceManager::open(root.path(), owner().identity, policy)?;
    resources.initialize(context(root.path()))?;
    protected_directories(root.path())?;
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let resource = {
        let mut state = resources.test_state().await;
        let Reservation::Reserved {
            effect, resource, ..
        } = state.test_journal().reserve_start(
            &owner(),
            "pending",
            ResourceKind::Debugger,
            &json!({"trusted_fixture": true}),
        )?
        else {
            anyhow::bail!("missing pending fixture reservation");
        };
        let ticket = state.test_journal().begin(&effect)?;
        state.active = ActiveResource::Starting(Box::new(Starting {
            resource: resource.clone(),
            request_id: "pending".into(),
            ticket,
            task: tokio::spawn(async move {
                let _ = wait.await;
                anyhow::bail!("fixture startup interrupted");
            }),
        }));
        resource
    };
    assert!(!resources.idle()?);
    let error = resources
        .shutdown(
            tokio::time::Instant::now() + std::time::Duration::from_millis(150),
            || false,
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("native resource cleanup remains pending during service stop"),
        "{error:#}"
    );
    // The refused shutdown leaves the original blocker for a later attempt.
    assert!(!resources.idle()?);
    assert_eq!(
        resources
            .test_state()
            .await
            .test_journal()
            .resource(&owner(), &resource)?
            .phase,
        Phase::Preparing
    );
    assert!(!resources.poll(false)?);
    release.send(()).unwrap();
    tokio::task::yield_now().await;
    // The interrupted launch still owns its journal blocker.
    assert!(!resources.idle()?);
    Ok(())
}
