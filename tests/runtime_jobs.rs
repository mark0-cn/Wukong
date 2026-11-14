use anyhow::Result;
use serde_json::{Value, json};
use std::time::Duration;
use wukong::runtime::{RuntimeClient, RuntimeConfig};

fn config(root: &std::path::Path) -> RuntimeConfig {
    RuntimeConfig {
        transport: wukong::runtime::TransportConfig::TrustedLocal(
            wukong::runtime::TrustedLocalConfig {
                state_dir: root.to_path_buf(),
                ..Default::default()
            },
        ),
        ..Default::default()
    }
}

async fn finished(client: &RuntimeClient, job: &Value) -> Result<Value> {
    for _ in 0..200 {
        let status = client
            .call("job.status", json!({"job_id": job["job_id"]}))
            .await?;
        if ["completed", "failed", "cancelled", "timed_out", "lost"]
            .contains(&status["state"].as_str().unwrap_or_default())
        {
            return Ok(status);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    anyhow::bail!("job did not reach a terminal state")
}

#[tokio::test]
async fn disabled_backend_never_executes() {
    assert!(
        RuntimeClient::connect(RuntimeConfig::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn capacity_queue_allows_closure_and_retries_only_explicit_pre_effect_rejections()
-> Result<()> {
    let root = tempfile::tempdir()?;
    let bridge = root.path().join("capacity_bridge.py");
    std::fs::write(
        &bridge,
        r#"
import json, sys
busy = True
calls = 0
effects = []
for line in sys.stdin:
    request = json.loads(line)
    method, params = request['method'], request.get('params', {})
    error = None
    if method == 'ping':
        result = {'identity':{'protocol':3,'installation_id':'capacity-fixture','boot_id':'boot','package_sha256':'0'*64}}
    elif method == 'job.start':
        calls += 1
        if params.get('request_id') == 'unknown':
            error = {'code':-32000,'message':'uncertain execution','data':{'kind':'outcome_unknown'}}
        elif busy:
            error = {'code':-32000,'message':'capacity occupied','data':{'kind':'resource_busy'}}
        else:
            effects.append(params['request_id'])
            result = {'job_id':params['request_id']}
    elif method == 'job.cancel':
        busy = False
        result = {'cancelled':True}
    else:
        result = {'calls':calls,'effects':effects}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],**({'error':error} if error else {'result':result})}),flush=True)
"#,
    )?;
    let client = RuntimeClient::connect(RuntimeConfig {
        transport: wukong::runtime::TransportConfig::TrustedLocal(
            wukong::runtime::TrustedLocalConfig {
                bridge_path: bridge,
                state_dir: root.path().to_path_buf(),
                ..Default::default()
            },
        ),
        request_timeout_secs: 3,
        ..Default::default()
    })
    .await?;
    let waiting = client.clone();
    let start = tokio::spawn(async move {
        waiting
            .call("job.start", json!({"request_id":"once"}))
            .await
    });
    let mut observed_busy = false;
    for _ in 0..50 {
        if client.call("job.status", json!({})).await?["calls"]
            .as_u64()
            .unwrap()
            > 0
        {
            observed_busy = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(observed_busy);
    assert!(!start.is_finished());
    client.call("job.cancel", json!({})).await?;
    assert_eq!(start.await??["job_id"], "once");
    assert_eq!(
        client.call("job.status", json!({})).await?["effects"],
        json!(["once"])
    );
    let before = client.call("job.status", json!({})).await?["calls"]
        .as_u64()
        .unwrap();
    let error = client
        .call("job.start", json!({"request_id":"unknown"}))
        .await
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<wukong::runtime::protocol::WorkerError>()
            .unwrap()
            .kind,
        wukong::runtime::protocol::WorkerErrorKind::OutcomeUnknown
    );
    assert_eq!(
        client.call("job.status", json!({})).await?["calls"],
        before + 1
    );
    client.close().await?;
    Ok(())
}

#[tokio::test]
async fn lost_transport_reconnects_and_timed_out_requests_are_not_replayed() -> Result<()> {
    // A fake bridge that answers the handshake, hangs forever on the first
    // "exec.run" request (forcing a controller timeout) and answers instantly
    // once its marker file proves a fresh instance was spawned.
    let root = tempfile::tempdir()?;
    let bridge = root.path().join("fake_bridge.py");
    std::fs::write(
        &bridge,
        r#"
import json, sys, time, pathlib
marker = pathlib.Path(sys.argv[sys.argv.index("--state") + 1]) / "slow-marker"
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "ping":
        result = {'identity':{'protocol':3,'installation_id':'fixture','boot_id':'fixture-boot','package_sha256':'0'*64}}
    elif method == "exec.run":
        if not marker.exists():
            marker.write_text("exec.run", encoding="utf-8")
            time.sleep(60)
        result = {"reconnected": True}
    else:
        result = {}
    print(json.dumps({"jsonrpc":"2.0", "id": request["id"], "result": result}), flush=True)
"#,
    )?;
    let config = RuntimeConfig {
        transport: wukong::runtime::TransportConfig::TrustedLocal(
            wukong::runtime::TrustedLocalConfig {
                state_dir: root.path().to_path_buf(),
                bridge_path: bridge,
                ..Default::default()
            },
        ),
        request_timeout_secs: 2,
        ..Default::default()
    };
    let client = RuntimeClient::connect(config).await?;
    let error = client.call("exec.run", json!({})).await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("timed out"), "{message}");
    // The next call replaces the poisoned transport; the hung first request
    // is not re-sent, and the fresh bridge instance answers immediately.
    let recovered = client.call("exec.run", json!({})).await?;
    assert_eq!(recovered["reconnected"], true);
    client.close().await?;
    Ok(())
}

#[tokio::test]
async fn jobs_survive_transport_restart_and_idempotency_is_enforced() -> Result<()> {
    let root = tempfile::tempdir()?;
    let client = RuntimeClient::connect(config(root.path())).await?;
    let params = json!({"argv":["python", "-c", "import time; time.sleep(0.7); print('reconnected')"],"request_id":"persist-test", "timeout_secs":5});
    let job = client.call("job.start", params.clone()).await?;
    client.close().await?;
    let replacement = RuntimeClient::connect(config(root.path())).await?;
    let repeated = replacement.call("job.start", params).await?;
    assert_eq!(job["job_id"], repeated["job_id"]);
    let status = finished(&replacement, &job).await?;
    assert_eq!(status["state"], "completed");
    assert!(
        status["result"]["stdout"]
            .as_str()
            .unwrap()
            .contains("reconnected")
    );
    assert!(replacement.call("job.start", json!({"argv":["python", "-c", "print('different')"],"request_id":"persist-test", "timeout_secs":5})).await.is_err());
    replacement.call("worker.shutdown", json!({})).await?;
    replacement.close().await?;
    Ok(())
}

#[tokio::test]
async fn output_limits_timeout_cancellation_and_paths_are_real() -> Result<()> {
    let root = tempfile::tempdir()?;
    let client = RuntimeClient::connect(config(root.path())).await?;
    let output = client
        .call(
            "job.start",
            json!({"argv":["python", "-c", "print('x'*200000)"],"output_limit":1024}),
        )
        .await?;
    let output = finished(&client, &output).await?;
    assert_eq!(output["state"], "completed");
    assert_eq!(output["result"]["output_truncated"], true);
    assert!(output["result"]["stdout"].as_str().unwrap().len() <= 1024);
    let timeout = client
        .call(
            "job.start",
            json!({"argv":["python", "-c", "import time; time.sleep(60)"],"timeout_secs":1}),
        )
        .await?;
    assert_eq!(finished(&client, &timeout).await?["state"], "timed_out");
    let cancellation = client
        .call(
            "job.start",
            json!({"argv":["python", "-c", "import time; time.sleep(60)"],"timeout_secs":60}),
        )
        .await?;
    client
        .call("job.cancel", json!({"job_id":cancellation["job_id"]}))
        .await?;
    assert_eq!(
        finished(&client, &cancellation).await?["state"],
        "cancelled"
    );
    assert!(
        client
            .call(
                "file.put",
                json!({"path":"../../outside", "data_base64":"eA=="})
            )
            .await
            .is_err()
    );
    assert!(
        client
            .call("file.get", json!({"path":"../outside"}))
            .await
            .is_err()
    );
    assert!(
        client
            .call(
                "job.start",
                json!({"argv":["python","-c","print(1)"],"cwd":"../"})
            )
            .await
            .is_err()
    );
    client.call("worker.shutdown", json!({})).await?;
    client.close().await?;
    Ok(())
}

#[tokio::test]
async fn worker_filters_model_credentials_and_declares_missing_capabilities() -> Result<()> {
    let root = tempfile::tempdir()?;
    let client = RuntimeClient::connect(config(root.path())).await?;
    let doctor = client.doctor().await?;
    assert_eq!(doctor["isolation"], "trusted_local");
    let job = client.call("job.start", json!({"argv":["python", "-c", "import os; print(','.join(k for k in os.environ if 'API_KEY' in k or 'TOKEN' in k))"]})).await?;
    let status = finished(&client, &job).await?;
    assert_eq!(status["result"]["stdout"].as_str().unwrap().trim(), "");
    assert!(client.call("job.start", json!({"argv":["python","-c","pass"],"env":{"OPENAI_API_KEY":"must_not_reach_worker"}})).await.is_err());
    assert!(client.call("poc.verify", json!({"target":"missing", "input":"missing", "producer_agent_id":"same","verifier_agent_id":"same","call_id":"verify"})).await.is_err());
    client.call("worker.shutdown", json!({})).await?;
    client.close().await?;
    Ok(())
}

#[tokio::test]
async fn interactive_sessions_are_persistent_serialized_and_owned() -> Result<()> {
    let root = tempfile::tempdir()?;
    let client = RuntimeClient::connect(config(root.path())).await?;
    let program = "import sys\nfor line in sys.stdin:\n    line=line.strip()\n    if line=='quit': break\n    print('echo:'+line, flush=True)\n";
    let session = client
        .call("interact.start", json!({"argv":["python", "-c", program]}))
        .await?;
    let session_id = session["session_id"].as_str().unwrap().to_owned();
    assert_eq!(session["state"], "running");
    let first = client
        .call(
            "interact.write",
            json!({"session_id":session_id, "data_base64":"aGVsbG8K", "read_timeout_secs":5}),
        )
        .await?;
    assert!(
        first["captured"].as_str().unwrap().contains("echo:hello"),
        "unexpected output: {first}"
    );
    assert_eq!(first["state"], "running");
    let second = client
        .call(
            "interact.write",
            json!({"session_id":session_id, "data_base64":"cXVpdAo=", "read_timeout_secs":5}),
        )
        .await?;
    assert_eq!(second["state"], "exited");
    assert_eq!(second["exit_code"], 0);
    let closed = client
        .call("interact.close", json!({"session_id":session_id}))
        .await?;
    assert_eq!(closed["state"], "closed");
    assert!(
        client
            .call(
                "interact.write",
                json!({"session_id":session_id, "data_base64":"eA=="}),
            )
            .await
            .is_err(),
        "writes to a closed session must fail"
    );
    assert!(
        client
            .call(
                "interact.write",
                json!({"session_id":"absent", "data_base64":"eA=="}),
            )
            .await
            .is_err(),
        "writes to an absent session must fail"
    );
    client.call("worker.shutdown", json!({})).await?;
    client.close().await?;
    Ok(())
}

#[tokio::test]
async fn rpc_errors_preserve_transport_but_broken_frames_force_reconnection() -> Result<()> {
    let root = tempfile::tempdir()?;
    let bridge = root.path().join("rpc_fixture.py");
    std::fs::write(
        &bridge,
        r#"
import json, sys, pathlib
root=pathlib.Path(sys.argv[sys.argv.index('--state')+1])
starts=root/'starts'
count=int(starts.read_text())+1 if starts.exists() else 1
starts.write_text(str(count))
for line in sys.stdin:
    request=json.loads(line)
    method=request['method']
    if method=='ping':
        reply={'jsonrpc':'2.0','id':request['id'],'result':{'identity':{'protocol':3,'installation_id':'fixture','boot_id':'fixture-boot','package_sha256':'0'*64}}}
    elif method=='file.stat':
        reply={'jsonrpc':'2.0','id':request['id'],'error':{'code':-32000,'message':'fixture rejection'}}
    elif method=='debug.command':
        (root/'executed').write_text('once')
        print('{invalid-json',flush=True)
        continue
    elif method=='file.read':
        sys.stdout.write('x'*(48*1024*1024+1));sys.stdout.flush()
        continue
    else:
        reply={'jsonrpc':'2.0','id':request['id'],'result':{'starts':count}}
    print(json.dumps(reply),flush=True)
"#,
    )?;
    let client = RuntimeClient::connect(RuntimeConfig {
        transport: wukong::runtime::TransportConfig::TrustedLocal(
            wukong::runtime::TrustedLocalConfig {
                state_dir: root.path().to_path_buf(),
                bridge_path: bridge,
                ..Default::default()
            },
        ),
        request_timeout_secs: 5,
        ..Default::default()
    })
    .await?;
    assert!(
        format!(
            "{:#}",
            client.call("file.stat", json!({})).await.unwrap_err()
        )
        .contains("fixture rejection")
    );
    assert_eq!(
        client.doctor().await?["starts"],
        1,
        "an RPC error must not poison a healthy transport"
    );
    assert!(client.call("debug.command", json!({})).await.is_err());
    assert_eq!(client.doctor().await?["starts"], 2);
    assert_eq!(
        std::fs::read_to_string(root.path().join("executed"))?,
        "once"
    );
    assert!(client.call("file.read", json!({})).await.is_err());
    assert_eq!(client.doctor().await?["starts"], 3);
    client.close().await?;
    Ok(())
}

#[tokio::test]
async fn bridge_startup_error_includes_captured_stderr() -> Result<()> {
    let root = tempfile::tempdir()?;
    let bridge = root.path().join("failed_bridge.py");
    std::fs::write(
        &bridge,
        "import sys\nsys.stderr.write('fixture: Docker engine unavailable\\n')\nsys.exit(1)\n",
    )?;
    let config = RuntimeConfig {
        transport: wukong::runtime::TransportConfig::TrustedLocal(
            wukong::runtime::TrustedLocalConfig {
                bridge_path: bridge,
                state_dir: root.path().to_path_buf(),
                ..Default::default()
            },
        ),
        request_timeout_secs: 2,
        ..Default::default()
    };
    let error = match RuntimeClient::connect(config).await {
        Ok(client) => {
            client.close().await?;
            anyhow::bail!("failing bridge unexpectedly connected");
        }
        Err(error) => error,
    };
    assert!(
        format!("{error:#}").contains("fixture: Docker engine unavailable"),
        "{error:#}"
    );
    Ok(())
}

#[tokio::test]
async fn reconnected_worker_identity_is_checked_before_resending_any_operation() -> Result<()> {
    let root = tempfile::tempdir()?;
    let bridge = root.path().join("identity_fixture.py");
    std::fs::write(
        &bridge,
        r#"import json,sys,pathlib
root=pathlib.Path(sys.argv[sys.argv.index('--state')+1])
for line in sys.stdin:
 r=json.loads(line)
 if r['method']=='job.status':
  print('{invalid-json',flush=True);continue
 if r['method']=='ping':
  boot=(root/'boot').read_text() if (root/'boot').exists() else 'first'
  result={'identity':{'protocol':3,'installation_id':'fixture','boot_id':boot,'package_sha256':'0'*64}}
 else:
  (root/'effect').write_text('executed');result={}
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':result}),flush=True)
"#,
    )?;
    let client = RuntimeClient::connect(RuntimeConfig {
        transport: wukong::runtime::TransportConfig::TrustedLocal(
            wukong::runtime::TrustedLocalConfig {
                bridge_path: bridge,
                state_dir: root.path().to_path_buf(),
                ..Default::default()
            },
        ),
        ..Default::default()
    })
    .await?;
    assert!(client.call("job.status", json!({})).await.is_err());
    std::fs::write(root.path().join("boot"), "second")?;
    let error = client.call("job.cancel", json!({})).await.unwrap_err();
    assert!(format!("{error:#}").contains("identity changed"));
    assert!(!root.path().join("effect").exists());
    client.close().await?;
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn desktop_runtime_does_not_inherit_invalid_console_stderr() -> Result<()> {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(kind: u32) -> *mut c_void;
        fn SetStdHandle(kind: u32, handle: *mut c_void) -> i32;
    }
    // Keep process-wide Win32 handle changes out of the parallel test runner.
    if std::env::var_os("WUKONG_STDERR_TEST_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "desktop_runtime_does_not_inherit_invalid_console_stderr",
                "--nocapture",
            ])
            .env("WUKONG_STDERR_TEST_CHILD", "1")
            .output()?;
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let bridge = root.path().join("desktop_bridge.py");
    std::fs::write(
        &bridge,
        r#"
import json, sys
sys.stderr.write('x' * 131072)
sys.stderr.flush()
for line in sys.stdin:
    request = json.loads(line)
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': {'ready': True,'identity':{'protocol':3,'installation_id':'fixture','boot_id':'fixture-boot','package_sha256':'0'*64}}}), flush=True)
"#,
    )?;
    let config = RuntimeConfig {
        transport: wukong::runtime::TransportConfig::TrustedLocal(
            wukong::runtime::TrustedLocalConfig {
                bridge_path: bridge,
                state_dir: root.path().to_path_buf(),
                ..Default::default()
            },
        ),
        request_timeout_secs: 3,
        ..Default::default()
    };
    let previous = unsafe { GetStdHandle((-12i32) as u32) };
    assert_ne!(
        unsafe { SetStdHandle((-12i32) as u32, 0x12345678usize as *mut c_void) },
        0
    );
    // This models the stale standard handle left by the detached desktop console.
    let legacy = std::process::Command::new("python")
        .args(["-c", "pass"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn();
    let result = RuntimeClient::connect(config).await;
    unsafe {
        SetStdHandle((-12i32) as u32, previous);
    }
    assert!(
        legacy.is_err(),
        "fixture must reproduce the inherited-handle failure"
    );
    let client = result?;
    assert_eq!(client.call("doctor", json!({})).await?["ready"], true);
    client.close().await?;
    Ok(())
}
