//! Execution tool operations.
use super::*;

impl ToolService {
    pub(crate) fn worker_identity(&self) -> Result<String> {
        Ok(stable(
            "worker",
            &serde_json::to_string(&self.config.runtime)?,
        ))
    }

    pub(super) fn request_identity(&self, agent: &AgentRecord, call_id: &str) -> String {
        stable(
            "request",
            &format!("{}:{}:{call_id}", agent.run_id, agent.id),
        )
    }

    pub(super) async fn worker_call(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        method: &str,
        mut params: Value,
    ) -> Result<Value> {
        let request_id = self.request_identity(agent, call_id);
        params["request_id"] = json!(request_id);
        params["call_id"] = json!(request_id);
        self.runtime()?.call(method,params).await.with_context(||format!("worker dispatch {method}; reconcile stable request {request_id} before changing its arguments"))
    }

    pub(super) async fn runtime_tool(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        mut args: Value,
    ) -> Result<Value> {
        match name {
            "job_status" | "fuzz_status" => {
                self.observe_job(agent, call_id, text(&args, "job_id")?)
                    .await
            }
            "job_cancel" => {
                let job_id = text(&args, "job_id")?;
                self.known_job(agent, job_id, true)?;
                self.worker_call(agent, call_id, "job.cancel", args).await
            }
            "job_artifacts" => {
                self.import_job_artifacts(agent, call_id, text(&args, "job_id")?)
                    .await
            }
            "debug_command" | "debug_close" | "debug_start" | "interactive_start"
            | "interactive_send" | "interactive_close" => {
                self.session_tool(agent, call_id, name, args).await
            }
            "exec_command" => {
                let _ = text(&args, "command")?;
                self.worker_call(agent, call_id, "exec.run", args).await
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
                    let finding: Finding = self.require_in_run(
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
                        .list::<Value>("job", &agent.run_id)?
                        .into_iter()
                        .filter(|job| {
                            job["tool_name"].as_str() == Some("poc_verify")
                                && job["agent_id"].as_str() == Some(agent.id.as_str())
                                && job["finding_id"].as_str() == Some(finding.id.as_str())
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
                    self.known_file(agent, text(&args, "target")?, true)?;
                    if !matches!(name, "fuzz_start" | "angr_solve") {
                        self.known_file(agent, text(&args, "input")?, false)?;
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
                            .list::<Value>("worker_file", &agent.run_id)?
                            .iter()
                            .any(|file| file["worker_id"]
                                == self.worker_identity().unwrap_or_default()
                                && file["path"]
                                    .as_str()
                                    .is_some_and(|path| path.starts_with(&prefix))),
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
                    .worker_call(agent, call_id, method, params.clone())
                    .await?;
                let job_id = raw["job_id"].as_str().context("worker job ID missing")?;
                let record = json!({"id":job_id,"run_id":agent.run_id,"agent_id":agent.id,"worker_id":self.worker_identity()?,"tool_name":name,"finding_id":finding_id,"call_id":call_id,"worker_request_id":self.request_identity(agent,call_id),"parameters":params,"target_sha256":run.target_sha256,"status":raw,"created_at":now(),"updated_at":now()});
                if let Some(previous) = self.store.get::<Value>("job", job_id)? {
                    ensure!(
                        previous["run_id"] == agent.run_id
                            && previous["agent_id"] == agent.id
                            && previous["call_id"] == call_id,
                        "worker job identity is already bound to another request"
                    );
                }
                self.store.put("job", job_id, &agent.run_id, &record)?;
                Ok(raw)
            }
            _ => bail!("unsupported runtime tool {name}"),
        }
    }
}
