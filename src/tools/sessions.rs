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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_start_preserves_terminal_state_evidence_and_ownership() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("state"))?;
        let target = temp.path().join("fixture.bin");
        fs::write(&target, b"trusted inert fixture")?;
        let coordinator = Coordinator::new(store.clone());
        let run = coordinator.create_run(&target, "fixture", Budget::default(), json!({}))?;
        let mut agent: AgentMetadata = store.get_record(&run.root_id)?.context("root missing")?;
        for kind in [SessionKind::Debugger, SessionKind::Interactive] {
            let raw = json!({"session_id":"worker-resource","state":"running"});
            let mut record =
                SessionsService::record_start(&store, "worker".into(), &agent, kind, &raw)?;
            let created_at = record.created_at;
            record.state = SessionState::Closed;
            record.transcript_evidence_id = Some("original-transcript".into());
            store.save_session(kind, &record)?;
            let repeated =
                SessionsService::record_start(&store, "worker".into(), &agent, kind, &raw)?;
            assert_eq!(repeated.state, SessionState::Closed);
            assert_eq!(repeated.created_at, created_at);
            assert_eq!(
                repeated.transcript_evidence_id,
                record.transcript_evidence_id
            );
            assert_eq!(repeated.worker, Some(raw.clone()));
            agent.run_id = "another-run".into();
            assert!(
                SessionsService::record_start(&store, "worker".into(), &agent, kind, &raw).is_err()
            );
            agent.run_id = run.id.clone();
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct SessionsService {
    pub(super) store: Store,
    pub(super) gateway: WorkerGateway,
    pub(super) files: FilesService,
}
impl SessionsService {
    fn record_start(
        store: &Store,
        worker_id: String,
        agent: &AgentMetadata,
        kind: SessionKind,
        raw: &Value,
    ) -> Result<SessionRecord> {
        let handle: SessionHandle = serde_json::from_value(raw.clone())?;
        let id = stable("session", &format!("{worker_id}:{}", handle.session_id));
        store.write(|tx| {
            if let Some(record) = tx.session(kind, &id)? {
                ensure!(
                    record.worker_resource_id == handle.session_id
                        && record.worker_id == worker_id
                        && record.run_id == agent.run_id
                        && record.agent_id == agent.id,
                    "session receipt belongs to another owner"
                );
                // A cached start receipt is historical evidence. Never reopen a closed
                // session or discard observations imported after that receipt.
                return Ok(record);
            }
            let record = SessionRecord {
                id,
                worker_resource_id: handle.session_id,
                run_id: agent.run_id.clone(),
                agent_id: agent.id.clone(),
                worker_id,
                state: handle.state,
                worker: Some(raw.clone()),
                transcript_evidence_id: None,
                created_at: now(),
                updated_at: now(),
            };
            tx.save_session(kind, &record)?;
            Ok(record)
        })
    }
    fn present(session: &SessionRecord, mut raw: Value) -> Result<Value> {
        if let Some(id) = raw.get("session_id").and_then(Value::as_str) {
            ensure!(
                id == session.worker_resource_id,
                "worker returned another session identity"
            );
            raw["worker_session_id"] = json!(session.worker_resource_id);
            raw["session_id"] = json!(session.id);
        }
        Ok(raw)
    }

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
        mut args: Value,
    ) -> Result<Value> {
        match name {
            "debug_command" | "debug_inspect" | "debug_control" | "debug_close"
            | "debug_profile" | "debug_snapshot" => {
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
                args["session_id"] = json!(session.worker_resource_id);
                let raw = match self
                    .gateway
                    .worker_call(
                        agent,
                        call_id,
                        (if name == "debug_close" {
                            Method::DebugClose
                        } else if name == "debug_profile" {
                            Method::DebugProfile
                        } else if name == "debug_snapshot" {
                            Method::DebugSnapshot
                        } else if name == "debug_inspect" {
                            Method::DebugInspect
                        } else if name == "debug_control" {
                            Method::DebugControl
                        } else {
                            Method::DebugCommand
                        })
                        .decode(args)?,
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
                        Method::FileGet.name(),
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
                Ok(
                    json!({"response":Self::present(&session, raw)?,"transcript_evidence_id":evidence.id}),
                )
            }
            "debug_start" => {
                self.files.known_file(agent, text(&args, "target")?, true)?;
                let dump_mode = args["mode"].as_str() == Some("dump");
                ensure!(
                    dump_mode == args["dump"].is_string(),
                    "dump mode requires a dump artifact; launch mode cannot supply a dump"
                );
                if dump_mode {
                    self.files.known_file(agent, text(&args, "dump")?, false)?;
                }
                let raw = self
                    .gateway
                    .worker_call(
                        agent,
                        call_id,
                        Request::DebugStart(serde_json::from_value(args)?),
                    )
                    .await?;
                let record = Self::record_start(
                    &self.store,
                    self.gateway.worker_identity()?,
                    agent,
                    SessionKind::Debugger,
                    &raw,
                )?;
                Self::present(&record, raw)
            }
            "interactive_start" => {
                ensure!(
                    !string_list(&args, "argv")?.is_empty(),
                    "interactive argv cannot be empty"
                );
                let raw = self
                    .gateway
                    .worker_call(
                        agent,
                        call_id,
                        Request::InteractStart(serde_json::from_value(args)?),
                    )
                    .await?;
                let record = Self::record_start(
                    &self.store,
                    self.gateway.worker_identity()?,
                    agent,
                    SessionKind::Interactive,
                    &raw,
                )?;
                Self::present(&record, raw)
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
                args["session_id"] = json!(session.worker_resource_id);
                let raw = match self
                    .gateway
                    .worker_call(
                        agent,
                        call_id,
                        (if name == "interactive_close" {
                            Method::InteractClose
                        } else if name == "interactive_profile" {
                            Method::InteractProfile
                        } else {
                            Method::InteractWrite
                        })
                        .decode(args)?,
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
                            json!({"session_id":session.worker_resource_id,"state":"lost","reconciled":true})
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
                Self::present(&session, raw)
            }
            _ => bail!("unknown session tool: {name}"),
        }
    }
}
