//! Sessions tool operations.
use super::*;
use crate::runtime::protocol::{SessionHandle, SessionState, WorkerError, WorkerErrorKind};

fn session_lost(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<WorkerError>()
            .is_some_and(|error| error.kind == WorkerErrorKind::SessionLost)
    })
}

impl ToolService {
    pub(super) fn authorize_session(
        &self,
        agent: &AgentMetadata,
        session: &Value,
        closing: bool,
    ) -> Result<()> {
        ensure!(
            session["worker_id"] == self.worker_identity()?,
            "session belongs to another runtime"
        );
        if session["agent_id"] != agent.id {
            let owner: AgentMetadata =
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
        agent: &AgentMetadata,
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
                let current: SessionState = serde_json::from_value(session["state"].clone())?;
                // Idempotent close, mirroring interactive_close: a closed/lost
                // debug session closes again successfully.
                if name == "debug_close"
                    && matches!(current, SessionState::Closed | SessionState::Lost)
                {
                    session["updated_at"] = json!(now());
                    self.store
                        .put("debug_session", &session_id, &agent.run_id, &session)?;
                    return Ok(json!({"session_id":session_id,"state":current,"reconciled":true}));
                }
                ensure!(
                    current == SessionState::Running,
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
                        if name == "debug_close" && session_lost(&error) {
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
                let handle: SessionHandle = serde_json::from_value(raw.clone())?;
                let session_id = handle.session_id.as_str();
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
                let handle: SessionHandle = serde_json::from_value(raw.clone())?;
                let session_id = handle.session_id.as_str();
                let record = json!({"id":session_id,"run_id":agent.run_id,"agent_id":agent.id,"worker_id":self.worker_identity()?,"state":handle.state,"created_at":now(),"updated_at":now()});
                self.store
                    .put("interact_session", session_id, &agent.run_id, &record)?;
                Ok(raw)
            }
            "interactive_send" | "interactive_close" => {
                let session_id = text(&args, "session_id")?.to_owned();
                let mut session: Value =
                    self.require_in_run("interact_session", &session_id, &agent.run_id)?;
                self.authorize_session(agent, &session, name == "interactive_close")?;
                let current: SessionState = serde_json::from_value(session["state"].clone())?;
                // Closing is idempotent: an already closed/lost session closes
                // again successfully instead of stranding the caller.
                if name == "interactive_close"
                    && matches!(current, SessionState::Closed | SessionState::Lost)
                {
                    session["updated_at"] = json!(now());
                    self.store
                        .put("interact_session", &session_id, &agent.run_id, &session)?;
                    return Ok(json!({"session_id":session_id,"state":current,"reconciled":true}));
                }
                ensure!(
                    current == SessionState::Running
                        || (name == "interactive_close" && current == SessionState::Exited),
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
                        let lost = session_lost(&error);
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
