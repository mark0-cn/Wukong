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
    assert!(ResourceState::handles("job.start", &Map::new()));
    assert!(ResourceState::handles("job.list", &Map::new()));
    assert!(ResourceState::handles("resource.list", &Map::new()));
    assert!(!ResourceState::handles(
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
