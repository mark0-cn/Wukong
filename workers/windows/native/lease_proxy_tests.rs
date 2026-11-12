use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn identity() -> Identity {
    Identity {
        protocol: 2,
        installation_id: "a".repeat(32),
        boot_id: format!("windows:{}:40", "b".repeat(32)),
        package_sha256: "c".repeat(64),
    }
}
fn fixture() -> Result<(tempfile::TempDir, LeaseProxy)> {
    let temp = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(temp.path(), true)?;
    let proxy = LeaseProxy::with_policy(identity(), temp.path(), policy)?;
    Ok((temp, proxy))
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
async fn upstream(bytes: Vec<u8>) -> Result<Vec<u8>> {
    let value: Value = serde_json::from_slice(&bytes)?;
    let result = match value["method"].as_str().unwrap() {
        "file.put" => {
            use base64::Engine;
            use sha2::Digest;
            assert_eq!(value["params"]["_lease"]["token"], "d".repeat(32));
            let data = base64::engine::general_purpose::STANDARD
                .decode(value["params"]["data_base64"].as_str().unwrap())?;
            json!({"path":format!("files/{}",value["params"]["path"].as_str().unwrap()),
                "sha256":hex::encode(sha2::Sha256::digest(&data)),"size":data.len()})
        }
        "environment.lease.acquire" => json!({"epoch":900,"run_id":value["params"]["run_id"],
            "owner_id":value["params"]["owner_id"],"boot_id":identity().boot_id,"token":"d".repeat(32),"expires_at":9999.0}),
        "environment.lease.release" => {
            assert_eq!(value["params"]["lease"]["token"], "d".repeat(32));
            json!({"released":true,"epoch":900})
        }
        _ => {
            assert_eq!(value["params"]["_lease"]["token"], "d".repeat(32));
            json!({"fixture":true})
        }
    };
    encode(&json!({"jsonrpc":"2.0","id":value["id"],"result":result}))
}
async fn call(proxy: &LeaseProxy, value: &Value) -> Result<Value> {
    Ok(serde_json::from_slice(
        &proxy.call(&encode(value)?, upstream).await?,
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
        json!({"request_id":"stable","_lease":lease,"_binding":identity()}),
    )
}
fn expire(proxy: &LeaseProxy) {
    proxy
        .state
        .lock()
        .unwrap()
        .current
        .as_mut()
        .unwrap()
        .deadline = Instant::now() - Duration::from_secs(1);
}

#[tokio::test]
async fn protected_epochs_survive_service_restart_and_never_expose_inner_tokens() -> Result<()> {
    let (temp, proxy) = fixture()?;
    let first = call(&proxy, &acquire("run", "controller")).await?["result"].clone();
    assert_eq!(first["epoch"], 1);
    assert_ne!(first["token"], "d".repeat(32));
    assert_eq!(
        call(&proxy, &effect(&first)).await?["result"]["fixture"],
        true
    );
    let renewed = call(&proxy, &acquire("run", "controller")).await?["result"].clone();
    assert_eq!(renewed["token"], first["token"]);
    assert_eq!(renewed["epoch"], first["epoch"]);
    assert_eq!(
        call(&proxy, &acquire("other", "controller")).await?["error"]["data"]["kind"],
        "conflict"
    );
    assert_eq!(call(&proxy, &release(&first)).await?["result"]["epoch"], 1);
    assert!(call(&proxy, &effect(&first)).await?.get("error").is_some());
    drop(proxy);
    let next = LeaseProxy::with_policy(identity(), temp.path(), AclPolicy::current_user()?)?;
    let second = call(&next, &acquire("other", "controller")).await?["result"].clone();
    assert_eq!(second["epoch"], 2);
    assert!(call(&next, &effect(&first)).await?.get("error").is_some());
    Ok(())
}

#[tokio::test]
async fn renewal_does_not_wait_for_effects_and_expiry_does_not_prove_quiescence() -> Result<()> {
    let (_temp, proxy) = fixture()?;
    let proxy = Arc::new(proxy);
    let token = call(&proxy, &acquire("run", "controller")).await?["result"].clone();
    let entered = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let p = proxy.clone();
    let e = entered.clone();
    let f = finish.clone();
    let req = effect(&token);
    let task = tokio::spawn(async move {
        p.call(&encode(&req)?, |bytes| {
            let e = e.clone();
            let f = f.clone();
            async move {
                e.notify_one();
                f.notified().await;
                upstream(bytes).await
            }
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await?;
    let renewed = tokio::time::timeout(
        Duration::from_secs(2),
        call(&proxy, &acquire("run", "controller")),
    )
    .await??;
    assert_eq!(renewed["result"]["token"], token["token"]);
    assert!(call(&proxy, &release(&token)).await?.get("error").is_some());
    expire(&proxy);
    assert!(
        call(&proxy, &acquire("next", "controller"))
            .await?
            .get("error")
            .is_some()
    );
    finish.notify_one();
    task.await??;
    assert_eq!(
        call(&proxy, &acquire("next", "controller")).await?["result"]["epoch"],
        2
    );
    let forwarded = AtomicUsize::new(0);
    proxy
        .call(&encode(&effect(&token))?, |bytes| {
            forwarded.fetch_add(1, Ordering::SeqCst);
            upstream(bytes)
        })
        .await?;
    assert_eq!(
        forwarded.load(Ordering::SeqCst),
        0,
        "stale fence must fail before a cached receipt lookup"
    );
    Ok(())
}

#[tokio::test]
async fn transport_failure_and_cancelled_futures_block_idle_takeover() -> Result<()> {
    let (_temp, proxy) = fixture()?;
    let proxy = Arc::new(proxy);
    let token = call(&proxy, &acquire("run", "controller")).await?["result"].clone();
    let p = proxy.clone();
    let req = effect(&token);
    let entered = Arc::new(tokio::sync::Notify::new());
    let e = entered.clone();
    let task = tokio::spawn(async move {
        p.call(&encode(&req)?, |_| {
            let e = e.clone();
            async move {
                e.notify_one();
                std::future::pending::<Result<Vec<u8>>>().await
            }
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await?;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(proxy.state.lock().unwrap().inflight, 0);
    assert!(proxy.state.lock().unwrap().uncertain);
    assert!(call(&proxy, &release(&token)).await?.get("error").is_some());
    expire(&proxy);
    assert!(
        call(&proxy, &acquire("run", "successor"))
            .await?
            .get("error")
            .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn binding_mismatch_and_invalid_inner_responses_never_issue_a_fence() -> Result<()> {
    let (_temp, proxy) = fixture()?;
    let mut wrong = acquire("run", "controller");
    wrong["params"]["_binding"]["package_sha256"] = "e".repeat(64).into();
    let forwarded = AtomicUsize::new(0);
    let reply = proxy
        .call(&encode(&wrong)?, |bytes| {
            forwarded.fetch_add(1, Ordering::SeqCst);
            upstream(bytes)
        })
        .await?;
    assert_eq!(
        serde_json::from_slice::<Value>(&reply)?["error"]["data"]["kind"],
        "binding_changed"
    );
    assert_eq!(forwarded.load(Ordering::SeqCst), 0);
    proxy
        .call(&encode(&acquire("run", "controller"))?, |_| async {
            encode(&json!({"jsonrpc":"2.0","id":1,"result":{"not":"a lease"}}))
        })
        .await?;
    assert!(proxy.state.lock().unwrap().current.is_none());
    assert_eq!(
        call(&proxy, &acquire("run", "controller")).await?["result"]["epoch"],
        1
    );
    Ok(())
}

#[test]
fn journal_rejects_concurrent_writer_partial_records_and_noncanonical_names() -> Result<()> {
    let (temp, proxy) = fixture()?;
    let policy = AclPolicy::current_user()?;
    assert!(LeaseProxy::with_policy(identity(), temp.path(), policy.clone()).is_err());
    drop(proxy);
    let path = temp
        .path()
        .join("lease-epochs/epoch-00000000000000000001.json");
    use std::io::Write;
    policy.create_file(&path)?.write_all(b"{")?;
    assert!(LeaseProxy::with_policy(identity(), temp.path(), policy.clone()).is_err());
    // Retain the corrupt fixture; use a separate owned directory for alias checks.
    let second = tempfile::tempdir()?;
    policy.apply(second.path(), true)?;
    let mut epochs = Epochs::open(second.path(), policy.clone())?;
    epochs.reserve()?;
    drop(epochs);
    fs::rename(
        second
            .path()
            .join("lease-epochs/epoch-00000000000000000001.json"),
        second.path().join("lease-epochs/epoch-1.json"),
    )?;
    assert!(Epochs::open(second.path(), policy).is_err());
    Ok(())
}

#[tokio::test]
async fn lost_or_malformed_effect_replies_remain_uncertain_but_rpc_errors_settle() -> Result<()> {
    for malformed in [false, true] {
        let (_temp, proxy) = fixture()?;
        let token = call(&proxy, &acquire("run", "controller")).await?["result"].clone();
        proxy
            .call(&encode(&effect(&token))?, |_| async move {
                if malformed {
                    Ok(b"not-json\n".to_vec())
                } else {
                    anyhow::bail!("controlled transport failure")
                }
            })
            .await?;
        assert!(proxy.state.lock().unwrap().uncertain);
        assert_eq!(
            call(&proxy, &release(&token)).await?["error"]["data"]["kind"],
            "conflict"
        );
    }
    let (_temp, proxy) = fixture()?;
    let token = call(&proxy, &acquire("run", "controller")).await?["result"].clone();
    proxy.call(&encode(&effect(&token))?, |_| async {
        encode(&json!({"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"controlled fixture error","data":{"kind":"invalid_request"}}}))
    }).await?;
    assert!(!proxy.state.lock().unwrap().uncertain);
    assert_eq!(
        call(&proxy, &release(&token)).await?["result"]["released"],
        true
    );
    Ok(())
}

#[tokio::test]
async fn delayed_renewal_cannot_resurrect_an_expired_outer_fence() -> Result<()> {
    let (_temp, proxy) = fixture()?;
    let first = call(&proxy, &acquire("run", "controller")).await?["result"].clone();
    let reply = proxy
        .call(&encode(&acquire("run", "controller"))?, |bytes| {
            let proxy = &proxy;
            async move {
                expire(proxy);
                upstream(bytes).await
            }
        })
        .await?;
    assert_eq!(
        serde_json::from_slice::<Value>(&reply)?["error"]["data"]["kind"],
        "lease_lost"
    );
    let next = call(&proxy, &acquire("run", "controller")).await?["result"].clone();
    assert_eq!(next["epoch"], 2);
    assert_ne!(next["token"], first["token"]);
    Ok(())
}

fn put_request(lease: &Value) -> Value {
    request(
        "file.put",
        json!({"request_id":"native-upload","path":"targets/sample.bin",
        "data_base64":"AAECAw==","executable":true,"_lease":lease,"_binding":identity()}),
    )
}

#[tokio::test]
async fn only_fenced_controller_bytes_receive_stable_native_input_proof() -> Result<()> {
    let (directory, proxy) = fixture()?;
    let lease = call(&proxy, &acquire("run", "controller")).await?["result"].clone();
    let original = call(&proxy, &put_request(&lease)).await?;
    let proof = &original["result"]["native_input"];
    assert_eq!(proof["epoch"], lease["epoch"]);
    assert_eq!(proof["run_id"], "run");
    assert_eq!(proof["owner_id"], "controller");
    assert_eq!(proof["identity"], serde_json::to_value(identity())?);
    assert_eq!(proof["executable"], true);
    assert_eq!(call(&proxy, &put_request(&lease)).await?, original);
    call(&proxy, &release(&lease)).await?;
    let forwarded = AtomicUsize::new(0);
    let rejected: Value = serde_json::from_slice(
        &proxy
            .call(&encode(&put_request(&lease))?, |bytes| {
                forwarded.fetch_add(1, Ordering::SeqCst);
                upstream(bytes)
            })
            .await?,
    )?;
    assert_eq!(rejected["error"]["data"]["kind"], "lease_lost");
    assert_eq!(forwarded.load(Ordering::SeqCst), 0);
    drop(proxy);
    let next = LeaseProxy::with_policy(identity(), directory.path(), AclPolicy::current_user()?)?;
    let successor = call(&next, &acquire("run", "controller")).await?["result"].clone();
    let new = call(&next, &put_request(&successor)).await?;
    assert_ne!(new["result"]["native_input"]["id"], proof["id"]);
    Ok(())
}

#[tokio::test]
async fn rejected_uploads_settle_before_forwarding_but_bad_success_receipts_block_idle()
-> Result<()> {
    let (_directory, proxy) = fixture()?;
    let lease = call(&proxy, &acquire("run", "controller")).await?["result"].clone();
    for path in ["../metadata/authority.json", "NUL"] {
        let mut invalid = put_request(&lease);
        invalid["params"]["path"] = json!(path);
        let reply = proxy
            .call(&encode(&invalid)?, |_| async {
                panic!("invalid upload reached worker")
            })
            .await?;
        assert!(
            serde_json::from_slice::<Value>(&reply)?
                .get("error")
                .is_some()
        );
        assert!(!proxy.state.lock().unwrap().uncertain);
    }
    let reply = proxy
        .call(&encode(&put_request(&lease))?, |bytes| async move {
            let request: Value = serde_json::from_slice(&bytes)?;
            encode(&json!({"jsonrpc":"2.0","id":request["id"],"result":{
            "path":"files/targets/sample.bin","sha256":"f".repeat(64),"size":4}}))
        })
        .await?;
    assert!(
        serde_json::from_slice::<Value>(&reply)?
            .get("error")
            .is_some()
    );
    assert!(proxy.state.lock().unwrap().uncertain);
    assert_eq!(
        call(&proxy, &release(&lease)).await?["error"]["data"]["kind"],
        "conflict"
    );
    Ok(())
}

#[tokio::test]
async fn native_upload_serialization_does_not_block_lease_renewal() -> Result<()> {
    let (_directory, proxy) = fixture()?;
    let proxy = Arc::new(proxy);
    let lease = call(&proxy, &acquire("run", "controller")).await?["result"].clone();
    let entered = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let p = proxy.clone();
    let e = entered.clone();
    let f = finish.clone();
    let req = put_request(&lease);
    let task = tokio::spawn(async move {
        p.call(&encode(&req)?, |bytes| {
            let e = e.clone();
            let f = f.clone();
            async move {
                e.notify_one();
                f.notified().await;
                upstream(bytes).await
            }
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await?;
    let renewed = tokio::time::timeout(
        Duration::from_secs(2),
        call(&proxy, &acquire("run", "controller")),
    )
    .await??;
    assert_eq!(renewed["result"]["token"], lease["token"]);
    assert_eq!(
        call(&proxy, &release(&lease)).await?["error"]["data"]["kind"],
        "conflict"
    );
    finish.notify_one();
    let result: Value = serde_json::from_slice(&task.await??)?;
    assert_eq!(result["result"]["native_input"]["epoch"], lease["epoch"]);
    Ok(())
}
