use anyhow::Result;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};
use std::{fs, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wukong::{
    config::AppConfig,
    coordinator::Coordinator,
    domain::{Budget, Role},
    store::Store,
    tools::{ToolOutcome, ToolService},
};

fn value(outcome: &ToolOutcome) -> &Value {
    assert!(!outcome.is_error, "{:?}", outcome.value);
    &outcome.value["result"]
}
fn worker_config(path: &std::path::Path) -> AppConfig {
    AppConfig {
        default_environment: Some("fixture".into()),
        environments: std::collections::BTreeMap::from([(
            "fixture".into(),
            wukong::environments::EnvironmentConfig {
                name: "Trusted fixture".into(),
                platform: wukong::environments::Platform::current(),
                installation_id: None,
                runtime: wukong::runtime::RuntimeConfig {
                    transport: wukong::runtime::TransportConfig::TrustedLocal(
                        wukong::runtime::TrustedLocalConfig {
                            state_dir: path.join("worker"),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                },
            },
        )]),
        ..Default::default()
    }
}

#[tokio::test]
async fn seccomp_artifacts_are_scoped_hashed_and_unavailable_to_root() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("state"))?;
    let coordinator = Coordinator::new(store.clone());
    let sample = temp.path().join("sample");
    fs::write(&sample, b"trusted static fixture")?;
    let config = worker_config(temp.path());
    let run = coordinator.create_run(
        &sample,
        "filter analysis",
        Budget::default(),
        serde_json::to_value(&config)?,
    )?;
    let analyst = coordinator
        .create_child(
            &run.root_id,
            serde_json::from_value(json!({
                "name":"analyst","role":"analyst","scope":"filter","task":"inspect filter"
            }))?,
        )?
        .agent;
    let service = ToolService::new(config, store).await?;
    let raw = [
        6u16.to_le_bytes().as_slice(),
        &[0, 0],
        &0x7fff0000u32.to_le_bytes(),
    ]
    .concat();
    let put = service
        .execute(
            &analyst.id,
            "filter-put",
            "file_put",
            json!({"path":"filters/allow.bpf","data_base64":BASE64.encode(raw)}),
        )
        .await?;
    let path = value(&put)["file"]["path"].as_str().unwrap();
    let args = json!({"filter":path,"probes":[{"arch":0xc000003eu32,"nr":1,"args":[1]}]});
    let denied = service
        .execute(&run.root_id, "root-filter", "seccomp_analyze", args.clone())
        .await?;
    assert!(denied.is_error);
    let analyzed = service
        .execute(&analyst.id, "analyze-filter", "seccomp_analyze", args)
        .await?;
    assert_eq!(value(&analyzed)["probes"][0]["decision"]["action"], "ALLOW");
    assert_eq!(
        value(&analyzed)["filter_sha256"],
        value(&put)["file"]["sha256"]
    );
    assert_eq!(value(&analyzed)["runtime_binding"], "unconfirmed_artifact");
    let unknown = service
        .execute(
            &analyst.id,
            "unknown-libc",
            "libc_identify",
            json!({"leaked":4096,"candidates":["unregistered.so"]}),
        )
        .await?;
    assert!(unknown.is_error);
    service.close().await;
    Ok(())
}

#[tokio::test]
async fn network_verification_imports_transcripts_and_preserves_identity_limits() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("state"))?;
    let coordinator = Coordinator::new(store.clone());
    let sample = temp.path().join("sample");
    fs::write(&sample, b"trusted network fixture identity; not executed")?;
    let config = worker_config(temp.path());
    let run = coordinator.create_run(
        &sample,
        "verify service",
        Budget::default(),
        serde_json::to_value(&config)?,
    )?;
    let mut agents = Vec::new();
    for (name, role) in [("producer", Role::Analyst), ("verifier", Role::Verifier)] {
        agents.push(
            coordinator
                .create_child(
                    &run.root_id,
                    serde_json::from_value(json!({
                        "name":name,"role":role,"task":"check service","scope":name
                    }))?,
                )?
                .agent,
        );
    }
    let producer = &agents[0];
    let verifier = &agents[1];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await?;
            socket.write_all(b"ready> ").await?;
            let mut bytes = [0; 4];
            socket.read_exact(&mut bytes).await?;
            anyhow::ensure!(&bytes == b"PING", "unexpected protocol input");
            socket.write_all(b"ok:network-marker\n").await?;
        }
        Ok::<_, anyhow::Error>(())
    });
    let worker = temp.path().join("worker");
    let base = worker.join("files/challenge");
    fs::create_dir_all(&base)?;
    fs::copy(&sample, base.join("target"))?;
    fs::write(
        base.join("launch.json"),
        serde_json::to_vec(&json!({
            "schema_version":1,"challenge_id":"trusted-loopback","base":"files/challenge","target":"target",
            "files":{"target":run.target_sha256},"service":{"mode":"local_service","port":port,"host":null}
        }))?,
    )?;
    let service = ToolService::new(config, store).await?;
    let staged = service
        .execute(&producer.id, "net-stage", "stage_target", json!({}))
        .await?;
    let target = value(&staged)["path"].as_str().unwrap();
    let input = service
        .execute(
            &producer.id,
            "net-input",
            "file_put",
            json!({"path":"inputs/protocol","text":"PING"}),
        )
        .await?;
    let input_path = value(&input)["file"]["path"].as_str().unwrap();
    let finding = service.execute(&producer.id,"net-finding","finding_file",json!({
        "title":"Observed service behavior","description":"A response predicate, not code execution",
        "evidence_ids":[input.value["evidence_id"]]
    })).await?;
    let finding_id = value(&finding)["finding"]["id"].as_str().unwrap();
    let args = json!({"finding_id":finding_id,"target":target,"input":input_path,"trials":2,
    "launch_spec":"files/challenge/launch.json","network":{"steps":[
        {"op":"receive","until_base64":BASE64.encode(b"ready> "),"max_bytes":64},
        {"op":"send","offset":0,"length":4},
        {"op":"receive","until_base64":BASE64.encode(b"\n"),"max_bytes":64},
        {"op":"expect","contains_base64":BASE64.encode(b"ok:network-marker")}
    ]}});
    let denied = service
        .execute(&producer.id, "net-self-verify", "poc_verify", args.clone())
        .await?;
    assert!(denied.is_error);
    let started = service
        .execute(&verifier.id, "net-verify", "poc_verify", args)
        .await?;
    let job = value(&started)["job_id"].as_str().unwrap();
    let mut completed = false;
    for index in 0..160 {
        let observed = service
            .execute(
                &verifier.id,
                &format!("net-status-{index}"),
                "job_status",
                json!({"job_id":job}),
            )
            .await?;
        if value(&observed)["state"] == "completed" {
            completed = true;
            break;
        }
        assert_ne!(value(&observed)["state"], "failed", "{observed:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(completed);
    server.await??;
    let recorded = service
        .execute(
            &verifier.id,
            "net-record",
            "verification_record",
            json!({
                "finding_id":finding_id,"job_id":job,"impact":"arbitrary code execution"
            }),
        )
        .await?;
    let record = value(&recorded);
    assert_eq!(record["passed"], true);
    assert_eq!(record["successes"], 2);
    assert!(
        record["impact"]
            .as_str()
            .unwrap()
            .contains("runtime identity is unconfirmed")
    );
    assert!(record["evidence_ids"].as_array().unwrap().len() >= 3);
    service.close().await;
    Ok(())
}
