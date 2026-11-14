use super::*;
use serde_json::json;

fn owner() -> Owner {
    Owner {
        identity: Binding {
            protocol: 3,
            installation_id: "a".repeat(32),
            boot_id: format!("windows:{}:40", "b".repeat(32)),
            package_sha256: "c".repeat(64),
        },
        run_id: "run".into(),
        owner_id: "controller-one".into(),
        epoch: 1,
    }
}
fn fixture() -> Result<(tempfile::TempDir, AclPolicy, Journal)> {
    let root = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(root.path(), true)?;
    let journal = Journal::with_policy(root.path(), owner().identity, policy.clone())?;
    Ok((root, policy, journal))
}
fn reserve(journal: &mut Journal, actor: &Owner, request: &str) -> Result<(String, String)> {
    match journal.reserve_start(
        actor,
        request,
        ResourceKind::Debugger,
        &json!({"target_sha256":"d".repeat(64),"args":[]}),
    )? {
        Reservation::Reserved {
            effect, resource, ..
        } => Ok((effect, resource)),
        _ => anyhow::bail!("fixture did not receive a reservation"),
    }
}
fn cleanup() -> Cleanup {
    Cleanup {
        jobs_empty: true,
        profiles_removed: true,
        scratch_removed: true,
    }
}

fn complete_start(journal: &mut Journal) -> Result<(String, String)> {
    let (effect, resource) = reserve(journal, &owner(), "start")?;
    let ticket = journal.begin(&effect)?;
    journal.complete(
        &ticket,
        &Outcome::Result {
            value: json!({"session_id":resource}),
        },
    )?;
    Ok((effect, resource))
}
fn mutation(journal: &mut Journal, request: &str, resource: &str) -> Result<Reservation> {
    journal.reserve_mutation(
        &owner(),
        request,
        resource,
        MutationKind::DebugControl,
        &json!({"action":"continue"}),
    )
}
fn reserved_effect(reservation: Reservation) -> Result<String> {
    match reservation {
        Reservation::Reserved { effect, .. } => Ok(effect),
        _ => anyhow::bail!("expected a reserved fixture effect"),
    }
}

#[test]
fn inventory_is_bounded_scoped_paged_and_preserves_original_owners_across_reopen() -> Result<()> {
    let (root, policy, mut journal) = fixture()?;
    let foreign = Owner {
        run_id: "other-run".into(),
        ..owner()
    };
    let Reservation::Reserved { resource, .. } =
        journal.reserve_start(&foreign, "foreign", ResourceKind::Process, &json!({}))?
    else {
        anyhow::bail!("missing fixture reservation")
    };
    journal.start_closing(&resource)?;
    journal.close(&resource, cleanup())?;
    for index in 0..7 {
        let Reservation::Reserved { resource, .. } = journal.reserve_start(
            &owner(),
            &format!("start-{index}"),
            if index % 2 == 0 {
                ResourceKind::Process
            } else {
                ResourceKind::Debugger
            },
            &json!({"index":index}),
        )?
        else {
            anyhow::bail!("missing fixture reservation")
        };
        if index != 6 {
            journal.start_closing(&resource)?;
            journal.close(&resource, cleanup())?;
        }
    }
    let successor = Owner {
        owner_id: "successor".into(),
        epoch: 2,
        ..owner()
    };
    let all = journal.resources(&successor, None, None, 64)?;
    assert_eq!(all.len(), 7);
    assert!(all.iter().all(|record| record.owner == owner()));
    assert_eq!(
        all.iter()
            .filter(|record| record.phase == Phase::Preparing)
            .count(),
        1
    );
    let processes = journal.resources(&successor, Some(ResourceKind::Process), None, 64)?;
    assert_eq!(processes.len(), 4);
    let mut observed = Vec::new();
    let mut after = None;
    loop {
        let page =
            journal.resources(&successor, Some(ResourceKind::Process), after.as_deref(), 1)?;
        if page.is_empty() {
            break;
        }
        after = Some(page[0].id.clone());
        observed.extend(page);
    }
    assert_eq!(observed, processes);
    assert_eq!(journal.resources(&foreign, None, None, 64)?.len(), 1);
    assert!(
        journal
            .resources(
                &Owner {
                    run_id: "empty-run".into(),
                    ..successor.clone()
                },
                None,
                None,
                64
            )?
            .is_empty()
    );
    for (cursor, limit) in [
        (Some("../outside"), 1),
        (Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"), 1),
        (None, 0),
        (None, 65),
    ] {
        assert!(journal.resources(&successor, None, cursor, limit).is_err());
    }
    assert!(
        journal
            .resources(
                &Owner {
                    epoch: 0,
                    ..successor.clone()
                },
                None,
                None,
                1
            )
            .is_err()
    );
    drop(journal);
    let journal = Journal::with_policy(root.path(), successor.identity.clone(), policy.clone())?;
    assert_eq!(journal.resources(&successor, None, None, 64)?, all);
    drop(journal);
    let mut next_boot = successor;
    next_boot.identity.boot_id = format!("windows:{}:41", "b".repeat(32));
    let journal = Journal::with_policy(root.path(), next_boot.identity.clone(), policy)?;
    // Even an empty page cannot hide a prior run's different worker binding.
    assert!(
        journal
            .resources(&next_boot, None, Some(&"f".repeat(32)), 1)
            .is_err()
    );
    Ok(())
}

#[test]
fn inventory_validates_fingerprints_and_rejects_missing_or_ambiguous_start_intents() -> Result<()> {
    let (_root, _policy, mut journal) = fixture()?;
    let (effect, resource) = reserve(&mut journal, &owner(), "start")?;
    journal.start_closing(&resource)?;
    journal.close(&resource, cleanup())?;
    let original: String = journal.connection.query_row(
        "SELECT payload FROM effects WHERE id=?1",
        [&effect],
        |row| row.get(0),
    )?;
    let mut changed: Value = serde_json::from_str(&original)?;
    changed["specification"]["changed"] = json!(true);
    journal.connection.execute(
        "UPDATE effects SET payload=?1 WHERE id=?2",
        params![changed.to_string(), effect],
    )?;
    assert!(journal.resources(&owner(), None, None, 1).is_err());
    journal.connection.execute(
        "UPDATE effects SET payload=?1 WHERE id=?2",
        params![original, effect],
    )?;
    journal.connection.execute("INSERT INTO effects SELECT ?1,resource,fingerprint,payload,actor,state,ticket,outcome FROM effects WHERE id=?2", params!["f".repeat(64), effect])?;
    assert!(journal.resources(&owner(), None, None, 1).is_err());
    journal
        .connection
        .execute("DELETE FROM effects WHERE resource=?1", [&resource])?;
    assert!(journal.resources(&owner(), None, None, 1).is_err());
    Ok(())
}

#[test]
fn recovery_view_preserves_original_intent_and_owner_without_reopening_old_boot_work() -> Result<()>
{
    let (root, policy, mut journal) = fixture()?;
    assert!(journal.active()?.is_none());
    let (effect, resource) = reserve(&mut journal, &owner(), "start")?;
    let initial = journal.active()?.context("missing preparing intent")?;
    assert_eq!(
        initial,
        ResourceRecord {
            id: resource.clone(),
            phase: Phase::Preparing,
            start_effect: effect.clone(),
            owner: owner(),
            kind: ResourceKind::Debugger,
            specification: json!({"target_sha256":"d".repeat(64),"args":[]})
        }
    );
    let ticket = journal.begin(&effect)?;
    journal.complete(
        &ticket,
        &Outcome::Result {
            value: json!({"session_id":resource}),
        },
    )?;
    assert_eq!(journal.active()?.unwrap().phase, Phase::Live);
    journal.start_closing(&resource)?;
    drop(journal);
    let mut current = owner();
    current.identity.boot_id = format!("windows:{}:41", "b".repeat(32));
    let mut journal = Journal::with_policy(root.path(), current.identity.clone(), policy)?;
    let persisted = journal.active()?.context("lost old-boot resource intent")?;
    assert_eq!(persisted.phase, Phase::Closing);
    assert_eq!(persisted.owner, initial.owner);
    assert_eq!(persisted.specification, initial.specification);
    assert!(journal.resource(&current, &resource).is_err());
    assert!(!journal.idle()?);
    journal.close(&resource, cleanup())?;
    assert!(journal.active()?.is_none());
    Ok(())
}

#[test]
fn live_mutations_are_serialized_and_cached_across_completion_and_resource_close() -> Result<()> {
    let (_root, _policy, mut journal) = fixture()?;
    let (_, resource) = complete_start(&mut journal)?;
    let first = reserved_effect(mutation(&mut journal, "continue-1", &resource)?)?;
    assert!(mutation(&mut journal, "continue-2", &resource).is_err());
    let ticket = journal.begin(&first)?;
    assert!(journal.begin(&first).is_err());
    assert!(mutation(&mut journal, "continue-2", &resource).is_err());
    let result = Outcome::Result {
        value: json!({"state":"stopped"}),
    };
    journal.complete(&ticket, &result)?;
    assert_eq!(
        mutation(&mut journal, "continue-1", &resource)?,
        Reservation::Completed {
            effect: first.clone(),
            resource: resource.clone(),
            phase: Phase::Live,
            outcome: result.clone()
        }
    );
    assert!(
        journal
            .reserve_mutation(
                &owner(),
                "continue-1",
                &resource,
                MutationKind::DebugControl,
                &json!({"action":"step"})
            )
            .is_err()
    );
    let second = reserved_effect(mutation(&mut journal, "continue-2", &resource)?)?;
    let ticket = journal.begin(&second)?;
    journal.complete(&ticket, &result)?;
    assert!(
        journal
            .reserve_mutation(
                &Owner {
                    run_id: "other-run".into(),
                    ..owner()
                },
                "stolen",
                &resource,
                MutationKind::DebugControl,
                &json!({})
            )
            .is_err()
    );
    journal.start_closing(&resource)?;
    assert!(mutation(&mut journal, "after-closing", &resource).is_err());
    journal.close(&resource, cleanup())?;
    assert_eq!(
        mutation(&mut journal, "continue-1", &resource)?,
        Reservation::Completed {
            effect: first,
            resource: resource.clone(),
            phase: Phase::Closed,
            outcome: result
        }
    );
    assert!(mutation(&mut journal, "after-close", &resource).is_err());
    assert!(journal.idle()?);
    Ok(())
}

#[test]
fn interrupted_live_mutation_remains_pending_until_cleanup_and_rejects_old_tickets() -> Result<()> {
    let (root, policy, mut journal) = fixture()?;
    let (_, resource) = complete_start(&mut journal)?;
    let effect = reserved_effect(mutation(&mut journal, "continue", &resource)?)?;
    let ticket = journal.begin(&effect)?;
    drop(journal);
    let mut journal = Journal::with_policy(root.path(), owner().identity, policy)?;
    assert_eq!(
        mutation(&mut journal, "continue", &resource)?,
        Reservation::Pending {
            effect: effect.clone(),
            resource: resource.clone(),
            phase: Phase::Live
        }
    );
    assert!(journal.begin(&effect).is_err());
    assert!(mutation(&mut journal, "another", &resource).is_err());
    assert!(
        journal
            .complete(
                &ticket,
                &Outcome::Result {
                    value: json!("late")
                }
            )
            .is_err()
    );
    assert_eq!(journal.active()?.unwrap().id, resource);
    journal.start_closing(&resource)?;
    journal.close(&resource, cleanup())?;
    assert_eq!(
        mutation(&mut journal, "continue", &resource)?,
        Reservation::Completed {
            effect,
            resource,
            phase: Phase::Closed,
            outcome: Outcome::UncertainClosed
        }
    );
    assert!(journal.active()?.is_none());
    Ok(())
}

#[test]
fn mutation_kind_and_recovery_intent_fingerprint_are_not_schema_only_checks() -> Result<()> {
    let (_root, _policy, mut journal) = fixture()?;
    let resource = match journal.reserve_start(
        &owner(),
        "interactive",
        ResourceKind::Interactive,
        &json!({"args":[]}),
    )? {
        Reservation::Reserved {
            effect, resource, ..
        } => {
            let ticket = journal.begin(&effect)?;
            journal.complete(&ticket, &Outcome::Result { value: json!({}) })?;
            resource
        }
        _ => unreachable!(),
    };
    assert!(mutation(&mut journal, "wrong-kind", &resource).is_err());
    let effect = reserved_effect(journal.reserve_mutation(
        &owner(),
        "write",
        &resource,
        MutationKind::InteractiveWrite,
        &json!({"data_base64":"YQ=="}),
    )?)?;
    let ticket = journal.begin(&effect)?;
    journal.complete(
        &ticket,
        &Outcome::Result {
            value: json!({"written":1}),
        },
    )?;
    journal.connection.execute("UPDATE effects SET payload=json_set(payload,'$.specification','changed') WHERE json_extract(payload,'$.operation')='start'",[])?;
    assert!(
        journal
            .active()
            .unwrap_err()
            .to_string()
            .contains("fingerprint")
    );
    journal.connection.execute("UPDATE effects SET payload=json_set(payload,'$.operation','missing') WHERE json_extract(payload,'$.operation')='start'",[])?;
    assert!(
        journal
            .active()
            .unwrap_err()
            .to_string()
            .contains("recoverable")
    );
    assert!(!journal.idle()?);
    Ok(())
}

#[test]
fn protected_database_and_reopen_support_paths_beyond_the_legacy_windows_limit() -> Result<()> {
    let root = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(root.path(), true)?;
    let metadata = root
        .path()
        .join(format!("long-metadata-{}", "x".repeat(180)));
    policy.create_directory(&metadata)?;
    assert!(
        metadata
            .join("native-resources/resources.sqlite")
            .to_string_lossy()
            .len()
            > 260
    );
    let mut journal = Journal::with_policy(&metadata, owner().identity, policy.clone())?;
    let (effect, resource) = reserve(&mut journal, &owner(), "long-path")?;
    let _ticket = journal.begin(&effect)?;
    drop(journal);
    let mut journal = Journal::with_policy(&metadata, owner().identity, policy)?;
    assert!(!journal.idle()?);
    assert!(journal.begin(&effect).is_err());
    journal.start_closing(&resource)?;
    journal.close(&resource, cleanup())?;
    assert!(journal.idle()?);
    Ok(())
}

#[test]
fn completed_start_is_reused_after_reopen_and_fence_change_without_reopening_closed_work()
-> Result<()> {
    let (root, policy, mut journal) = fixture()?;
    let (effect, resource) = reserve(&mut journal, &owner(), "start")?;
    let ticket = journal.begin(&effect)?;
    let outcome = Outcome::Result {
        value: json!({"session_id":resource}),
    };
    journal.complete(&ticket, &outcome)?;
    journal.complete(&ticket, &outcome)?;
    assert!(!journal.idle()?);
    journal.start_closing(&resource)?;
    journal.close(&resource, cleanup())?;
    assert!(journal.idle()?);
    drop(journal);
    let mut journal = Journal::with_policy(root.path(), owner().identity, policy)?;
    let actor = Owner {
        owner_id: "controller-two".into(),
        epoch: 2,
        ..owner()
    };
    let replay = journal.reserve_start(
        &actor,
        "start",
        ResourceKind::Debugger,
        &json!({"args":[],"target_sha256":"d".repeat(64)}),
    )?;
    assert_eq!(
        replay,
        Reservation::Completed {
            effect: effect.clone(),
            resource: resource.clone(),
            phase: Phase::Closed,
            outcome
        }
    );
    assert!(journal.begin(&effect).is_err());
    assert!(journal.idle()?);
    Ok(())
}

#[test]
fn interrupted_start_remains_uncertain_and_blocks_other_resources_across_reopen() -> Result<()> {
    let (root, policy, mut journal) = fixture()?;
    let (effect, resource) = reserve(&mut journal, &owner(), "interrupted")?;
    let old_ticket = journal.begin(&effect)?;
    drop(journal);
    let mut journal = Journal::with_policy(root.path(), owner().identity, policy)?;
    let actor = Owner {
        owner_id: "successor".into(),
        epoch: 3,
        ..owner()
    };
    assert_eq!(
        journal.reserve_start(
            &actor,
            "interrupted",
            ResourceKind::Debugger,
            &json!({"target_sha256":"d".repeat(64),"args":[]})
        )?,
        Reservation::Pending {
            effect: effect.clone(),
            resource: resource.clone(),
            phase: Phase::Preparing
        }
    );
    assert!(journal.begin(&effect).is_err());
    assert!(
        journal
            .complete(
                &old_ticket,
                &Outcome::Result {
                    value: json!({"not":"current"})
                }
            )
            .is_err()
    );
    assert!(reserve(&mut journal, &actor, "another").is_err());
    assert!(!journal.idle()?);
    journal.start_closing(&resource)?;
    journal.close(&resource, cleanup())?;
    assert_eq!(
        journal.reserve_start(
            &actor,
            "interrupted",
            ResourceKind::Debugger,
            &json!({"target_sha256":"d".repeat(64),"args":[]})
        )?,
        Reservation::Completed {
            effect,
            resource,
            phase: Phase::Closed,
            outcome: Outcome::UncertainClosed
        }
    );
    assert!(journal.idle()?);
    reserve(&mut journal, &actor, "new-after-cleanup")?;
    Ok(())
}

#[test]
fn cleanup_flags_do_not_release_a_resource_until_all_business_preconditions_hold() -> Result<()> {
    let (_root, _policy, mut journal) = fixture()?;
    let (effect, resource) = reserve(&mut journal, &owner(), "start")?;
    let ticket = journal.begin(&effect)?;
    journal.start_closing(&resource)?;
    for failed in 0..3 {
        assert!(
            journal
                .close(
                    &resource,
                    Cleanup {
                        jobs_empty: failed != 0,
                        profiles_removed: failed != 1,
                        scratch_removed: failed != 2
                    }
                )
                .is_err()
        );
        assert!(!journal.idle()?);
        assert!(reserve(&mut journal, &owner(), "another").is_err());
    }
    journal.close(&resource, cleanup())?;
    assert!(
        journal
            .complete(
                &ticket,
                &Outcome::Result {
                    value: json!({"late":"success"})
                }
            )
            .is_err()
    );
    assert!(journal.idle()?);
    Ok(())
}

#[test]
fn changed_parameters_and_old_boot_run_bindings_never_create_another_effect() -> Result<()> {
    let (root, policy, mut journal) = fixture()?;
    let (_, resource) = reserve(&mut journal, &owner(), "same-id")?;
    assert!(
        journal
            .reserve_start(
                &owner(),
                "same-id",
                ResourceKind::Debugger,
                &json!({"target_sha256":"different"})
            )
            .is_err()
    );
    journal.start_closing(&resource)?;
    journal.close(&resource, cleanup())?;
    drop(journal);
    let mut actor = owner();
    actor.identity.boot_id = format!("windows:{}:41", "b".repeat(32));
    let mut journal = Journal::with_policy(root.path(), actor.identity.clone(), policy)?;
    assert!(reserve(&mut journal, &actor, "same-id").is_err());
    assert!(reserve(&mut journal, &actor, "new-id-same-run").is_err());
    actor.run_id = "new-run".into();
    reserve(&mut journal, &actor, "new-id")?;
    Ok(())
}

#[test]
fn exclusive_writer_and_database_handle_prevent_concurrent_adoption_or_replacement() -> Result<()> {
    let (root, policy, journal) = fixture()?;
    assert!(Journal::with_policy(root.path(), owner().identity, policy).is_err());
    let database = root.path().join("native-resources/resources.sqlite");
    assert!(fs::rename(&database, database.with_extension("other")).is_err());
    assert!(journal.idle()?);
    Ok(())
}

#[test]
fn invalid_scopes_and_oversized_specs_do_not_reserve_the_active_slot() -> Result<()> {
    let (_root, _policy, mut journal) = fixture()?;
    for actor in [
        Owner {
            epoch: 0,
            ..owner()
        },
        Owner {
            run_id: "bad\0scope".into(),
            ..owner()
        },
        Owner {
            identity: Binding {
                package_sha256: "d".repeat(64),
                ..owner().identity
            },
            ..owner()
        },
    ] {
        assert!(reserve(&mut journal, &actor, "bad").is_err());
    }
    assert!(
        journal
            .reserve_start(
                &owner(),
                "too-large",
                ResourceKind::Process,
                &json!({"args":"x".repeat(MAX_JSON)})
            )
            .is_err()
    );
    assert!(journal.idle()?);
    reserve(&mut journal, &owner(), "valid")?;
    Ok(())
}

#[test]
fn old_reserved_work_and_unsupported_database_versions_fail_closed() -> Result<()> {
    let (root, policy, mut journal) = fixture()?;
    let (effect, _) = reserve(&mut journal, &owner(), "not-started")?;
    drop(journal);
    let mut binding = owner().identity;
    binding.boot_id = format!("windows:{}:41", "b".repeat(32));
    let mut journal = Journal::with_policy(root.path(), binding.clone(), policy.clone())?;
    assert!(journal.begin(&effect).is_err());
    assert!(!journal.idle()?);
    journal.connection.pragma_update(None, "user_version", 99)?;
    drop(journal);
    assert!(Journal::with_policy(root.path(), binding, policy).is_err());
    Ok(())
}

#[test]
fn crash_fixture_entry() -> Result<()> {
    let Some(root) = std::env::var_os("WUKONG_RESOURCE_JOURNAL_CRASH_FIXTURE") else {
        return Ok(());
    };
    let root = std::path::PathBuf::from(root);
    ensure!(root.is_absolute(), "fixture root must be absolute");
    let policy = AclPolicy::current_user()?;
    let mut journal = Journal::with_policy(&root, owner().identity, policy)?;
    journal.connection.pragma_update(None, "cache_size", 1)?;
    let transaction = journal
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "UPDATE effects SET payload=?1,state='completed',outcome=?2",
        params![
            "x".repeat(MAX_JSON - 1),
            bounded_json(&Outcome::Result {
                value: json!({"not":"committed"})
            })?
        ],
    )?;
    transaction.execute("UPDATE resources SET phase='closed'", [])?;
    // Exercise our ACL and held-handle policy with a real hot rollback journal,
    // without a destructor/SQLite rollback in the deliberately killed child.
    fs::write(root.join("crash-fixture-ready"), b"uncommitted")?;
    loop {
        std::thread::park_timeout(Duration::from_secs(1));
    }
}

#[tokio::test]
async fn a_killed_database_writer_does_not_release_the_resource_or_break_rollback_recovery()
-> Result<()> {
    let (root, policy, mut journal) = fixture()?;
    let (effect, resource) = reserve(&mut journal, &owner(), "crash")?;
    let _ticket = journal.begin(&effect)?;
    drop(journal);
    let mut child = tokio::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "resource_journal::tests::crash_fixture_entry",
            "--nocapture",
        ])
        .env("WUKONG_RESOURCE_JOURNAL_CRASH_FIXTURE", root.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !root.path().join("crash-fixture-ready").exists() {
        ensure!(
            child.try_wait()?.is_none() && tokio::time::Instant::now() < deadline,
            "journal crash fixture did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        fs::metadata(
            root.path()
                .join("native-resources/resources.sqlite-journal")
        )?
        .len()
            > 0
    );
    child.kill().await?;
    let mut journal = Journal::with_policy(root.path(), owner().identity, policy)?;
    assert!(!journal.idle()?);
    assert_eq!(
        journal.reserve_start(
            &owner(),
            "crash",
            ResourceKind::Debugger,
            &json!({"target_sha256":"d".repeat(64),"args":[]})
        )?,
        Reservation::Pending {
            effect,
            resource: resource.clone(),
            phase: Phase::Preparing
        }
    );
    journal.start_closing(&resource)?;
    journal.close(&resource, cleanup())?;
    assert!(journal.idle()?);
    Ok(())
}
