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
impl ExecutionService {
    pub(super) fn new(store: Store, gateway: WorkerGateway, locks: ResourceLocks) -> Self {
        let files = FilesService {
            store: store.clone(),
            gateway: gateway.clone(),
        };
        let jobs = jobs::JobService {
            store: store.clone(),
            gateway: gateway.clone(),
            files: files.clone(),
            locks,
        };
        let sessions = SessionsService {
            store: store.clone(),
            gateway: gateway.clone(),
            files: files.clone(),
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
        match name {
            "job_status" | "fuzz_status" => {
                self.jobs
                    .observe(agent, call_id, text(&args, "job_id")?)
                    .await
            }
            "job_cancel" => {
                let job_id = text(&args, "job_id")?;
                self.jobs.known_job(agent, job_id, true)?;
                self.gateway
                    .worker_call(agent, call_id, "job.cancel", args)
                    .await
            }
            "job_artifacts" => {
                self.jobs
                    .import_job_artifacts(agent, call_id, text(&args, "job_id")?)
                    .await
            }
            "debug_command" | "debug_close" | "debug_start" | "interactive_start"
            | "interactive_send" | "interactive_close" => {
                self.sessions.session_tool(agent, call_id, name, args).await
            }
            "exec_command" => {
                let _ = text(&args, "command")?;
                if let Some(target) = args.get("target").and_then(Value::as_str) {
                    self.files.known_file(agent, target, true)?;
                }
                self.gateway
                    .worker_call(agent, call_id, "exec.run", args)
                    .await
            }
            "job_start" | "fuzz_start" | "crash_replay" | "input_minimize" | "poc_verify"
            | "angr_solve" | "py_run" => {
                let run: RunRecord = self.store.load(&agent.run_id)?;
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
                    let _ = text(&args, "source")?;
                    if let Some(target) = args.get("target").and_then(Value::as_str) {
                        self.files.known_file(agent, target, true)?;
                    }
                } else if name != "job_start" {
                    self.files.known_file(agent, text(&args, "target")?, true)?;
                    if !matches!(name, "fuzz_start" | "angr_solve") {
                        self.files.known_file(agent, text(&args, "input")?, false)?;
                    }
                } else {
                    ensure!(
                        !string_list(&args, "argv")?.is_empty(),
                        "job argv cannot be empty"
                    );
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
                    "job_start" => "job.start",
                    "fuzz_start" => "fuzz.start",
                    "crash_replay" => "crash.replay",
                    "input_minimize" => "input.minimize",
                    "angr_solve" => "angr.solve",
                    "py_run" => "py.run",
                    _ => "poc.verify",
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
                    .worker_call(agent, call_id, method, params.clone())
                    .await?;
                let job_id = raw["job_id"].as_str().context("worker job ID missing")?;
                let record = JobRecord {
                    id: job_id.into(),
                    run_id: agent.run_id.clone(),
                    agent_id: agent.id.clone(),
                    worker_id: self.gateway.worker_identity()?,
                    owner_generation: agent.generation,
                    tool_name: name.into(),
                    finding_id,
                    call_id: call_id.into(),
                    worker_request_id: self.gateway.request_identity(agent, call_id),
                    parameters: params,
                    target_sha256: run.target_sha256,
                    status: serde_json::from_value(raw.clone())?,
                    status_evidence_id: None,
                    imported_artifacts: None,
                    notification_message_id: None,
                    created_at: now(),
                    updated_at: now(),
                };
                self.store.write(|tx| {
                    if let Some(previous) = tx.get_record::<JobRecord>(job_id)? {
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
                Ok(raw)
            }
            _ => bail!("unsupported runtime tool {name}"),
        }
    }
}
