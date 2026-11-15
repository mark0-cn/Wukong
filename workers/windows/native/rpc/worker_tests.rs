use super::*;
use crate::artifacts::inputs::Owner;
use crate::resources::models::ResourceKind;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn identity() -> Identity {
    Identity {
        protocol: 3,
        installation_id: "a".repeat(32),
        boot_id: format!("windows:{}:40", "b".repeat(32)),
        package_sha256: "c".repeat(64),
    }
}
fn fixture() -> Result<(tempfile::TempDir, Arc<Worker>)> {
    let temp = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(temp.path(), true)?;
    let worker = Worker::with_policy(identity(), temp.path(), policy)?;
    Ok((temp, Arc::new(worker)))
}
fn request(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
}
fn acquire(run: &str, owner: &str) -> Value {
    request(
        "environment.lease.acquire",
        json!({"run_id":run,"owner_id":owner,"_binding":identity()}),
    )
}

#[tokio::test]
async fn replay_and_minimization_dispatch_reach_owned_input_admission() -> Result<()> {
    let (_root, worker) = fixture()?;
    let (runtime_root, _inputs, _runtime) = crate::resources::state::tests::fixture()?;
    worker
        .resources
        .initialize(crate::resources::state::tests::context(runtime_root.path()))?;
    let lease = call(&worker, &acquire("run", "controller")).await?["result"].clone();
    let uploaded = call(
        &worker,
        &request(
            "file.put",
            json!({
                "path":"fixture.bin","data_base64":"AA==","executable":false,
                "request_id":"input","_lease":lease,"_binding":identity()
            }),
        ),
    )
    .await?;
    assert!(uploaded.get("error").is_none(), "{uploaded}");
    for method in ["input.minimize", "crash.replay", "poc.verify"] {
        let mut params = json!({
            "target":"files/fixture.bin","input":"files/fixture.bin","args":["@@"],
            "request_id":method,"_lease":lease,"_binding":identity()
        });
        if method == "poc.verify" {
            params["producer_agent_id"] = json!("producer");
            params["verifier_agent_id"] = json!("verifier");
        }
        let reply = call(&worker, &request(method, params)).await?;
        assert_eq!(
            reply["error"]["message"],
            "native logical input differs from admitted owner or execution policy"
        );
    }
    // Admission fails before reserving a resource or executing the fixture bytes.
    assert_eq!(
        call(&worker, &release(&lease)).await?["result"]["released"],
        true
    );
    Ok(())
}

#[tokio::test]
async fn native_resource_state_blocks_release_and_expired_takeover_but_not_renewal() -> Result<()> {
    use crate::resources::journal::{Cleanup, Reservation};
    let (_root, worker) = fixture()?;
    let first = call(&worker, &acquire("run", "controller")).await?["result"].clone();
    let resource = {
        let mut resources = worker.resources.test_state().await;
        let actor = Owner {
            identity: identity(),
            run_id: "run".into(),
            owner_id: "controller".into(),
            epoch: first["epoch"].as_u64().unwrap(),
        };
        match resources.test_journal().reserve_start(
            &actor,
            "start",
            ResourceKind::Process,
            &json!({"trusted_fixture":true}),
        )? {
            Reservation::Reserved { resource, .. } => resource,
            _ => anyhow::bail!("missing native resource reservation"),
        }
    };
    let release = request(
        "environment.lease.release",
        json!({"_binding":identity(),"lease":first}),
    );
    assert_eq!(
        call(&worker, &release).await?["error"]["data"]["kind"],
        "conflict"
    );
    assert_eq!(
        call(&worker, &acquire("run", "controller")).await?["result"]["epoch"],
        first["epoch"]
    );
    worker
        .leases
        .state
        .lock()
        .unwrap()
        .current
        .as_mut()
        .unwrap()
        .deadline = Instant::now() - Duration::from_secs(1);
    assert_eq!(
        call(&worker, &acquire("next", "successor")).await?["error"]["data"]["kind"],
        "conflict"
    );
    {
        let mut resources = worker.resources.test_state().await;
        resources.test_journal().start_closing(&resource)?;
        resources.test_journal().close(
            &resource,
            Cleanup {
                jobs_empty: true,
                profiles_removed: true,
                scratch_removed: true,
            },
        )?;
    }
    assert!(
        call(&worker, &acquire("next", "successor")).await?["result"]["epoch"]
            .as_u64()
            .unwrap()
            > first["epoch"].as_u64().unwrap()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn busy_native_resources_do_not_block_renewal_or_allow_idle_transitions() -> Result<()> {
    let (_root, worker) = fixture()?;
    let first = call(&worker, &acquire("run", "controller")).await?["result"].clone();
    let (ready, locked) = tokio::sync::oneshot::channel();
    let (release, finish) = std::sync::mpsc::channel();
    let busy = worker.clone();
    let resource = std::thread::spawn(move || {
        let _resources = busy.resources.test_blocking_state();
        ready.send(()).unwrap();
        // The bound also releases the lock if a regression blocks the caller.
        let _ = finish.recv_timeout(Duration::from_secs(5));
    });
    locked.await?;
    // Contended RPCs must yield the executor, not occupy all service threads
    // while waiting for the same native resource. Poll the real dispatch path.
    let requests: Vec<_> = (0..4).map(|_| effect(&first)).collect();
    let pending = futures_util::future::join_all(requests.iter().map(|value| call(&worker, value)));
    tokio::pin!(pending);
    assert!(futures_util::poll!(&mut pending).is_pending());
    assert_eq!(worker.leases.state.lock().unwrap().inflight, 4);
    assert!(!worker.poll_resources()?);
    let active = worker.clone();
    let expected_epoch = first["epoch"].clone();
    let operations = tokio::spawn(async move {
        let renewed = call(&active, &acquire("run", "controller")).await?;
        assert_eq!(renewed["result"]["epoch"], expected_epoch);
        let close = request(
            "environment.lease.release",
            json!({"_binding":identity(),"lease":first}),
        );
        assert_eq!(
            call(&active, &close).await?["error"]["data"]["kind"],
            "conflict"
        );
        active
            .leases
            .state
            .lock()
            .unwrap()
            .current
            .as_mut()
            .unwrap()
            .deadline = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            call(&active, &acquire("next", "successor")).await?["error"]["data"]["kind"],
            "conflict"
        );
        Ok::<_, anyhow::Error>(())
    });
    let result = tokio::time::timeout(Duration::from_secs(1), operations).await;
    let _ = release.send(());
    resource.join().unwrap();
    result???;
    for result in pending.await {
        result?;
    }
    assert!(
        call(&worker, &acquire("next", "successor")).await?["result"]["epoch"]
            .as_u64()
            .is_some_and(|epoch| epoch > 1)
    );
    Ok(())
}

#[tokio::test]
async fn persisted_native_resource_state_still_blocks_takeover_after_worker_reopen() -> Result<()> {
    use crate::resources::models::ResourceKind;
    let (root, worker) = fixture()?;
    let lease = call(&worker, &acquire("run", "controller")).await?["result"].clone();
    {
        let mut resources = worker.resources.test_state().await;
        resources.test_journal().reserve_start(
            &Owner {
                identity: identity(),
                run_id: "run".into(),
                owner_id: "controller".into(),
                epoch: lease["epoch"].as_u64().unwrap(),
            },
            "reserved-before-crash",
            ResourceKind::Debugger,
            &json!({"trusted_fixture":true}),
        )?;
    }
    drop(worker);
    let worker = Arc::new(Worker::with_policy(
        identity(),
        root.path(),
        AclPolicy::current_user()?,
    )?);
    assert_eq!(
        call(&worker, &acquire("next", "successor")).await?["error"]["data"]["kind"],
        "conflict"
    );
    Ok(())
}
async fn call(worker: &Arc<Worker>, value: &Value) -> Result<Value> {
    Ok(serde_json::from_slice(
        &crate::rpc::server::Server::new(worker.clone())?
            .call(&serde_json::to_vec(value)?)
            .await?,
    )?)
}
fn release(lease: &Value) -> Value {
    request(
        "environment.lease.release",
        json!({"lease":lease,"_binding":identity()}),
    )
}
fn effect(lease: &Value) -> Value {
    request(
        "job.status",
        json!({"job_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","request_id":"stable","_lease":lease,"_binding":identity()}),
    )
}

#[tokio::test]
async fn one_authority_owns_handshake_fencing_and_files() -> Result<()> {
    let (root, worker) = fixture()?;
    let ping = call(&worker, &request("ping", json!({}))).await?;
    assert_eq!(
        ping["result"]["identity"],
        serde_json::to_value(identity())?
    );
    let doctor = call(&worker, &request("doctor", json!({}))).await?;
    assert_eq!(doctor["result"]["capacity"]["max_active_resources"], 1);
    assert_eq!(doctor["result"]["capabilities"]["debug_dbgeng"], false);
    worker.qualify();
    assert_eq!(
        call(&worker, &request("doctor", json!({}))).await?["result"]["capabilities"]["debug_dbgeng"],
        true
    );
    let lease = call(&worker, &acquire("run", "controller")).await?["result"].clone();
    let put = request(
        "file.put",
        json!({"path":"sample.bin","data_base64":"YWJj","_binding":identity(),"_lease":lease}),
    );
    let uploaded = call(&worker, &put).await?;
    assert_eq!(uploaded["result"]["native_input"]["epoch"], lease["epoch"]);
    assert_eq!(call(&worker, &put).await?, uploaded);
    let get = request(
        "file.get",
        json!({"path":"files/sample.bin","_binding":identity(),"_lease":lease}),
    );
    assert_eq!(call(&worker, &get).await?["result"]["data_base64"], "YWJj");
    assert!(!root.path().join("files").exists());
    assert_eq!(
        call(&worker, &release(&lease)).await?["result"]["released"],
        true
    );
    assert_eq!(
        call(&worker, &get).await?["error"]["data"]["kind"],
        "lease_lost"
    );
    drop(worker);
    let worker = Arc::new(Worker::with_policy(
        identity(),
        root.path(),
        AclPolicy::current_user()?,
    )?);
    let successor = call(&worker, &acquire("next", "controller")).await?["result"].clone();
    assert!(successor["epoch"].as_u64().unwrap() > lease["epoch"].as_u64().unwrap());
    assert_eq!(
        call(&worker, &put).await?["error"]["data"]["kind"],
        "lease_lost"
    );
    let mut foreign = get;
    foreign["params"]["_lease"] = successor;
    assert!(call(&worker, &foreign).await?.get("error").is_some());
    Ok(())
}

#[tokio::test]
async fn profile_queries_remain_available_while_resource_mutation_is_locked() -> Result<()> {
    let (_root, worker) = fixture()?;
    let lease = call(&worker, &acquire("run", "controller")).await?["result"].clone();
    let upload = call(
        &worker,
        &request(
            "file.put",
            json!({"path":"sample.bin",
        "data_base64":"YWJj","_binding":identity(),"_lease":lease}),
        ),
    )
    .await?;
    let resources = worker.resources.test_state().await;
    let query = request(
        "runtime.profile",
        json!({"target":"files/sample.bin",
        "_binding":identity(),"_lease":lease}),
    );
    let profile = tokio::time::timeout(Duration::from_secs(1), call(&worker, &query)).await??;
    assert_eq!(
        profile["result"]["files"]["target"]["sha256"],
        upload["result"]["sha256"]
    );
    assert_eq!(profile["result"]["status"], "declared_files_only");
    assert_eq!(profile["result"]["mapped_match"], false);
    assert_eq!(
        profile["result"]["environment_support"]["status"],
        "unobserved"
    );
    drop(resources);
    call(&worker, &release(&lease)).await?;
    let successor = call(&worker, &acquire("next", "controller")).await?["result"].clone();
    let mut foreign = query;
    foreign["params"]["_lease"] = successor;
    assert!(call(&worker, &foreign).await?.get("error").is_some());
    Ok(())
}

#[tokio::test]
async fn cancelled_input_wait_has_no_effect_and_cannot_poison_the_next_owner() -> Result<()> {
    let (_root, worker) = fixture()?;
    let lease = call(&worker, &acquire("run", "controller")).await?["result"].clone();
    let locked = worker.inputs.lock().await;
    let put = request(
        "file.put",
        json!({"path":"never.bin","data_base64":"AA==","_binding":identity(),"_lease":lease}),
    );
    {
        let pending = call(&worker, &put);
        tokio::pin!(pending);
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert_eq!(worker.leases.state.lock().unwrap().inflight, 1);
    }
    assert_eq!(worker.leases.state.lock().unwrap().inflight, 0);
    drop(locked);
    assert_eq!(
        call(&worker, &release(&lease)).await?["result"]["released"],
        true
    );
    let next = call(&worker, &acquire("next", "successor")).await?["result"].clone();
    assert!(next["epoch"].as_u64().unwrap() > lease["epoch"].as_u64().unwrap());
    let get = request(
        "file.get",
        json!({"path":"files/never.bin","_binding":identity(),"_lease":next}),
    );
    assert!(call(&worker, &get).await?.get("error").is_some());
    Ok(())
}

#[tokio::test]
async fn corrupt_epoch_records_are_never_repaired_or_reused() -> Result<()> {
    let (root, worker) = fixture()?;
    let lease = call(&worker, &acquire("run", "controller")).await?["result"].clone();
    call(&worker, &release(&lease)).await?;
    drop(worker);
    let epoch = root
        .path()
        .join("lease-epochs/epoch-00000000000000000001.json");
    std::fs::write(&epoch, b"corrupt")?;
    assert!(Worker::with_policy(identity(), root.path(), AclPolicy::current_user()?).is_err());
    assert_eq!(std::fs::read(epoch)?, b"corrupt");
    Ok(())
}
