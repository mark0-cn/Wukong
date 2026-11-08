//! Worker execution use cases.
use super::*;

#[derive(Clone)]
pub(super) struct ExecutionService {
    pub(super) store: Store,
    pub(super) gateway: WorkerGateway,
    pub(super) files: FilesService,
    pub(super) jobs: jobs::JobService,
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
                self.gateway
                    .worker_call(agent, call_id, "exec.run", args)
                    .await
            }
            "job_start" | "fuzz_start" | "crash_replay" | "input_minimize" | "poc_verify"
            | "angr_solve" => {
                let run: RunRecord = self.store.require("run", &agent.run_id)?;
                if name == "angr_solve" {
                    ensure!(
                        !address_list(&args, "find")?.is_empty(),
                        "angr_solve requires at least one find address"
                    );
                }
                let finding_id = args["finding_id"].as_str().map(str::to_owned);
                if name == "poc_verify" {
                    let finding: Finding = require_in_run(
                        &self.store,
                        "finding",
                        finding_id.as_deref().context("finding_id missing")?,
                        &agent.run_id,
                    )?;
                    ensure!(
                        finding.agent_id != agent.id,
                        "the verifier must differ from the finding producer"
                    );
                    // Structural closure budget: at most two replay jobs per
                    // (verifier, finding). The verdict then belongs in
                    // verification_record, passed=true or a diagnosed false.
                    let used = self
                        .store
                        .list::<JobRecord>("job", &agent.run_id)?
                        .into_iter()
                        .filter(|job| {
                            job.tool_name == "poc_verify"
                                && job.agent_id == agent.id
                                && job.call_id != call_id
                                && job.finding_id.as_deref() == Some(finding.id.as_str())
                        })
                        .count();
                    ensure!(
                        used < 2,
                        "poc_verify budget for this finding is exhausted ({used}/2 jobs used); record your verdict with verification_record now — passed=false with a diagnosis is a complete outcome"
                    );
                    args.as_object_mut()
                        .context("arguments not an object")?
                        .remove("finding_id");
                    args["producer_agent_id"] = json!(finding.agent_id);
                    args["verifier_agent_id"] = json!(agent.id);
                }
                if name != "job_start" {
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
                            .list::<WorkerFileRecord>("worker_file", &agent.run_id)?
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
                    if let Some(previous) = tx.get::<JobRecord>("job", job_id)? {
                        ensure!(
                            previous.run_id == agent.run_id
                                && previous.agent_id == agent.id
                                && previous.call_id == call_id,
                            "worker job identity is already bound to another request"
                        );
                        return Ok(());
                    }
                    tx.put("job", job_id, &agent.run_id, &record)
                })?;
                Ok(raw)
            }
            _ => bail!("unsupported runtime tool {name}"),
        }
    }
}
