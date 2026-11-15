//! Real worker acceptance with trusted Python/Shell fixtures, never host target execution.
use super::*;

async fn wait_job(service: &ToolService, verifier: &str, job: &str) -> Result<Value> {
    for index in 0..150 {
        let result = service
            .execute(
                verifier,
                &format!("observe:{job}:{index}"),
                "job_status",
                json!({"job_id":job}),
            )
            .await?;
        assert!(!result.is_error, "{result:?}");
        let value = super::result(&result);
        if !["queued", "running"].contains(&value["state"].as_str().unwrap_or("")) {
            return Ok(value.clone());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("trusted acceptance job did not finish")
}

#[tokio::test]
#[ignore = "requires a dedicated real Linux Docker worker; WUKONG_TEST_DOCKER_CONTAINER"]
async fn docker_script_predicates_accept_python_shell_and_reject_banner_only() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut config = worker_config(temp.path());
    let environment = config.environments.get_mut("fixture").unwrap();
    environment.platform = wukong::environments::Platform::Linux;
    environment.runtime.transport =
        wukong::runtime::TransportConfig::Docker(wukong::runtime::DockerConfig {
            docker_executable: std::env::var("WUKONG_TEST_DOCKER_EXECUTABLE")
                .unwrap_or_else(|_| "docker".into()),
            remote_state_dir: format!("/state/validator-{}", uuid::Uuid::new_v4().simple()),
            ..wukong::runtime::DockerConfig::new(std::env::var("WUKONG_TEST_DOCKER_CONTAINER")?)
        });
    let (store, coordinator, run, producer) = fixture_with_config(temp.path(), &config)?;
    let verifier = child(
        &coordinator,
        &run.root_id,
        Role::Verifier,
        "independent predicates",
    )?;
    let service = ToolService::new(config, store.clone()).await?;
    for (name, shell, broken) in [
        ("python", false, false),
        ("shell", true, false),
        ("broken", false, true),
    ] {
        let generated = std::process::Command::new("python")
            .env("PYTHONPATH",Path::new(env!("CARGO_MANIFEST_DIR")).join("workers/tests"))
            .args(["-c", &format!("from validator_fixtures import candidate; print(candidate(broken={},shell={}),end='')",if broken {"True"} else {"False"},if shell {"True"} else {"False"})]).output()?;
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let submitted = service.execute(&producer.id,&format!("put:{name}"),"file_put",json!({"path":format!("{name}.txt"),"path_policy":"versioned","text":String::from_utf8(generated.stdout)?})).await?;
        assert!(!submitted.is_error, "{submitted:?}");
        let artifact_id = result(&submitted)["content_evidence_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let staged = service
            .execute(
                &verifier.id,
                &format!("stage:{name}"),
                "artifact_stage",
                json!({"artifact_id":artifact_id}),
            )
            .await?;
        assert!(!staged.is_error, "{staged:?}");
        let script_path = result(&staged)["file"]["path"].as_str().unwrap();
        let interpreter = if shell { "bash" } else { "python3" };
        let normal = service
            .execute(
                &verifier.id,
                &format!("normal:{name}"),
                "job_start",
                json!({"argv":[interpreter,script_path,"--verify"]}),
            )
            .await?;
        assert!(!normal.is_error, "{normal:?}");
        let job_id = result(&normal)["job_id"].as_str().unwrap().to_owned();
        let normal = wait_job(&service, &verifier.id, &job_id).await?;
        assert_eq!(normal["result"]["exit_code"], 0, "{normal}");
        let job: JobRecord = store.load(&job_id)?;
        let artifacts = job.imported_artifacts.as_ref().unwrap()["artifacts"]
            .as_array()
            .unwrap();
        let baseline = artifacts
            .iter()
            .find(|v| {
                v["artifact"]["path"]
                    .as_str()
                    .unwrap()
                    .ends_with("/baseline.json")
            })
            .unwrap()["evidence_id"]
            .as_str()
            .unwrap();
        let review = store.artifact(&run.id,&verifier.id,&format!("review:{name}"),"test.collector_review",b"Trusted fixture inspection: --verify and --validation-io call validation_status; the broken fixture deliberately ignores process state.","text/plain",json!({}))?;
        let integration = service.execute(&verifier.id,&format!("integration:{name}"),"analysis_verify",json!({"producer_id":producer.id,"claim":"shared evaluator","scope":"trusted fixture wiring","expected":"same evaluator in both modes","observed":"inspected fixture source","verdict":"supported","counterexamples":[],"evidence_ids":[review.id,artifact_id],"limitations":["Collector wiring review does not establish predicate correctness; artifact_validate tests it separately."],"checks":[{"kind":"validator_integration","claim":"shared evaluator","expected":"same evaluator","observed":"both entry points call validation_status","verdict":"supported","evidence_ids":[review.id],"artifact_evidence_id":artifact_id,"reproduction_job_id":job_id,"expected_exit_code":0}]})).await?;
        assert!(!integration.is_error, "{integration:?}");
        let args = json!({"artifact_evidence_id":artifact_id,"baseline_evidence_id":baseline,"baseline_job_id":job_id,"interpreter":interpreter,"arguments":["--verify"]});
        let checks = service
            .execute(
                &verifier.id,
                &format!("validate:{name}"),
                "artifact_validate",
                args.clone(),
            )
            .await?;
        assert!(!checks.is_error, "{checks:?}");
        let validation_id = result(&checks)["validation_id"].as_str().unwrap();
        wait_job(&service, &verifier.id, validation_id).await?;
        let validation: ArtifactValidation = store.load(validation_id)?;
        assert_eq!(validation.passed, !broken, "{validation:?}");
        assert!(validation.status_evidence_id.is_some());
        assert!(validation.result.as_ref().unwrap()["report_evidence_id"].is_string());
        let replay = service
            .execute(
                &verifier.id,
                &format!("validate:{name}"),
                "artifact_validate",
                args,
            )
            .await?;
        assert_eq!(result(&replay)["validation_id"], validation_id);
    }
    // Explicit permission survives placeholder resolution, without running the inert sample.
    let stage = service
        .execute(
            &verifier.id,
            "stage-fixture-target",
            "stage_target",
            json!({}),
        )
        .await?;
    assert!(!stage.is_error);
    let permission = service.execute(&verifier.id,"target-executable","job_start",json!({"argv":["python3","-c","import os,sys; assert os.access(sys.argv[1],os.X_OK)","{target}"],"executable_inputs":["{target}"]})).await?;
    assert!(!permission.is_error, "{permission:?}");
    let permission = wait_job(
        &service,
        &verifier.id,
        result(&permission)["job_id"].as_str().unwrap(),
    )
    .await?;
    assert_eq!(permission["result"]["exit_code"], 0);
    if let Ok(path) = std::env::var("WUKONG_ACCEPTANCE_OUTPUT") {
        wukong::report::export(&store, &run.id, Path::new(&path))?;
    }
    service.close().await;
    Ok(())
}
