use super::*;

async fn fixture() -> Result<(tempfile::TempDir, Store, ToolService, AgentMetadata, String)> {
    let dir = tempfile::tempdir()?;
    let bridge = dir.path().join("fixture.py");
    fs::write(
        &bridge,
        r#"import sys,json,pathlib,hashlib,base64,platform
root=pathlib.Path(sys.argv[sys.argv.index('--state')+1]);root.mkdir(exist_ok=True)
identity={'protocol':3,'installation_id':'evidence-fixture','boot_id':'boot','package_sha256':'0'*64}
files=[{'path':p,'sha256':hashlib.sha256(p.encode()).hexdigest(),'size':len(p)} for p in ['first','second']]
for line in sys.stdin:
 r=json.loads(line);method=r['method'];params=r.get('params',{})
 if method=='ping': value={'identity':identity}
 elif method=='doctor': value={'os':platform.system().lower(),'identity':identity}
 elif method=='job.status': value={'job_id':'worker-job','state':'completed','stdout':'finished'}
 elif method=='job.artifacts': value=files
 elif method=='file.get':
  path=params['path']; counter=root/(path+'-calls');n=int(counter.read_text()) if counter.exists() else 0;counter.write_text(str(n+1))
  if path=='second' and not (root/'resume').exists():
   print(json.dumps({'jsonrpc':'2.0','id':r['id'],'error':{'code':-32000,'message':'fixture temporary read failure','data':{'kind':'invalid_request'}}}),flush=True);continue
  value=next(f.copy() for f in files if f['path']==path);value['data_base64']=base64.b64encode(path.encode()).decode()
  if (root/'corrupt').exists(): value['size']+=1
 else: value={}
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':value}),flush=True)
"#,
    )?;
    let mut config = AppConfig {
        default_environment: Some("fixture".into()),
        ..Default::default()
    };
    config.environments.insert(
        "fixture".into(),
        crate::adapters::environments::EnvironmentConfig {
            name: "Fixture".into(),
            platform: crate::adapters::environments::Platform::current(),
            installation_id: None,
            runtime: crate::adapters::worker::RuntimeConfig {
                transport: crate::adapters::worker::TransportConfig::TrustedLocal(
                    crate::adapters::worker::TrustedLocalConfig {
                        state_dir: dir.path().join("worker"),
                        bridge_path: bridge,
                        ..Default::default()
                    },
                ),
                ..Default::default()
            },
        },
    );
    let store = Store::open(dir.path().join("state"))?;
    let target = dir.path().join("target");
    fs::write(&target, b"inert fixture")?;
    let coordinator = Coordinator::new(store.clone());
    let run = coordinator.create_run(
        &target,
        "evidence fixture",
        Budget::default(),
        serde_json::to_value(&config)?,
    )?;
    let owner = coordinator
        .create_child(
            &run.root_id,
            serde_json::from_value(
                json!({"name":"worker","role":"analyst","task":"fixture","scope":"fixture"}),
            )?,
        )?
        .agent
        .metadata;
    let tools = ToolService::new(config, store.clone()).await?;
    let context = tools.bound_to(&owner).await?;
    let id = "fixture-job".to_owned();
    let job: JobRecord = serde_json::from_value(
        json!({"id":id,"worker_resource_id":"worker-job","run_id":run.id,
        "agent_id":owner.id,"worker_id":context.worker_identity()?,"owner_generation":owner.generation,"tool_name":"job_start",
        "finding_id":null,"call_id":"start","worker_request_id":"start","parameters":{},"target_sha256":run.target_sha256,
        "status":{"job_id":"worker-job","state":"running"},"status_evidence_id":null,"imported_artifacts":null,
        "notification_message_id":null,"created_at":now(),"updated_at":now()}),
    )?;
    store.save_record(&job)?;
    Ok((dir, store, tools, owner, id))
}

#[tokio::test]
async fn transfer_failure_resumes_files_and_notifications_survive_reconstruction() -> Result<()> {
    let (dir, store, tools, owner, id) = fixture().await?;
    let context = tools.bound_to(&owner).await?;
    let status = context.jobs().observe(&owner, "observe", &id).await?;
    assert_eq!(status["evidence_sync"]["state"], "pending");
    assert!(
        !dir.path().join("worker/first-calls").exists(),
        "status must never download files"
    );
    assert_eq!(store.inbox(&owner.id)?.len(), 1);
    let partial = context
        .jobs()
        .import_job_artifacts(&owner, "sync", &id)
        .await?;
    assert_eq!(partial["evidence_sync"]["state"], "retry");
    assert_eq!(partial["evidence_sync"]["completed"], 1);
    assert!(
        store
            .load::<JobRecord>(&id)?
            .notification_message_id
            .is_none()
    );
    fs::write(dir.path().join("worker/resume"), b"ready")?;
    store.write(|tx| {
        let mut job: JobRecord = tx.get_record(&id)?.unwrap();
        job.evidence_sync.retry_at = None;
        tx.save_record(&job)
    })?;
    let reopened = Store::open(store.root())?;
    let resumed = tools.with_store(reopened.clone());
    let context = resumed.bound_to(&owner).await?;
    let (first, second) = tokio::join!(
        context.jobs().import_job_artifacts(&owner, "resume", &id),
        context.jobs().import_job_artifacts(&owner, "manual", &id)
    );
    assert_eq!(first?, second?);
    assert_eq!(
        fs::read_to_string(dir.path().join("worker/first-calls"))?,
        "1"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("worker/second-calls"))?,
        "2"
    );
    assert_eq!(reopened.inbox(&owner.id)?.len(), 2);
    // Simulate a crash after the notification transaction: replay cannot duplicate it.
    context.jobs().observe(&owner, "replay", &id).await?;
    context
        .jobs()
        .import_job_artifacts(&owner, "replay", &id)
        .await?;
    assert_eq!(reopened.inbox(&owner.id)?.len(), 2);
    tools.close().await;
    Ok(())
}

#[tokio::test]
async fn integrity_failure_is_durable_and_never_delivers_completion() -> Result<()> {
    let (dir, store, tools, owner, id) = fixture().await?;
    let context = tools.bound_to(&owner).await?;
    context.jobs().observe(&owner, "observe", &id).await?;
    fs::write(dir.path().join("worker/corrupt"), b"yes")?;
    let result = context
        .jobs()
        .import_job_artifacts(&owner, "sync", &id)
        .await?;
    assert_eq!(result["evidence_sync"]["state"], "failed");
    context
        .jobs()
        .import_job_artifacts(&owner, "retry", &id)
        .await?;
    assert_eq!(
        fs::read_to_string(dir.path().join("worker/first-calls"))?,
        "1"
    );
    let job = store.load::<JobRecord>(&id)?;
    assert!(!job.evidence_ready());
    assert!(job.notification_message_id.is_none());
    assert!(job.evidence_sync.files[0].error.is_some());
    tools.close().await;
    Ok(())
}

#[tokio::test]
async fn stale_generation_and_legacy_final_notice_are_not_reactivated() -> Result<()> {
    let (dir, store, tools, owner, id) = fixture().await?;
    fs::write(dir.path().join("worker/resume"), b"ready")?;
    let mut newer = owner.clone();
    newer.generation += 1;
    store.write(|tx| tx.save_record(&newer))?;
    let context = tools.bound_to(&newer).await?;
    context.jobs().observe(&newer, "observe", &id).await?;
    context
        .jobs()
        .import_job_artifacts(&newer, "sync", &id)
        .await?;
    assert!(store.inbox(&owner.id)?.is_empty());
    let messages = store.read(|tx| tx.messages(&owner.run_id))?;
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.sender == "worker" && m.envelope.delivery == MessageDelivery::Stale)
            .count(),
        2
    );
    store.write(|tx| {
        let mut job: JobRecord = tx.get_record(&id)?.unwrap();
        job.execution_notification_message_id = None;
        job.execution_status_evidence_id = None;
        job.evidence_sync = Default::default();
        tx.save_record(&job)
    })?;
    context.jobs().observe(&newer, "legacy", &id).await?;
    assert!(
        store
            .load::<JobRecord>(&id)?
            .execution_notification_message_id
            .is_none()
    );
    assert!(store.load::<JobRecord>(&id)?.evidence_ready());
    tools.close().await;
    Ok(())
}

#[test]
fn manifest_and_bytes_require_full_content_identity() -> Result<()> {
    let empty =
        json!({"path":"empty","sha256":binary::sha256_bytes(b""),"size":0,"data_base64":""});
    validate_file(&manifest("worker", &json!([empty.clone()]))?[0], &empty)?;
    let raw = json!([{"path":"a","sha256":binary::sha256_bytes(b"a"),"size":1}]);
    let files = manifest("worker", &raw)?;
    let mut file =
        json!({"path":"a","sha256":files[0].sha256,"size":1,"data_base64":BASE64.encode(b"a")});
    validate_file(&files[0], &file)?;
    file["data_base64"] = json!(BASE64.encode(b"b"));
    assert!(validate_file(&files[0], &file).is_err());
    assert!(manifest("worker", &json!([raw[0], raw[0]])).is_err());
    Ok(())
}

#[tokio::test]
async fn root_success_requires_ready_evidence_but_explicit_uncertainty_can_finish() -> Result<()> {
    let (_dir, store, tools, owner, id) = fixture().await?;
    let context = tools.bound_to(&owner).await?;
    context.jobs().observe(&owner, "observe", &id).await?;
    let coordinator = Coordinator::new(store.clone());
    let mail = store.inbox(&owner.id)?;
    store.checkpoint_and_ack_generation(
        &owner.id,
        json!({}),
        &mail.iter().map(|m| m.id).collect::<Vec<_>>(),
        Some(owner.generation),
    )?;
    coordinator.finish(&owner.id, "fixture execution observed")?;
    let run: RunRecord = store.load(&owner.run_id)?;
    let root: AgentMetadata = store.load(&run.root_id)?;
    let mail = store.inbox(&root.id)?;
    store.checkpoint_and_ack_generation(
        &root.id,
        json!({}),
        &mail.iter().map(|m| m.id).collect::<Vec<_>>(),
        Some(root.generation),
    )?;
    assert!(
        coordinator
            .finish(&root.id, "success")
            .unwrap_err()
            .to_string()
            .contains("evidence is not ready")
    );
    coordinator.finish_result(
        &root.id,
        TaskResult {
            answer: "Execution ended; artifact transfer is unresolved.".into(),
            outcome: TaskOutcome::Inconclusive,
            unknowns: vec!["Worker evidence has not synchronized.".into()],
            ..Default::default()
        },
    )?;
    tools.close().await;
    Ok(())
}

#[tokio::test]
async fn synchronization_slots_are_global_and_per_environment() -> Result<()> {
    let limits = SyncLimits::default();
    let first = limits.acquire("a").await?;
    let second = limits.acquire("b").await?;
    assert_eq!(limits.global.available_permits(), 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), limits.acquire("c"))
            .await
            .is_err()
    );
    drop(second);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), limits.acquire("a"))
            .await
            .is_err()
    );
    let other = limits.acquire("c").await?;
    drop(other);
    drop(first);
    assert_eq!(limits.global.available_permits(), 2);
    Ok(())
}
