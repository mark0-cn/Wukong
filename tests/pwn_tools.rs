use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};
use std::{fs, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wukong::{
    config::AppConfig,
    coordinator::Coordinator,
    domain::{Budget, Finding, FindingStage, Role, Verification},
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

async fn run_verification_job(
    service: &ToolService,
    verifier_id: &str,
    call_id: &str,
    args: Value,
) -> Result<String> {
    let started = service
        .execute(verifier_id, call_id, "poc_verify", args)
        .await?;
    let job = value(&started)["job_id"]
        .as_str()
        .context("verification job ID missing")?
        .to_owned();
    for index in 0..160 {
        let observed = service
            .execute(
                verifier_id,
                &format!("{call_id}-status-{index}"),
                "job_status",
                json!({"job_id":job}),
            )
            .await?;
        if value(&observed)["state"] == "completed" {
            return Ok(job);
        }
        assert_ne!(value(&observed)["state"], "failed", "{observed:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    anyhow::bail!("live worker verification did not complete")
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
        for _ in 0..6 {
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
    let service = ToolService::new(config, store.clone()).await?;
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
    let job = run_verification_job(&service, &verifier.id, "net-verify", args.clone()).await?;
    let record_args = json!({
        "finding_id":finding_id,"job_id":job,"impact":"arbitrary code execution"
    });
    let recorded = service
        .execute(
            &verifier.id,
            "net-record",
            "verification_record",
            record_args.clone(),
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
    let verification_id = record["id"].as_str().unwrap().to_owned();
    let linked = store.require::<Finding>("finding", finding_id)?;
    assert_eq!(
        linked.verification_id.as_deref(),
        Some(verification_id.as_str())
    );
    assert_eq!(linked.stage, FindingStage::Suspected);
    let inspected = service
        .execute(
            &producer.id,
            "net-finding-get",
            "finding_get",
            json!({"id":finding_id}),
        )
        .await?;
    assert_eq!(value(&inspected)["verification"]["id"], verification_id);

    let replayed = service
        .execute(
            &verifier.id,
            "net-record",
            "verification_record",
            record_args.clone(),
        )
        .await?;
    assert_eq!(replayed.value, recorded.value);

    let mut unlinked = store.require::<Finding>("finding", finding_id)?;
    unlinked.verification_id = None;
    store.save_record(&unlinked)?;
    let repaired = service
        .execute(
            &verifier.id,
            "net-record-repair",
            "verification_record",
            record_args,
        )
        .await?;
    assert_eq!(value(&repaired)["id"], verification_id);
    let repaired_finding = store.require::<Finding>("finding", finding_id)?;
    assert_eq!(
        repaired_finding.verification_id.as_deref(),
        Some(verification_id.as_str())
    );
    assert_eq!(repaired_finding.stage, FindingStage::Suspected);

    let conflicting_job =
        run_verification_job(&service, &verifier.id, "net-verify-conflict", args.clone()).await?;
    let conflict = service
        .execute(
            &verifier.id,
            "net-record-conflict",
            "verification_record",
            json!({"finding_id":finding_id,"job_id":conflicting_job}),
        )
        .await?;
    assert!(conflict.is_error);
    assert!(
        conflict.value["result"]["error"]
            .as_str()
            .unwrap()
            .contains("different valid passing verification")
    );
    let after_conflict = store.require::<Finding>("finding", finding_id)?;
    assert_eq!(
        after_conflict.verification_id.as_deref(),
        Some(verification_id.as_str())
    );
    assert_eq!(after_conflict.stage, FindingStage::Suspected);
    assert_eq!(
        store
            .records::<Verification>(&run.id)?
            .into_iter()
            .filter(|record| record.finding_id == finding_id)
            .count(),
        1
    );

    let failed_finding = service.execute(&producer.id,"net-failed-finding","finding_file",json!({
        "title":"Unmatched service behavior","description":"The expected marker is deliberately absent",
        "evidence_ids":[input.value["evidence_id"]]
    })).await?;
    let failed_finding_id = value(&failed_finding)["finding"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut failed_args = args;
    failed_args["finding_id"] = json!(failed_finding_id);
    failed_args["network"]["steps"][3]["contains_base64"] =
        json!(BASE64.encode(b"not-network-marker"));
    let failed_job =
        run_verification_job(&service, &verifier.id, "net-verify-failed", failed_args).await?;
    let failed = service
        .execute(
            &verifier.id,
            "net-record-failed",
            "verification_record",
            json!({"finding_id":failed_finding_id,"job_id":failed_job}),
        )
        .await?;
    assert_eq!(value(&failed)["passed"], false);
    let failed_finding = store.require::<Finding>("finding", &failed_finding_id)?;
    assert!(failed_finding.verification_id.is_none());
    assert_eq!(failed_finding.stage, FindingStage::Suspected);
    server.await??;
    service.close().await;
    Ok(())
}
