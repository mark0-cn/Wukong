use super::*;
use crate::core::Owner;
use crate::core::resource::ResourceKind;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn identity() -> Identity {
    Identity {
        protocol: wukong_worker_protocol::PROTOCOL,
        installation_id: "a".repeat(32),
        boot_id: format!("windows:{}:40", "b".repeat(32)),
        package_sha256: "c".repeat(64),
    }
}
fn fixture() -> Result<(tempfile::TempDir, Arc<Worker>)> {
    let (root, worker, _resources) = fixture_state()?;
    Ok((root, worker))
}
fn fixture_state() -> Result<(tempfile::TempDir, Arc<Worker>, Arc<ResourceManager>)> {
    let temp = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(temp.path(), true)?;
    let resources = Arc::new(ResourceManager::open(
        temp.path(),
        identity(),
        policy.clone(),
    )?);
    let worker = Worker::with_policy(identity(), temp.path(), policy, Arc::clone(&resources))?;
    Ok((temp, Arc::new(worker), resources))
}
/// Reopen the RPC entry over an existing installation after a simulated
/// service restart; the host would hand in a freshly opened manager.
fn reopen(root: &std::path::Path) -> Result<Arc<Worker>> {
    let policy = AclPolicy::current_user()?;
    let resources = Arc::new(ResourceManager::open(root, identity(), policy.clone())?);
    Ok(Arc::new(Worker::with_policy(
        identity(),
        root,
        policy,
        resources,
    )?))
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
    assert!(!worker.resources.poll(false)?);
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
    use crate::core::resource::ResourceKind;
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
    let worker = reopen(root.path())?;
    assert_eq!(
        call(&worker, &acquire("next", "successor")).await?["error"]["data"]["kind"],
        "conflict"
    );
    Ok(())
}
async fn call(worker: &Arc<Worker>, value: &Value) -> Result<Value> {
    Ok(serde_json::from_slice(
        &crate::rpc::server::Server::with_telemetry(
            worker.clone(),
            crate::telemetry::Telemetry::disabled(),
        )?
        .call(&serde_json::to_vec(value)?)
        .await?,
    )?)
}

fn context(trace_id: &str, parent_id: &str) -> wukong_worker_protocol::TraceContext {
    wukong_worker_protocol::TraceContext {
        traceparent: format!("00-{trace_id}-{parent_id}-01"),
        tracestate: Some("vendor=opaque".into()),
    }
}

async fn traced_call(
    worker: &Arc<Worker>,
    telemetry: &crate::telemetry::Telemetry,
    request: &str,
) -> Result<(Value, Option<wukong_worker_protocol::TelemetryBatch>)> {
    let response = crate::rpc::server::Server::with_telemetry(worker.clone(), telemetry.clone())?
        .call(request.as_bytes())
        .await?;
    let response = std::str::from_utf8(&response)?
        .strip_suffix('\n')
        .expect("native RPC response is newline framed");
    let extracted = wukong_worker_protocol::extract_response_telemetry(response)?;
    Ok((serde_json::from_str(&extracted.message)?, extracted.batch))
}

fn decode_traces(
    batch: &wukong_worker_protocol::TelemetryBatch,
) -> Result<opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest> {
    use base64::Engine as _;
    use prost::Message as _;
    let encoded = batch.traces.as_ref().expect("trace batch is present");
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
    Ok(
        opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest::decode(
            bytes.as_slice(),
        )?,
    )
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
    let worker = reopen(root.path())?;
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
    assert!(reopen(root.path()).is_err());
    assert_eq!(std::fs::read(epoch)?, b"corrupt");
    Ok(())
}

#[tokio::test]
async fn rpc_status_cancel_and_host_shutdown_share_the_initialized_resource_state() -> Result<()> {
    use crate::resources::journal::{Cleanup, Reservation};
    let (root, worker, resources) = fixture_state()?;
    // The host and the RPC entry must hold the same resource state, not copies.
    assert!(Arc::ptr_eq(&worker.resources, &resources));
    resources.initialize(crate::resources::state::tests::context(root.path()))?;
    let private = AclPolicy::private_vm();
    for name in [
        "metadata",
        "metadata/resource-requests",
        "metadata/resource-results",
    ] {
        private.create_directory(&root.path().join(name))?;
    }
    let lease = call(&worker, &acquire("run", "controller")).await?["result"].clone();
    let owner = Owner {
        identity: identity(),
        run_id: "run".into(),
        owner_id: "controller".into(),
        epoch: lease["epoch"].as_u64().unwrap(),
    };
    // A process-kind record without the native process contract makes the
    // automatic recovery intent refusable, so status polling reports the
    // unchanged journal blocker instead of spawning an owned cleanup host.
    let specification = json!({"trailing_fixture": true});
    let resource = {
        let mut state = resources.test_state().await;
        match state.test_journal().reserve_start(
            &owner,
            "status-once",
            ResourceKind::Process,
            &specification,
        )? {
            Reservation::Reserved {
                effect, resource, ..
            } => {
                state.test_journal().begin(&effect)?;
                resource
            }
            _ => anyhow::bail!("missing native status reservation"),
        }
    };
    // The host admission/shutdown check and RPC status observe one blocker.
    assert!(!resources.idle()?);
    assert!(resources.poll(false).is_err());
    let status = call(
        &worker,
        &request(
            "job.status",
            json!({"job_id":resource,"_lease":lease,"_binding":identity()}),
        ),
    )
    .await?;
    assert_eq!(status["result"]["job_id"], resource);
    assert_eq!(status["result"]["state"], "lost");
    assert_eq!(status["result"]["cleanup_pending"], true);
    // RPC cancellation requests the same owned resource without clearing it.
    let cancelled = call(
        &worker,
        &request(
            "job.cancel",
            json!({"job_id":resource,"_lease":lease,"_binding":identity()}),
        ),
    )
    .await?;
    assert!(cancelled.get("error").is_none(), "{cancelled}");
    assert!(
        root.path()
            .join("metadata/resource-requests")
            .join(format!("{resource}.cancel"))
            .exists()
    );
    resources.cancel().await?;
    assert!(!resources.idle()?);

    // The resource layer commits the final state under the original ticket.
    let status_document = {
        let mut state = resources.test_state().await;
        let record = state.test_journal().resource(&owner, &resource)?;
        json!({"job_id":resource,"kind":"process","state":"lost","native_owner":record.owner,
            "cleanup_pending":false,
            "result":{"outcome":"unknown","jobs_empty":true,"profiles_removed":true,
                "scratch_removed":true,"recovery_artifact":format!("recovery-{}", "a".repeat(32)),
                "recovery_receipt_sha256":"b".repeat(64)},
            "isolation":"appcontainer"})
    };
    let status_path = root
        .path()
        .join("metadata/resource-results")
        .join(format!("{resource}.status.json"));
    crate::artifacts::io::publish_json(&status_path, &status_document, &private)?;
    {
        let mut state = resources.test_state().await;
        state.test_journal().start_closing(&resource)?;
        state.test_journal().close(
            &resource,
            Cleanup {
                jobs_empty: true,
                profiles_removed: true,
                scratch_removed: true,
            },
        )?;
    }
    // RPC status and the host's leftover-cleanup verification now agree.
    assert!(resources.idle()?);
    assert!(resources.poll(false)?);
    resources
        .shutdown(
            tokio::time::Instant::now() + std::time::Duration::from_secs(1),
            || false,
        )
        .await?;
    let terminal = call(
        &worker,
        &request(
            "job.status",
            json!({"job_id":resource,"_lease":lease,"_binding":identity()}),
        ),
    )
    .await?;
    assert_eq!(terminal["result"], status_document);
    assert_eq!(terminal["result"]["cleanup_pending"], false);
    // Releasing the lease uses the same manager state; the successor starts a
    // new epoch only after the blocker is settled.
    let released = call(&worker, &release(&lease)).await?;
    assert_eq!(released["result"]["released"], true);
    let successor = call(&worker, &acquire("next", "controller")).await?["result"].clone();
    assert!(
        successor["epoch"].as_u64().unwrap() > lease["epoch"].as_u64().unwrap(),
        "{successor}"
    );
    Ok(())
}

#[tokio::test]
async fn rpc_trace_context_preserves_raw_params_and_exports_standard_otlp() -> Result<()> {
    use serde_json::value::RawValue;
    use std::collections::BTreeMap;

    let (_root, worker) = fixture()?;
    let (telemetry, _guard) = crate::telemetry::Telemetry::new();
    let trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
    let parent_id = "00f067aa0ba902b7";
    let parent = context(trace_id, parent_id);
    let raw = r#"{"jsonrpc":"2.0","id":7,"method":"file.put","params":{"request_id":"stable","call_id":"call"}}"#;
    let injected = wukong_worker_protocol::attach_trace_context(raw, Some(&parent))?;
    assert_eq!(
        injected.status,
        wukong_worker_protocol::EnvelopeStatus::Attached
    );
    let stripped = wukong_worker_protocol::extract_trace_context(&injected.message)?;
    assert_eq!(stripped.context, Some(parent));
    let before: BTreeMap<String, Box<RawValue>> = serde_json::from_str(raw)?;
    let after: BTreeMap<String, Box<RawValue>> = serde_json::from_str(&stripped.message)?;
    for member in ["jsonrpc", "id", "method", "params"] {
        assert_eq!(before[member].get(), after[member].get());
    }

    let expected = call(&worker, &serde_json::from_str(&stripped.message)?).await?;
    let (business, batch) = traced_call(&worker, &telemetry, &injected.message).await?;
    assert_eq!(business, expected);
    assert!(business.get("error").is_some());
    assert_eq!(business["id"], 7);
    let batch = batch.expect("sampled RPC returns a telemetry batch");
    assert!(batch.is_bounded());
    let decoded = decode_traces(&batch)?;
    let spans: Vec<_> = decoded
        .resource_spans
        .iter()
        .flat_map(|resource| &resource.scope_spans)
        .flat_map(|scope| &scope.spans)
        .collect();
    let server = spans
        .iter()
        .find(|span| span.name == "rpc.server")
        .expect("RPC server span is exported");
    let method = spans
        .iter()
        .find(|span| span.name == "file.put")
        .expect("registered method span is exported");
    assert_eq!(server.trace_id, hex::decode(trace_id)?);
    assert_eq!(server.parent_span_id, hex::decode(parent_id)?);
    assert_eq!(server.trace_state, "vendor=opaque");
    assert_eq!(method.trace_id, server.trace_id);
    assert_eq!(method.parent_span_id, server.span_id);
    assert!(spans.iter().all(|span| span.events.is_empty()));
    assert!(spans.iter().all(|span| span.links.is_empty()));
    assert!(
        spans.iter().all(|span| {
            span.attributes
                .iter()
                .all(|attribute| matches!(attribute.key.as_str(), "rpc.system" | "rpc.method"))
        }),
        "{spans:?}"
    );
    Ok(())
}

#[tokio::test]
async fn invalid_trace_context_is_dropped_without_changing_business_errors() -> Result<()> {
    let (_root, worker) = fixture()?;
    let (telemetry, _guard) = crate::telemetry::Telemetry::new();
    let raw = r#"{"jsonrpc":"2.0","id":8,"method":"file.get","params":{"path":"secret-target-path"},"_wukong_telemetry":{"traceparent":"00-00000000000000000000000000000000-00f067aa0ba902b7-01"}}"#;
    let stripped = wukong_worker_protocol::extract_trace_context(raw)?;
    assert!(stripped.dropped);
    assert!(stripped.context.is_none());
    let expected = call(&worker, &serde_json::from_str(&stripped.message)?).await?;
    let (actual, batch) = traced_call(&worker, &telemetry, raw).await?;
    assert_eq!(actual, expected);
    assert!(actual.get("error").is_some());
    let decoded = decode_traces(&batch.expect("invalid parent still yields local RPC spans"))?;
    let server = decoded
        .resource_spans
        .iter()
        .flat_map(|resource| &resource.scope_spans)
        .flat_map(|scope| &scope.spans)
        .find(|span| span.name == "rpc.server")
        .expect("RPC server span is exported");
    assert!(server.parent_span_id.is_empty());
    Ok(())
}

#[tokio::test]
async fn different_trace_contexts_do_not_change_operation_replay() -> Result<()> {
    let (_root, worker) = fixture()?;
    let (telemetry, _guard) = crate::telemetry::Telemetry::new();
    let put = request(
        "file.put",
        json!({"path":"replay.bin","data_base64":"YWJj","request_id":"stable"}),
    );
    let raw = serde_json::to_string(&put)?;
    let first = wukong_worker_protocol::attach_trace_context(
        &raw,
        Some(&context(
            "4bf92f3577b34da6a3ce929d0e0e4736",
            "00f067aa0ba902b7",
        )),
    )?;
    let second = wukong_worker_protocol::attach_trace_context(
        &raw,
        Some(&context(
            "6bf92f3577b34da6a3ce929d0e0e4736",
            "10f067aa0ba902b7",
        )),
    )?;
    let first_stripped = wukong_worker_protocol::extract_trace_context(&first.message)?;
    let second_stripped = wukong_worker_protocol::extract_trace_context(&second.message)?;
    assert_eq!(first_stripped.message, second_stripped.message);
    let (first_business, first_batch) = traced_call(&worker, &telemetry, &first.message).await?;
    let (second_business, second_batch) = traced_call(&worker, &telemetry, &second.message).await?;
    assert!(first_business.get("error").is_some());
    assert_eq!(first_business, second_business);
    assert_ne!(
        decode_traces(&first_batch.unwrap())?.resource_spans[0].scope_spans[0]
            .spans
            .iter()
            .map(|span| &span.trace_id)
            .collect::<Vec<_>>(),
        decode_traces(&second_batch.unwrap())?.resource_spans[0].scope_spans[0]
            .spans
            .iter()
            .map(|span| &span.trace_id)
            .collect::<Vec<_>>()
    );
    Ok(())
}
