//! Worker execution use cases.
use super::*;

#[derive(Clone)]
pub(super) struct ExecutionService {
    pub(super) store: Store,
    pub(super) gateway: WorkerGateway,
    pub(super) files: FilesService,
    pub(super) jobs: jobs::JobService,
    pub(super) verification: verification::VerificationService,
    pub(super) sessions: SessionsService,
}

struct CheckedLaunchSpec {
    source_target_sha256: String,
}
impl ExecutionService {
    pub(super) fn new(
        store: Store,
        gateway: WorkerGateway,
        locks: ResourceLocks,
        sync_limits: evidence_sync::SyncLimits,
        telemetry: crate::adapters::telemetry::TelemetryHandle,
    ) -> Self {
        let files = FilesService {
            store: store.clone(),
            gateway: gateway.clone(),
        };
        let jobs = jobs::JobService {
            store: store.clone(),
            gateway: gateway.clone(),
            files: files.clone(),
            locks: locks.clone(),
            sync_limits,
            telemetry,
        };
        let sessions = SessionsService {
            store: store.clone(),
            gateway: gateway.clone(),
            files: files.clone(),
            locks,
        };
        Self {
            store,
            gateway,
            files,
            verification: verification::VerificationService { jobs: jobs.clone() },
            jobs,
            sessions,
        }
    }
    pub(super) async fn runtime_tool(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        name: &str,
        mut args: Value,
    ) -> Result<Value> {
        if name == "debug_inspect" && args["kind"] == "events" {
            return self.sessions.events(agent, call_id, &args).await;
        }
        if name == "interactive_send" {
            let input: args::InteractiveSendArgs = serde_json::from_value(args)?;
            let bytes = match input.source {
                args::ByteSource::Text { text } => text.into_bytes(),
                args::ByteSource::Base64 { data_base64 } => BASE64.decode(data_base64)?,
                args::ByteSource::Evidence { evidence_id } => {
                    let record: Evidence =
                        require_in_run(&self.store, &evidence_id, &agent.run_id)?;
                    ensure!(record.size <= 65536, "interactive input exceeds 64 KiB");
                    self.store.read_evidence(&agent.run_id, &evidence_id)?
                }
            };
            ensure!(bytes.len() <= 65536, "interactive input exceeds 64 KiB");
            args = json!({"session_id":input.session_id,"data_base64":BASE64.encode(bytes)});
            if let Some(timeout) = input.read_timeout_secs {
                args["read_timeout_secs"] = json!(timeout);
            }
        }
        if name == "trace_run" {
            return self.trace_run(agent, call_id, args).await;
        }
        match name {
            "job_status" => {
                self.jobs
                    .observe(agent, call_id, text(&args, "job_id")?)
                    .await
            }
            "job_cancel" => {
                self.jobs
                    .cancel(agent, call_id, text(&args, "job_id")?)
                    .await
            }
            "job_artifacts" => {
                self.jobs
                    .import_job_artifacts(agent, call_id, text(&args, "job_id")?)
                    .await
            }
            "debug_command" | "debug_inspect" | "debug_control" | "debug_close" | "debug_start"
            | "interactive_start" | "interactive_send" | "interactive_close" => {
                self.sessions.session_tool(agent, call_id, name, args).await
            }
            "exec_command" => {
                let _ = text(&args, "command")?;
                if let Some(target) = args.get("target").and_then(Value::as_str) {
                    self.files.known_file(agent, target, true)?;
                }
                self.gateway
                    .worker_call(
                        agent,
                        call_id,
                        Request::ExecRun(serde_json::from_value(args)?),
                    )
                    .await
            }
            "job_start" | "fuzz_start" | "crash_replay" | "input_minimize" | "poc_verify"
            | "angr_solve" | "py_run" => {
                let run: RunRecord = self.store.load(&agent.run_id)?;
                let mut execution_target_sha256 = run.target_sha256.clone();
                if name == "job_start" {
                    args = substitute_target_argv(&args, &run.target_sha256);
                }
                if name == "angr_solve" {
                    ensure!(
                        !address_list(&args, "find")?.is_empty(),
                        "angr_solve requires at least one find address"
                    );
                }
                let finding_id = args["finding_id"].as_str().map(str::to_owned);
                if name == "poc_verify" {
                    self.verification
                        .admit(agent, call_id, &run, finding_id.as_deref(), &mut args)
                        .await?;
                }
                if name == "py_run" {
                    let source = args["source"].as_str().filter(|value| !value.is_empty());
                    let source_file = args["source_file"].as_str();
                    ensure!(
                        source.is_some() != source_file.is_some(),
                        "py_run requires exactly one of source or source_file"
                    );
                    if let Some(file) = source_file {
                        self.files.known_file(agent, file, false)?;
                    }
                    if let Some(target) = args.get("target").and_then(Value::as_str) {
                        self.files.known_file(agent, target, true)?;
                    }
                } else if name != "job_start" {
                    self.files.known_file(agent, text(&args, "target")?, true)?;
                    if !matches!(name, "fuzz_start" | "angr_solve") {
                        self.files.known_file(agent, text(&args, "input")?, false)?;
                    }
                } else {
                    for input in string_list(&args, "executable_inputs")? {
                        self.files.known_file(agent, &input, false)?;
                    }
                    if args["launch_spec"].is_string() {
                        execution_target_sha256 = self
                            .checked_launch_spec(agent, call_id, &args)
                            .await?
                            .source_target_sha256;
                    } else {
                        ensure!(
                            !string_list(&args, "argv")?.is_empty(),
                            "job argv cannot be empty"
                        );
                    }
                    if let Some(input) = args["input"].as_str() {
                        self.files.known_file(agent, input, false)?;
                    }
                }
                if name == "fuzz_start" {
                    let seeds = text(&args, "seeds")?;
                    let prefix = format!("{}/", seeds.trim_end_matches('/'));
                    ensure!(
                        self.store
                            .records::<WorkerFileRecord>(&agent.run_id)?
                            .iter()
                            .any(|file| file.worker_id
                                == self.gateway.worker_identity().unwrap_or_default()
                                && file.path.starts_with(&prefix)),
                        "seed directory must contain staged in-run files"
                    );
                }
                let method = match name {
                    "job_start" => Method::JobStart,
                    "fuzz_start" => Method::FuzzStart,
                    "crash_replay" => Method::CrashReplay,
                    "input_minimize" => Method::InputMinimize,
                    "angr_solve" => Method::AngrSolve,
                    "py_run" => Method::PythonRun,
                    _ => Method::PocVerify,
                };
                // job_start argv may reference the staged target via the
                // {target} placeholder; the harness resolves it to the worker
                // path so scripts receive it as a plain argv element (staged
                // in isolated jobs) instead of guessing the worker layout.
                let params = if name == "job_start" {
                    substitute_target_argv(&args, &run.target_sha256)
                } else {
                    args.clone()
                };
                let raw = self
                    .gateway
                    .worker_call(agent, call_id, (method).decode(params.clone())?)
                    .await?;
                let status: JobStatus = serde_json::from_value(raw.clone())?;
                let worker_id = self.gateway.worker_identity()?;
                let job_id = stable("job", &format!("{worker_id}:{}", status.job_id));
                let record = JobRecord {
                    id: job_id.clone(),
                    worker_resource_id: status.job_id.clone(),
                    run_id: agent.run_id.clone(),
                    agent_id: agent.id.clone(),
                    worker_id: self.gateway.worker_identity()?,
                    owner_generation: agent.generation,
                    tool_name: name.into(),
                    finding_id,
                    call_id: call_id.into(),
                    worker_request_id: self.gateway.request_identity(agent, call_id),
                    parameters: params,
                    target_sha256: execution_target_sha256,
                    status,
                    status_evidence_id: None,
                    imported_artifacts: None,
                    notification_message_id: None,
                    execution_notification_message_id: None,
                    execution_status_evidence_id: None,
                    evidence_sync: Default::default(),
                    cleanup_intent: None,
                    created_at: now(),
                    updated_at: now(),
                };
                self.store.write(|tx| {
                    if let Some(previous) = tx.get_record::<JobRecord>(&job_id)? {
                        ensure!(
                            previous.run_id == agent.run_id
                                && previous.agent_id == agent.id
                                && previous.call_id == call_id,
                            "worker job identity is already bound to another request"
                        );
                        return Ok(());
                    }
                    tx.save_record(&record)
                })?;
                self.jobs.present(&record, raw)
            }
            _ => bail!("unsupported runtime tool {name}"),
        }
    }

    async fn checked_launch_spec(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        args: &Value,
    ) -> Result<CheckedLaunchSpec> {
        let launch = text(args, "launch_spec")?;
        let manifest_file = self.files.known_file(agent, launch, false)?;
        ensure!(manifest_file.size <= 65536, "LaunchSpec exceeds 64 KiB");
        let imported = self
            .files
            .file_get(
                agent,
                &format!("{call_id}:launch-identity"),
                &json!({"path":launch,"max_bytes":65536}),
            )
            .await?;
        let bytes = self
            .store
            .read_evidence(&agent.run_id, text(&imported, "content_evidence_id")?)?;
        ensure!(
            hex::encode(Sha256::digest(&bytes)) == manifest_file.sha256,
            "LaunchSpec changed since registration"
        );
        let manifest: Value = serde_json::from_slice(&bytes)?;
        let target_name = text(&manifest, "target")?;
        let declared = text(&manifest, "base")?;
        let member = self
            .files
            .known_file(agent, &format!("{declared}/{target_name}"), false)?;
        ensure!(
            manifest["files"][target_name] == member.sha256,
            "LaunchSpec target identity mismatch"
        );
        ensure!(
            string_list(args, "argv")?.is_empty(),
            "LaunchSpec owns job argv"
        );
        ensure!(
            args.get("timeout_secs").is_none(),
            "LaunchSpec owns its execution timeout; omit timeout_secs"
        );
        ensure!(
            args.get("cwd").is_none() && args.get("env").is_none(),
            "LaunchSpec owns cwd and environment; omit those overrides"
        );
        Ok(CheckedLaunchSpec {
            source_target_sha256: member.sha256,
        })
    }

    async fn trace_run(&self, agent: &AgentMetadata, call_id: &str, args: Value) -> Result<Value> {
        let input: args::TraceStartArgs = serde_json::from_value(args)?;
        input.spec.validate()?;
        let params = serde_json::to_value(&input)?;
        let launch = self.checked_launch_spec(agent, call_id, &params).await?;
        for point in &input.spec.points {
            let module = match point.location() {
                wukong_worker_protocol::TraceLocation::Symbol { module_sha256, .. }
                | wukong_worker_protocol::TraceLocation::ModuleOffset { module_sha256, .. } => {
                    module_sha256
                }
            };
            ensure!(
                module == &launch.source_target_sha256,
                "trace point module does not match the LaunchSpec source target"
            );
        }
        let input_file = self.files.known_file(agent, &input.input, false)?;
        let input_evidence_id = input_file
            .evidence_id
            .as_deref()
            .context("trace input must be bound to same-run content evidence")?;
        let input_evidence: Evidence =
            require_in_run(&self.store, input_evidence_id, &agent.run_id)?;
        ensure!(
            input_evidence.sha256 == input_file.sha256 && input_evidence.size == input_file.size,
            "trace input evidence identity mismatch"
        );
        self.store
            .verified_evidence_path(&agent.run_id, input_evidence_id)?;

        let mut raw = self
            .gateway
            .worker_call(agent, call_id, Request::TraceStart(input.clone()))
            .await?;
        raw["trace_spec"] = json!({
            "schema_version":input.spec.schema_version,
            "abi":input.spec.abi,
            "plan_signature":input.spec.plan_signature(),
            "source_target_sha256":launch.source_target_sha256,
        });
        let status: JobStatus = serde_json::from_value(raw.clone())?;
        let worker_id = self.gateway.worker_identity()?;
        let job_id = stable("job", &format!("{worker_id}:{}", status.job_id));
        let record = JobRecord {
            id: job_id.clone(),
            worker_resource_id: status.job_id.clone(),
            run_id: agent.run_id.clone(),
            agent_id: agent.id.clone(),
            worker_id,
            owner_generation: agent.generation,
            tool_name: "trace_run".into(),
            finding_id: None,
            call_id: call_id.into(),
            worker_request_id: self.gateway.request_identity(agent, call_id),
            parameters: params,
            target_sha256: launch.source_target_sha256,
            status,
            status_evidence_id: None,
            imported_artifacts: None,
            notification_message_id: None,
            execution_notification_message_id: None,
            execution_status_evidence_id: None,
            evidence_sync: Default::default(),
            cleanup_intent: None,
            created_at: now(),
            updated_at: now(),
        };
        self.store.write(|tx| {
            if let Some(previous) = tx.get_record::<JobRecord>(&job_id)? {
                ensure!(
                    previous.run_id == agent.run_id
                        && previous.agent_id == agent.id
                        && previous.call_id == call_id,
                    "worker job identity is already bound to another request"
                );
                return Ok(());
            }
            tx.save_record(&record)
        })?;
        self.jobs.present(&record, raw)
    }
}
