use super::*;
fn owner() -> Owner {
    Owner {
        identity: crate::lease_proxy::Identity {
            protocol: 2,
            installation_id: "a".repeat(32),
            boot_id: format!("windows:{}:1", "b".repeat(32)),
            package_sha256: "c".repeat(64),
        },
        run_id: "run".into(),
        owner_id: "controller".into(),
        epoch: 1,
    }
}
#[test]
fn interrupted_native_start_retry_reads_original_intent_before_new_input_scope() -> Result<()> {
    let root = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(root.path(), true)?;
    let mut journal = Journal::with_policy(root.path(), owner().identity, policy.clone())?;
    let inputs = Inputs::open(root.path(), policy)?;
    let mut runtime = Runtime {
        root: root.path().into(),
        executable: root.path().join("never-execute"),
        installation: owner().identity.installation_id,
        running: None,
        recovery: None,
        debugger: None,
    };
    let params = json!({"argv":["files/known.exe"],"request_id":"stable"});
    let spec: Specification =
        serde_json::from_value(json!({"argv":["files/known.exe"],"timeout_secs":30}))?;
    let reservation = journal.reserve_start(
        &owner(),
        "stable",
        ResourceKind::Process,
        &json!({"native_process_schema":crate::resource_process::SCHEMA,"process":spec}),
    )?;
    let Reservation::Reserved {
        effect, resource, ..
    } = reservation
    else {
        anyhow::bail!("missing reservation");
    };
    let _ticket = journal.begin(&effect)?;
    let mut successor = owner();
    successor.epoch = 2;
    successor.owner_id = "successor".into();
    let status = runtime.start(
        &mut journal,
        &inputs,
        &successor,
        params.as_object().unwrap(),
    )?;
    assert_eq!(status["job_id"], resource);
    assert_eq!(status["state"], "lost");
    assert_eq!(status["cleanup_pending"], true);
    assert!(journal.begin(&effect).is_err());
    assert!(!journal.idle()?);
    let mut conflict = params.clone();
    conflict["argv"] = json!(["files/different.exe"]);
    assert!(
        runtime
            .start(
                &mut journal,
                &inputs,
                &successor,
                conflict.as_object().unwrap()
            )
            .is_err()
    );
    let mut stranger = successor;
    stranger.run_id = "other-run".into();
    assert!(runtime.status(&journal, &stranger, &resource).is_err());
    Ok(())
}
#[test]
fn native_routing_and_request_schema_never_select_privileged_paths() -> Result<()> {
    assert!(Runtime::handles("job.start", &Map::new()));
    assert!(Runtime::handles("job.list", &Map::new()));
    assert!(Runtime::handles("resource.list", &Map::new()));
    assert!(!Runtime::handles(
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
    let runtime = Runtime {
        root: root.path().into(),
        executable: root.path().join("never-execute"),
        installation: owner().identity.installation_id,
        running: None,
        recovery: None,
        debugger: None,
    };
    let successor = Owner {
        epoch: 2,
        owner_id: "successor".into(),
        ..owner()
    };
    let page = runtime.list(&journal, &successor, "resource.list", &Map::new())?;
    assert_eq!(page["resources"][0]["resource_id"], resource);
    assert_eq!(page["resources"][0]["phase"], "preparing");
    assert_eq!(
        page["resources"][0]["native_owner"],
        serde_json::to_value(owner())?
    );
    assert_eq!(
        page["resources"][0]["specification_sha256"],
        crate::inputs::digest(&specification)?
    );
    assert!(!page.to_string().contains("DO-NOT-PUBLISH-INVENTORY"));
    let jobs = runtime.list(&journal, &successor, "job.list", &Map::new())?;
    assert_eq!(jobs[0]["job_id"], resource);
    assert_eq!(jobs[0]["state"], "lost");
    assert_eq!(jobs[0]["cleanup_pending"], true);
    assert!(!journal.idle()?);
    for params in [
        json!({"run_id":"foreign"}),
        json!({"kind":"debugger"}),
        json!({"limit":65}),
        json!({"after":".."}),
    ] {
        assert!(
            runtime
                .list(
                    &journal,
                    &successor,
                    "job.list",
                    params.as_object().unwrap()
                )
                .is_err()
        );
    }
    let foreign = Owner {
        run_id: "foreign".into(),
        ..successor
    };
    assert_eq!(
        runtime.list(&journal, &foreign, "job.list", &Map::new())?,
        json!([])
    );
    journal.start_closing(&resource)?;
    journal.close(
        &resource,
        Cleanup {
            jobs_empty: true,
            profiles_removed: true,
            scratch_removed: true,
        },
    )?;
    // A closed ledger entry is not a fabricated successful status file.
    assert!(
        runtime
            .list(&journal, &owner(), "job.list", &Map::new())
            .is_err()
    );
    Ok(())
}

#[test]
fn interrupted_launch_retry_never_reopens_a_new_owners_manifest_or_input() -> Result<()> {
    let root = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(root.path(), true)?;
    let mut journal = Journal::with_policy(root.path(), owner().identity, policy.clone())?;
    let inputs = Inputs::open(root.path(), policy)?;
    let mut runtime = Runtime {
        root: root.path().into(),
        executable: root.path().join("never-execute"),
        installation: owner().identity.installation_id,
        running: None,
        recovery: None,
        debugger: None,
    };
    let params = json!({"launch_spec":"files/missing/launch.json","input":"files/missing/input.bin","request_id":"stable-launch"});
    let spec: Specification = serde_json::from_value(
        json!({"launch_spec":"files/missing/launch.json","input":"files/missing/input.bin"}),
    )?;
    let Reservation::Reserved {
        effect, resource, ..
    } = journal.reserve_start(
        &owner(),
        "stable-launch",
        ResourceKind::Process,
        &json!({"native_process_schema":crate::resource_process::SCHEMA,"process":spec}),
    )?
    else {
        anyhow::bail!("missing reservation")
    };
    journal.begin(&effect)?;
    let mut successor = owner();
    successor.epoch = 2;
    successor.owner_id = "successor".into();
    let status = runtime.start(
        &mut journal,
        &inputs,
        &successor,
        params.as_object().unwrap(),
    )?;
    assert_eq!(status["job_id"], resource);
    assert_eq!(status["state"], "lost");
    assert_eq!(status["cleanup_pending"], true);
    assert!(journal.begin(&effect).is_err());
    assert!(!journal.idle()?);
    Ok(())
}
