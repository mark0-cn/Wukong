use anyhow::Result;
use re_agent::runtime::{RuntimeBackend, RuntimeClient, RuntimeConfig};
use serde_json::{Value, json};
use std::time::Duration;

fn config(root: &std::path::Path) -> RuntimeConfig {
    RuntimeConfig {
        backend: RuntimeBackend::TrustedLocal,
        state_dir: root.to_path_buf(),
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
async fn lost_transport_reconnects_and_timed_out_requests_are_not_replayed() -> Result<()> {
    // A fake bridge that answers the handshake, hangs forever on the first
    // "slow" request (forcing a controller timeout) and answers instantly
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
    if method == "slow":
        if not marker.exists():
            marker.write_text("slow", encoding="utf-8")
            time.sleep(60)
        result = {"reconnected": True}
    else:
        result = {}
    print(json.dumps({"id": request["id"], "result": result}), flush=True)
"#,
    )?;
    let config = RuntimeConfig {
        backend: RuntimeBackend::TrustedLocal,
        state_dir: root.path().to_path_buf(),
        bridge_path: bridge,
        request_timeout_secs: 2,
        ..Default::default()
    };
    let client = RuntimeClient::connect(config).await?;
    let error = client.call("slow", json!({})).await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("timed out"), "{message}");
    // The next call replaces the poisoned transport; the hung first request
    // is not re-sent, and the fresh bridge instance answers immediately.
    let recovered = client.call("slow", json!({})).await?;
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
