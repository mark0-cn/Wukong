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
