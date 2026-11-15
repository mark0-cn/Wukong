use anyhow::Result;
use serde_json::json;
use std::collections::BTreeMap;
use wukong::{
    config::AppConfig,
    coordinator::{ChildRequest, Coordinator},
    domain::{AgentMetadata, Budget, Role, WorkerFileRecord},
    environments::{EnvironmentConfig, EnvironmentRegistry, Platform},
    runtime::RuntimeConfig,
    store::Store,
    tools::ToolService,
};

fn environment(root: &std::path::Path) -> EnvironmentConfig {
    EnvironmentConfig {
        installation_id: None,
        name: "Trusted development fixture".into(),
        platform: Platform::current(),
        runtime: RuntimeConfig {
            transport: wukong::runtime::TransportConfig::TrustedLocal(
                wukong::runtime::TrustedLocalConfig {
                    state_dir: root.into(),
                    ..Default::default()
                },
            ),
            ..Default::default()
        },
    }
}

fn child(environment_id: Option<&str>) -> ChildRequest {
    ChildRequest {
        name: "specialist".into(),
        role: Role::Analyst,
        task: "inspect fixture".into(),
        scope: "shared scope".into(),
        environment_id: environment_id.map(str::to_owned),
        contract: Default::default(),
        skills: vec![],
        inherit_context: false,
        wait_for_predecessors: vec![],
        dependency_policy: Default::default(),
    }
}

#[test]
fn binding_is_inherited_validated_and_participates_in_deduplication() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("state"))?;
    let coordinator = Coordinator::new(store.clone());
    let sample = temp.path().join("sample");
    std::fs::write(&sample, b"not executable")?;
    let config = json!({"default_environment":"linux", "environments":{"linux":{},"windows":{}}});
    let run = coordinator.create_run(&sample, "inspect", Budget::default(), config)?;
    let root: AgentMetadata = store.load(&run.root_id)?;
    assert_eq!(root.environment_id.as_deref(), Some("linux"));
    let first = coordinator.create_child(&root.id, child(None))?;
    assert_eq!(first.agent.environment_id.as_deref(), Some("linux"));
    assert!(
        !coordinator
            .create_child(&root.id, child(Some("linux")))?
            .created
    );
    let second = coordinator.create_child(&root.id, child(Some("windows")))?;
    assert!(second.created);
    assert_ne!(first.agent.id, second.agent.id);
    let nested = coordinator.create_child(&second.agent.id, child(None))?;
    assert_eq!(nested.agent.environment_id.as_deref(), Some("windows"));
    assert!(
        coordinator
            .create_child(&root.id, child(Some("outside")))
            .is_err()
    );
    assert!(
        coordinator
            .create_child(&root.id, child(Some("../escape")))
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn registration_is_lazy_and_missing_workers_do_not_block_other_environments() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut absent = environment(&temp.path().join("absent"));
    absent.runtime.trusted_local_mut().unwrap().python =
        temp.path().join("missing-python.exe").display().to_string();
    let registry = EnvironmentRegistry::new(BTreeMap::from([
        (
            "available".into(),
            environment(&temp.path().join("available")),
        ),
        ("absent".into(), absent),
    ]))?;
    assert!(
        registry
            .list()
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["connected"] == false)
    );
    assert!(registry.client("unknown").await.is_err());
    assert!(registry.client("absent").await.is_err());
    let client = registry.client("available").await?;
    assert_eq!(client.doctor().await?["os"], Platform::current().as_str());
    client.call("worker.shutdown", json!({})).await?;
    registry.close().await;
    Ok(())
}

#[tokio::test]
async fn same_paths_in_two_workers_are_registered_independently_and_configuration_is_pinned()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("state"))?;
    let coordinator = Coordinator::new(store.clone());
    let sample = temp.path().join("sample");
    std::fs::write(&sample, b"fixture bytes; never executed")?;
    let config = AppConfig {
        default_environment: Some("first".into()),
        environments: BTreeMap::from([
            ("first".into(), environment(&temp.path().join("first"))),
            ("second".into(), environment(&temp.path().join("second"))),
        ]),
        ..Default::default()
    };
    let run = coordinator.create_run(
        &sample,
        "inspect",
        Budget::default(),
        serde_json::to_value(&config)?,
    )?;
    let first = coordinator.create_child(&run.root_id, child(None))?.agent;
    let second = coordinator
        .create_child(&run.root_id, child(Some("second")))?
        .agent;
    let tools = ToolService::new(config.clone(), store.clone()).await?;
    for (agent, body) in [(&first, "one"), (&second, "two")] {
        let result = tools
            .execute(
                &agent.id,
                &format!("put-{}", agent.id),
                "file_put",
                json!({"path":"same.txt", "text":body}),
            )
            .await?;
        assert!(!result.is_error, "{}", result.value);
    }
    let files = store.records::<WorkerFileRecord>(&run.id)?;
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, files[1].path);
    assert_ne!(files[0].worker_id, files[1].worker_id);
    assert_ne!(files[0].sha256, files[1].sha256);
    assert_eq!(
        std::fs::read(temp.path().join("first/files/same.txt"))?,
        b"one"
    );
    assert_eq!(
        std::fs::read(temp.path().join("second/files/same.txt"))?,
        b"two"
    );
    let original = files
        .iter()
        .find(|file| file.agent_id == first.id)
        .unwrap()
        .evidence_id
        .as_ref()
        .unwrap();
    let staged = tools
        .execute(
            &second.id,
            "cross-environment-artifact",
            "artifact_stage",
            json!({"artifact_id":original}),
        )
        .await?;
    assert!(!staged.is_error, "{}", staged.value);
    let staged_path = staged.value["result"]["file"]["path"].as_str().unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("second").join(staged_path))?,
        b"one"
    );
    tools.close().await;
    let registry = EnvironmentRegistry::new(config.environments.clone())?;
    for id in ["first", "second"] {
        registry
            .client(id)
            .await?
            .call("worker.shutdown", json!({}))
            .await?;
    }
    registry.close().await;
    let mut changed = config;
    changed.environments.get_mut("second").unwrap().name = "changed".into();
    let tools = ToolService::new(changed, store).await?;
    let rejected = tools
        .execute(&second.id, "changed-config", "runtime_doctor", json!({}))
        .await?;
    assert!(rejected.is_error);
    assert!(rejected.value.to_string().contains("configuration changed"));
    tools.close().await;
    Ok(())
}

#[tokio::test]
async fn large_artifacts_round_trip_through_bounded_worker_frames() -> Result<()> {
    use std::io::Write;
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("state"))?;
    let coordinator = Coordinator::new(store.clone());
    let source = temp.path().join("large.dmp");
    let mut file = std::fs::File::create(&source)?;
    let chunk = vec![0x5a; 1024 * 1024];
    for _ in 0..33 {
        file.write_all(&chunk)?;
    }
    file.sync_all()?;
    let config = AppConfig {
        default_environment: Some("fixture".into()),
        environments: BTreeMap::from([(
            "fixture".into(),
            environment(&temp.path().join("worker")),
        )]),
        ..Default::default()
    };
    let run = coordinator.create_run(
        &source,
        "transfer only; never execute",
        Budget::default(),
        serde_json::to_value(&config)?,
    )?;
    let owner = coordinator.create_child(&run.root_id, child(None))?.agent;
    let original = store.artifact_file(
        &run.id,
        &owner.id,
        "input",
        "fixture",
        &source,
        "application/octet-stream",
        json!({}),
    )?;
    let tools = ToolService::new(config.clone(), store.clone()).await?;
    let staged = tools
        .execute(
            &owner.id,
            "large-stage",
            "artifact_stage",
            json!({"artifact_id":original.id}),
        )
        .await?;
    assert!(!staged.is_error, "{}", staged.value);
    let path = staged.value["result"]["file"]["path"].as_str().unwrap();
    let fetched = tools
        .execute(
            &owner.id,
            "large-fetch",
            "file_get",
            json!({"path":path,"max_bytes":34*1024*1024}),
        )
        .await?;
    assert!(!fetched.is_error, "{}", fetched.value);
    assert!(fetched.value.to_string().len() < 4096);
    let id = fetched.value["result"]["content_evidence_id"]
        .as_str()
        .unwrap();
    let returned: wukong::domain::Evidence = store.load(id)?;
    assert_eq!(returned.sha256, original.sha256);
    assert_eq!(returned.size, original.size);
    tools.close().await;
    let registry = EnvironmentRegistry::new(config.environments)?;
    registry
        .client("fixture")
        .await?
        .call("worker.shutdown", json!({}))
        .await?;
    registry.close().await;
    Ok(())
}

#[tokio::test]
async fn colliding_worker_job_ids_cannot_overwrite_or_cross_environment_boundaries() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let bridge = temp.path().join("collision-fixture.py");
    std::fs::write(
        &bridge,
        r#"import json,platform,sys
for line in sys.stdin:
    request=json.loads(line)
    method=request['method']
    if method=='ping': result={'identity':{'protocol':3,'installation_id':__import__('hashlib').sha256(str(sys.argv).encode()).hexdigest(),'boot_id':'fixture-boot','package_sha256':'0'*64}}
    elif method=='doctor': result={'os':platform.system().lower(),'capabilities':{'process_jobs':True},'methods':{name:{'ready':True} for name in ('job.start','job.status','job.artifacts')}}
    elif method=='job.start': result={'job_id':'same-local-id','state':'running'}
    elif method=='job.status': result={'job_id':'same-local-id','state':'completed','result':{'exit_code':0}}
    elif method=='job.artifacts': result=[]
    else: result={}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
"#,
    )?;
    let mut first_environment = environment(&temp.path().join("first"));
    first_environment
        .runtime
        .trusted_local_mut()
        .unwrap()
        .bridge_path = bridge.clone();
    let mut second_environment = environment(&temp.path().join("second"));
    second_environment
        .runtime
        .trusted_local_mut()
        .unwrap()
        .bridge_path = bridge;
    let config = AppConfig {
        default_environment: Some("first".into()),
        environments: BTreeMap::from([
            ("first".into(), first_environment),
            ("second".into(), second_environment),
        ]),
        ..Default::default()
    };
    let store = Store::open(temp.path().join("state"))?;
    let coordinator = Coordinator::new(store.clone());
    let target = temp.path().join("sample");
    std::fs::write(&target, b"fixture; never executed")?;
    let run = coordinator.create_run(
        &target,
        "check resource isolation",
        Budget::default(),
        serde_json::to_value(&config)?,
    )?;
    let first = coordinator.create_child(&run.root_id, child(None))?.agent;
    let second = coordinator
        .create_child(&run.root_id, child(Some("second")))?
        .agent;
    let tools = ToolService::new(config, store.clone()).await?;
    let mut ids = vec![];
    for agent in [&first, &second] {
        let started = tools
            .execute(
                &agent.id,
                &format!("start-{}", agent.id),
                "job_start",
                json!({"argv":["fixture-command-never-executed"]}),
            )
            .await?;
        assert!(!started.is_error, "{}", started.value);
        ids.push(
            started.value["result"]["job_id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert_ne!(ids[0], ids[1]);
    let receipts: Vec<serde_json::Value> = store.list("worker_receipt", &run.id)?;
    assert_eq!(receipts.len(), 2);
    assert!(
        receipts
            .iter()
            .all(|receipt| receipt["result"]["job_id"] == "same-local-id"
                && receipt["result"].get("worker_job_id").is_none())
    );
    let jobs = store.records::<wukong::domain::JobRecord>(&run.id)?;
    assert_eq!(jobs.len(), 2);
    assert!(jobs.iter().all(
        |job| job.worker_resource_id == "same-local-id" && job.status.job_id == "same-local-id"
    ));
    for (agent, id) in [(&first, &ids[0]), (&second, &ids[1])] {
        let status = tools
            .execute(
                &agent.id,
                &format!("status-{}", agent.id),
                "job_status",
                json!({"job_id":id}),
            )
            .await?;
        assert!(!status.is_error, "{}", status.value);
    }
    let rejected = tools
        .execute(
            &second.id,
            "cross-environment-job",
            "job_status",
            json!({"job_id":ids[0]}),
        )
        .await?;
    assert!(rejected.is_error);
    assert!(rejected.value.to_string().contains("different runtime"));
    tools.close().await;
    Ok(())
}
