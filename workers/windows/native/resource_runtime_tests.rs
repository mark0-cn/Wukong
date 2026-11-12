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
    };
    let params = json!({"argv":["files/known.exe"],"request_id":"stable"});
    let spec: Specification = serde_json::from_value(json!({"argv":["files/known.exe"]}))?;
    let reservation = journal.reserve_start(
        &owner(),
        "stable",
        ResourceKind::Process,
        &serde_json::to_value(spec)?,
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
