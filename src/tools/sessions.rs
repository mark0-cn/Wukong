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

#[derive(Clone)]
pub(super) struct SessionsService {
    pub(super) store: Store,
    pub(super) gateway: WorkerGateway,
    pub(super) files: FilesService,
}
impl SessionsService {
    pub(super) fn authorize_session(
        &self,
        agent: &AgentMetadata,
        session: &SessionRecord,
        closing: bool,
    ) -> Result<()> {
        ensure!(
            session.worker_id == self.gateway.worker_identity()?,
            "session belongs to another runtime"
        );
        if session.agent_id != agent.id {
            let owner: AgentMetadata =
                require_in_run(&self.store, &session.agent_id, &agent.run_id)?;
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
            "debug_command" | "debug_close" | "debug_profile" | "debug_snapshot" => {
                let session_id = text(&args, "session_id")?.to_owned();
                let mut session: SessionRecord = require_session(
                    &self.store,
                    SessionKind::Debugger,
                    &session_id,
                    &agent.run_id,
                )?;
                self.authorize_session(agent, &session, name == "debug_close")?;
                let current = session.state;
                // Idempotent close, mirroring interactive_close: a closed/lost
                // debug session closes again successfully.
                if name == "debug_close"
                    && matches!(current, SessionState::Closed | SessionState::Lost)
                {
                    session.updated_at = now();
                    self.store.save_session(SessionKind::Debugger, &session)?;
                    return Ok(json!({"session_id":session_id,"state":current,"reconciled":true}));
                }
                ensure!(
                    current == SessionState::Running,
                    "debug session is closed or lost"
                );
                let raw = match self
                    .gateway
                    .worker_call(
                        agent,
                        call_id,
                        if name == "debug_close" {
                            "debug.close"
                        } else if name == "debug_profile" {
                            "debug.profile"
                        } else if name == "debug_snapshot" {
                            "debug.snapshot"
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
                            session.state = SessionState::Lost;
                            session.updated_at = now();
                            self.store.save_session(SessionKind::Debugger, &session)?;
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
                    .unwrap_or_else(|| {
                        format!("debug/{}/transcript.mi", session.worker_resource_id)
                    });
                let transcript = self
                    .gateway
                    .runtime()?
                    .call(
                        "file.get",
                        json!({"path":transcript,"max_bytes":8*1024*1024}),
                    )
                    .await?;
                let evidence = self.files.import_file(agent, call_id, &transcript)?;
                session.transcript_evidence_id = Some(evidence.id.clone());
                if name == "debug_close" {
                    session.state = SessionState::Closed;
                }
                session.updated_at = now();
                self.store.save_session(SessionKind::Debugger, &session)?;
                Ok(json!({"response":raw,"transcript_evidence_id":evidence.id}))
            }
            "debug_start" => {
                self.files.known_file(agent, text(&args, "target")?, true)?;
                let raw = self
                    .gateway
                    .worker_call(agent, call_id, "debug.start", args)
                    .await?;
                let handle: SessionHandle = serde_json::from_value(raw.clone())?;
                let session_id = handle.session_id.as_str();
                let record = SessionRecord {
                    id: session_id.into(),
                    worker_resource_id: text(&raw, "worker_session_id")?.into(),
                    run_id: agent.run_id.clone(),
                    agent_id: agent.id.clone(),
                    worker_id: self.gateway.worker_identity()?,
                    state: handle.state,
                    worker: Some(raw.clone()),
                    transcript_evidence_id: None,
                    created_at: now(),
                    updated_at: now(),
                };
                self.store.save_session(SessionKind::Debugger, &record)?;
                Ok(raw)
            }
            "interactive_start" => {
                ensure!(
                    !string_list(&args, "argv")?.is_empty(),
                    "interactive argv cannot be empty"
                );
                let raw = self
                    .gateway
                    .worker_call(agent, call_id, "interact.start", args)
                    .await?;
                let handle: SessionHandle = serde_json::from_value(raw.clone())?;
                let session_id = handle.session_id.as_str();
                let record = SessionRecord {
                    id: session_id.into(),
                    worker_resource_id: text(&raw, "worker_session_id")?.into(),
                    run_id: agent.run_id.clone(),
                    agent_id: agent.id.clone(),
                    worker_id: self.gateway.worker_identity()?,
                    state: handle.state,
                    worker: Some(raw.clone()),
                    transcript_evidence_id: None,
                    created_at: now(),
                    updated_at: now(),
                };
                self.store.save_session(SessionKind::Interactive, &record)?;
                Ok(raw)
            }
            "interactive_send" | "interactive_close" | "interactive_profile" => {
                let session_id = text(&args, "session_id")?.to_owned();
                let mut session: SessionRecord = require_session(
                    &self.store,
                    SessionKind::Interactive,
                    &session_id,
                    &agent.run_id,
                )?;
                self.authorize_session(agent, &session, name == "interactive_close")?;
                let current = session.state;
                // Closing is idempotent: an already closed/lost session closes
                // again successfully instead of stranding the caller.
                if name == "interactive_close"
                    && matches!(current, SessionState::Closed | SessionState::Lost)
                {
                    session.updated_at = now();
                    self.store
                        .save_session(SessionKind::Interactive, &session)?;
                    return Ok(json!({"session_id":session_id,"state":current,"reconciled":true}));
                }
                ensure!(
                    current == SessionState::Running
                        || (name == "interactive_close" && current == SessionState::Exited),
                    "interactive session is closed or lost"
                );
                let raw = match self
                    .gateway
                    .worker_call(
                        agent,
                        call_id,
                        if name == "interactive_close" {
                            "interact.close"
                        } else if name == "interactive_profile" {
                            "interact.profile"
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
                    session.state = if raw["state"].as_str() == Some("lost") {
                        SessionState::Lost
                    } else {
                        SessionState::Closed
                    };
                } else if raw["state"].as_str() == Some("exited") {
                    session.state = SessionState::Exited;
                }
                session.updated_at = now();
                self.store
                    .save_session(SessionKind::Interactive, &session)?;
                Ok(raw)
            }
            _ => bail!("unknown session tool: {name}"),
        }
    }
}
