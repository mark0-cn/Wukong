//! Sessions tool operations.
use super::*;
use crate::adapters::worker::protocol::{SessionHandle, SessionObservation, SessionState};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_confirmation_accepts_native_proof_and_rejects_pending_cleanup() {
        assert!(SessionsService::cleanup_confirmed(
            &json!({"state":"closed"})
        ));
        assert!(!SessionsService::cleanup_confirmed(
            &json!({"state":"closed","cleanup_pending":true})
        ));
        assert!(!SessionsService::cleanup_confirmed(
            &json!({"state":"lost"})
        ));
        assert!(SessionsService::cleanup_confirmed(
            &json!({"state":"lost","cleanup_confirmed":true})
        ));
        let mut native = json!({"state":"lost","cleanup_pending":false,"result":{"jobs_empty":true,"profiles_removed":true,"scratch_removed":true}});
        assert!(SessionsService::cleanup_confirmed(&native));
        native["result"]["scratch_removed"] = json!(false);
        assert!(!SessionsService::cleanup_confirmed(&native));
    }

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
    pub(super) locks: ResourceLocks,
}
impl SessionsService {
    fn cleanup_confirmed(raw: &Value) -> bool {
        serde_json::from_value::<SessionObservation>(raw.clone())
            .is_ok_and(|observation| observation.observation().settled())
    }
    fn closing(
        &self,
        kind: SessionKind,
        session: &mut SessionRecord,
        agent: &AgentMetadata,
        call: &str,
    ) -> Result<()> {
        session.cleanup_intent.get_or_insert_with(|| CleanupIntent {
            call_id: call.into(),
            agent_id: agent.id.clone(),
        });
        session.state = SessionState::Closing;
        session.updated_at = now();
        self.store.save_session(kind, session)
    }
    fn close_failed(
        &self,
        kind: SessionKind,
        session: &mut SessionRecord,
        error: &anyhow::Error,
    ) -> Result<()> {
        session.state = SessionState::RecoveryRequired;
        session.updated_at = now();
        let worker = session.worker.get_or_insert_with(|| json!({}));
        worker["cleanup"] = json!({"desired":"closed","confirmed":false,"error":error.to_string().chars().take(512).collect::<String>()});
        self.store.save_session(kind, session)
    }
    fn confirm_close(
        &self,
        kind: SessionKind,
        session: &mut SessionRecord,
        raw: &Value,
    ) -> Result<()> {
        ensure!(
            raw["session_id"] == session.worker_resource_id,
            "worker returned another session identity"
        );
        let confirmed = Self::cleanup_confirmed(raw);
        if !confirmed {
            let error = anyhow::anyhow!("worker did not confirm session cleanup");
            self.close_failed(kind, session, &error)?;
            return Err(error);
        }
        session.state = if raw["state"] == "lost" {
            SessionState::Lost
        } else {
            SessionState::Closed
        };
        session.worker.get_or_insert_with(|| json!({}))["cleanup"] =
            json!({"desired":"closed","confirmed":true});
        session.updated_at = now();
        self.store.save_session(kind, session)
    }
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
                state: if matches!(handle.state, SessionState::Closed | SessionState::Lost)
                    && !Self::cleanup_confirmed(raw)
                {
                    SessionState::RecoveryRequired
                } else {
                    handle.state
                },
                worker: Some(raw.clone()),
                transcript_evidence_id: None,
                cleanup_intent: None,
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
        let lock = if let Some(id) = args["session_id"].as_str() {
            let mut locks = self.locks.lock().await;
            if locks.len() >= 256 {
                locks.retain(|_, held| Arc::strong_count(held) > 1);
            }
            Some(
                locks
                    .entry(format!("resource:session:{id}"))
                    .or_insert_with(|| Arc::new(Mutex::new(())))
                    .clone(),
            )
        } else {
            None
        };
        let _guard = if let Some(lock) = &lock {
            Some(lock.lock().await)
        } else {
            None
        };
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
                    current == SessionState::Running
                        || (name == "debug_close"
                            && matches!(
                                current,
                                SessionState::Closing | SessionState::RecoveryRequired
                            )),
                    "debug session is closed or lost"
                );
                args["session_id"] = json!(session.worker_resource_id);
                if name == "debug_close" {
                    self.closing(SessionKind::Debugger, &mut session, agent, call_id)?;
                }
                let cleanup_actor = if name == "debug_close" {
                    Some(require_in_run::<AgentMetadata>(
                        &self.store,
                        &session
                            .cleanup_intent
                            .as_ref()
                            .context("cleanup intent missing")?
                            .agent_id,
                        &agent.run_id,
                    )?)
                } else {
                    None
                };
                let worker_call = if cleanup_actor.is_some() {
                    session
                        .cleanup_intent
                        .as_ref()
                        .expect("cleanup intent is present when a cleanup actor exists")
                        .call_id
                        .as_str()
                } else {
                    call_id
                };
                let raw = match self
                    .gateway
                    .worker_call(
                        cleanup_actor.as_ref().unwrap_or(agent),
                        worker_call,
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
                        if name == "debug_close" {
                            self.close_failed(SessionKind::Debugger, &mut session, &error)?;
                        }
                        return Err(error);
                    }
                };
                if name == "debug_close" {
                    self.confirm_close(SessionKind::Debugger, &mut session, &raw)?;
                    if session.state == SessionState::Lost {
                        return Self::present(&session, raw);
                    }
                }
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
                session.updated_at = now();
                self.store.save_session(SessionKind::Debugger, &session)?;
                Ok(
                    json!({"response":Self::present(&session, raw)?,"transcript_evidence_id":evidence.id}),
                )
            }
            "debug_start" => {
                if let Some(target) = args["target"].as_str() {
                    self.files.known_file(agent, target, true)?;
                } else {
                    ensure!(
                        args["launch_spec"].is_string(),
                        "debugger requires a staged target or LaunchSpec"
                    );
                }
                if let Some(launch) = args["launch_spec"].as_str() {
                    self.files.known_file(agent, launch, false)?;
                }
                if let Some(input) = args["input"].as_str() {
                    self.files.known_file(agent, input, false)?;
                }
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
                if let Some(launch) = args["launch_spec"].as_str() {
                    self.files.known_file(agent, launch, false)?;
                    ensure!(
                        string_list(&args, "argv")?.is_empty() && args.get("env").is_none(),
                        "LaunchSpec owns interactive argv and environment"
                    );
                } else {
                    ensure!(
                        !string_list(&args, "argv")?.is_empty(),
                        "interactive argv cannot be empty"
                    );
                }
                if let Some(input) = args["input"].as_str() {
                    self.files.known_file(agent, input, false)?;
                }
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
                        || (name == "interactive_close"
                            && matches!(
                                current,
                                SessionState::Exited
                                    | SessionState::Closing
                                    | SessionState::RecoveryRequired
                            )),
                    "interactive session is closed or lost"
                );
                args["session_id"] = json!(session.worker_resource_id);
                if name == "interactive_close" {
                    self.closing(SessionKind::Interactive, &mut session, agent, call_id)?;
                }
                let cleanup_actor = if name == "interactive_close" {
                    Some(require_in_run::<AgentMetadata>(
                        &self.store,
                        &session
                            .cleanup_intent
                            .as_ref()
                            .context("cleanup intent missing")?
                            .agent_id,
                        &agent.run_id,
                    )?)
                } else {
                    None
                };
                let worker_call = if cleanup_actor.is_some() {
                    session
                        .cleanup_intent
                        .as_ref()
                        .expect("cleanup intent is present when a cleanup actor exists")
                        .call_id
                        .as_str()
                } else {
                    call_id
                };
                let raw = match self
                    .gateway
                    .worker_call(
                        cleanup_actor.as_ref().unwrap_or(agent),
                        worker_call,
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
                        if name == "interactive_close" {
                            self.close_failed(SessionKind::Interactive, &mut session, &error)?;
                        }
                        return Err(error);
                    }
                };
                if name == "interactive_close" {
                    self.confirm_close(SessionKind::Interactive, &mut session, &raw)?;
                } else if raw["state"].as_str() == Some("exited") {
                    session.state = SessionState::Exited;
                }
                session.worker = Some(raw.clone());
                session.updated_at = now();
                self.store
                    .save_session(SessionKind::Interactive, &session)?;
                Self::present(&session, raw)
            }
            _ => bail!("unknown session tool: {name}"),
        }
    }
}

impl SessionsService {
    pub(super) async fn events(
        &self,
        agent: &AgentMetadata,
        call: &str,
        args: &Value,
    ) -> Result<Value> {
        let session = require_session(
            &self.store,
            SessionKind::Debugger,
            text(args, "session_id")?,
            &agent.run_id,
        )?;
        self.authorize_session(agent, &session, false)?;
        let raw = self
            .gateway
            .worker_query(
                Method::DebugEvents.decode(json!({"session_id":session.worker_resource_id}))?,
            )
            .await?;
        let events = raw["events"]
            .as_array()
            .context("worker returned no event snapshot")?;
        let offset = crate::application::tools::host_usize(number(args, "offset", 0));
        ensure!(
            offset <= events.len(),
            "offset is beyond the current event snapshot; start a fresh snapshot at zero"
        );
        let limit = crate::application::tools::host_usize(number(args, "limit", 32).min(256));
        let page = events.iter().skip(offset).take(limit).collect::<Vec<_>>();
        let evidence = self.store.artifact(
            &agent.run_id,
            &agent.id,
            call,
            "debug.events.snapshot",
            &serde_json::to_vec(&raw)?,
            "application/json",
            json!({"session_id":session.id}),
        )?;
        Ok(
            json!({"session_id":session.id,"events":page,"next_offset":offset+page.len(),"snapshot_evidence_id":evidence.id,"total":events.len(),"at_snapshot_end":offset+page.len()>=events.len()}),
        )
    }
}
