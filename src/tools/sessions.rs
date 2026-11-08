//! Sessions tool operations.
use super::*;

impl ToolService {
    pub(super) fn authorize_session(
        &self,
        agent: &AgentRecord,
        session: &Value,
        closing: bool,
    ) -> Result<()> {
        ensure!(
            session["worker_id"] == self.worker_identity()?,
            "session belongs to another runtime"
        );
        if session["agent_id"] != agent.id {
            let owner: AgentRecord =
                self.require_in_run("agent", text(session, "agent_id")?, &agent.run_id)?;
            ensure!(
                closing && owner.status.terminal(),
                "session is owned by another active agent"
            );
        }
        Ok(())
    }

    pub(super) async fn session_tool(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        args: Value,
    ) -> Result<Value> {
        match name {
            "debug_command" | "debug_close" => {
                let session_id = text(&args, "session_id")?.to_owned();
                let mut session: Value =
                    self.require_in_run("debug_session", &session_id, &agent.run_id)?;
                self.authorize_session(agent, &session, name == "debug_close")?;
                let current = session["state"].as_str().unwrap_or_default().to_owned();
                // Idempotent close, mirroring interactive_close: a closed/lost
                // debug session closes again successfully.
                if name == "debug_close" && matches!(current.as_str(), "closed" | "lost") {
                    session["updated_at"] = json!(now());
                    self.store
                        .put("debug_session", &session_id, &agent.run_id, &session)?;
                    return Ok(json!({"session_id":session_id,"state":current,"reconciled":true}));
                }
                ensure!(
                    session["state"] == "running",
                    "debug session is closed or lost"
                );
                let raw = match self
                    .worker_call(
                        agent,
                        call_id,
                        if name == "debug_close" {
                            "debug.close"
                        } else {
                            "debug.command"
                        },
                        args,
                    )
                    .await
                {
                    Ok(raw) => raw,
                    Err(error) => {
                        // Worker lost the session (daemon restart): reconcile
                        // the local record so agent_finish is not blocked.
                        if name == "debug_close" && error.to_string().contains("absent or lost") {
                            session["state"] = json!("lost");
                            session["updated_at"] = json!(now());
                            self.store.put(
                                "debug_session",
                                &session_id,
                                &agent.run_id,
                                &session,
                            )?;
                            return Ok(
                                json!({"session_id":session_id,"state":"lost","reconciled":true}),
                            );
                        }
                        return Err(error);
                    }
                };
                let transcript = raw["transcript"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("debug/{session_id}/transcript.mi"));
                let transcript = self
                    .runtime()?
                    .call(
                        "file.get",
                        json!({"path":transcript,"max_bytes":8*1024*1024}),
                    )
                    .await?;
                let evidence = self.import_file(agent, call_id, &transcript)?;
                session["transcript_evidence_id"] = json!(evidence.id);
                if name == "debug_close" {
                    session["state"] = json!("closed");
                }
                session["updated_at"] = json!(now());
                self.store
                    .put("debug_session", &session_id, &agent.run_id, &session)?;
                Ok(json!({"response":raw,"transcript_evidence_id":evidence.id}))
            }
            "debug_start" => {
                self.known_file(agent, text(&args, "target")?, true)?;
                let raw = self
                    .worker_call(agent, call_id, "debug.start", args)
                    .await?;
                let session_id = raw["session_id"]
                    .as_str()
                    .context("worker debug session ID missing")?;
                let record = json!({"id":session_id,"run_id":agent.run_id,"agent_id":agent.id,"worker_id":self.worker_identity()?,"state":raw["state"],"worker":raw,"created_at":now(),"updated_at":now()});
                self.store
                    .put("debug_session", session_id, &agent.run_id, &record)?;
                Ok(raw)
            }
            "interactive_start" => {
                ensure!(
                    !string_list(&args, "argv")?.is_empty(),
                    "interactive argv cannot be empty"
                );
                let raw = self
                    .worker_call(agent, call_id, "interact.start", args)
                    .await?;
                let session_id = raw["session_id"]
                    .as_str()
                    .context("worker interactive session ID missing")?;
                let record = json!({"id":session_id,"run_id":agent.run_id,"agent_id":agent.id,"worker_id":self.worker_identity()?,"state":"running","created_at":now(),"updated_at":now()});
                self.store
                    .put("interact_session", session_id, &agent.run_id, &record)?;
                Ok(raw)
            }
            "interactive_send" | "interactive_close" => {
                let session_id = text(&args, "session_id")?.to_owned();
                let mut session: Value =
                    self.require_in_run("interact_session", &session_id, &agent.run_id)?;
                self.authorize_session(agent, &session, name == "interactive_close")?;
                let current = session["state"].as_str().unwrap_or_default().to_owned();
                // Closing is idempotent: an already closed/lost session closes
                // again successfully instead of stranding the caller.
                if name == "interactive_close" && matches!(current.as_str(), "closed" | "lost") {
                    session["updated_at"] = json!(now());
                    self.store
                        .put("interact_session", &session_id, &agent.run_id, &session)?;
                    return Ok(json!({"session_id":session_id,"state":current,"reconciled":true}));
                }
                ensure!(
                    session["state"] == "running"
                        || (name == "interactive_close" && session["state"] == "exited"),
                    "interactive session is closed or lost"
                );
                let raw = match self
                    .worker_call(
                        agent,
                        call_id,
                        if name == "interactive_close" {
                            "interact.close"
                        } else {
                            "interact.write"
                        },
                        args,
                    )
                    .await
                {
                    Ok(raw) => raw,
                    Err(error) => {
                        // The worker no longer knows the session (daemon
                        // restart): the local record must not stay "running"
                        // and block agent_finish forever.
                        let lost = error
                            .chain()
                            .any(|cause| cause.to_string().contains("absent or lost"));
                        if name == "interactive_close" && lost {
                            json!({"session_id":session_id,"state":"lost","reconciled":true})
                        } else {
                            return Err(error);
                        }
                    }
                };
                if name == "interactive_close" {
                    session["state"] = json!(if raw["state"].as_str() == Some("lost") {
                        "lost"
                    } else {
                        "closed"
                    });
                } else if raw["state"].as_str() == Some("exited") {
                    session["state"] = json!("exited");
                }
                session["updated_at"] = json!(now());
                self.store
                    .put("interact_session", &session_id, &agent.run_id, &session)?;
                Ok(raw)
            }
            _ => bail!("unknown session tool: {name}"),
        }
    }
}
