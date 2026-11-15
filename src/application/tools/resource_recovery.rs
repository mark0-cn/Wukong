//! Reconcile controller cleanup intent with existing worker resource owners.
use super::*;

impl ToolService {
    /// Reconcile owned cleanup intents independently of model turns. Closing a
    /// session never replays debugger commands or interactive writes.
    pub(crate) async fn reconcile_resources(&self, run: &str) -> Result<()> {
        for kind in [SessionKind::Debugger, SessionKind::Interactive] {
            for listed in self.store.sessions(kind, run)? {
                let lock = {
                    let mut locks = self.locks.lock().await;
                    locks
                        .entry(format!("resource:session:{}", listed.id))
                        .or_insert_with(|| Arc::new(Mutex::new(())))
                        .clone()
                };
                let guard = lock.lock().await;
                let mut session = self.store.session(kind, &listed.id)?;
                if matches!(session.state, SessionState::Closed | SessionState::Lost) {
                    continue;
                }
                let owner: AgentMetadata = require_in_run(&self.store, &session.agent_id, run)?;
                let pending = matches!(
                    session.state,
                    SessionState::Closing | SessionState::RecoveryRequired
                );
                if !pending && !matches!(owner.status, Status::Stopped | Status::Failed) {
                    continue;
                }
                if pending && (now() - session.updated_at).num_seconds() < 5 {
                    continue;
                }
                session.state = SessionState::Closing;
                session.updated_at = now();
                self.store.save_session(kind, &session)?;
                drop(guard);
                let attempt = format!(
                    "cleanup:{}:{}",
                    session.id,
                    session.updated_at.timestamp_micros()
                );
                let result = async {
                    let tools = self.bound_to(&owner).await?;
                    tools
                        .execution
                        .sessions
                        .session_tool(
                            &owner,
                            &attempt,
                            if kind == SessionKind::Debugger {
                                "debug_close"
                            } else {
                                "interactive_close"
                            },
                            json!({"session_id":session.id}),
                        )
                        .await
                }
                .await;
                if let Err(error) = result {
                    tracing::warn!(session_id=%session.id, %error, "owned session cleanup remains pending");
                }
            }
        }
        for job in self.store.records::<JobRecord>(run)? {
            if job.status.settled() {
                continue;
            }
            let owner: AgentMetadata = require_in_run(&self.store, &job.agent_id, run)?;
            if !matches!(owner.status, Status::Stopped | Status::Failed) {
                continue;
            }
            let result = async {
                let tools = self.bound_to(&owner).await?;
                tools
                    .execution
                    .runtime_tool(
                        &owner,
                        &format!("cleanup-job:{}", job.id),
                        "job_cancel",
                        json!({"job_id":job.id}),
                    )
                    .await?;
                tools
                    .execution
                    .jobs
                    .observe(&owner, &format!("cleanup-job-status:{}", job.id), &job.id)
                    .await
            }
            .await;
            if let Err(error) = result {
                tracing::warn!(job_id=%job.id, %error, "owned job cleanup remains pending");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stopped_owner_sessions_are_reconciled_without_model_turns() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path().join("state"))?;
        let target = dir.path().join("sample");
        fs::write(&target, b"fixture")?;
        let coordinator = Coordinator::new(store.clone());
        let config = AppConfig {
            default_environment: Some("fixture".into()),
            environments: std::collections::BTreeMap::from([(
                "fixture".into(),
                crate::adapters::environments::EnvironmentConfig {
                    name: "Trusted fixture".into(),
                    platform: crate::adapters::environments::Platform::current(),
                    installation_id: None,
                    runtime: crate::adapters::worker::RuntimeConfig {
                        transport: crate::adapters::worker::TransportConfig::TrustedLocal(
                            crate::adapters::worker::TrustedLocalConfig {
                                state_dir: dir.path().join("worker"),
                                ..Default::default()
                            },
                        ),
                        ..Default::default()
                    },
                },
            )]),
            ..Default::default()
        };
        let run = coordinator.create_run(
            &target,
            "fixture",
            Budget::default(),
            serde_json::to_value(&config)?,
        )?;
        let owner = coordinator
            .create_child(
                &run.root_id,
                serde_json::from_value(
                    json!({"name":"worker","role":"analyst","task":"fixture","scope":"fixture"}),
                )?,
            )?
            .agent
            .metadata;
        let service = ToolService::new(config, store.clone()).await?;
        let started = service
            .execute(
                &owner.id,
                "session-start",
                "interactive_start",
                json!({"argv":["python","-u","-c","import time;time.sleep(300)"]}),
            )
            .await?;
        assert!(!started.is_error, "{started:?}");
        let session_id = text(&started.value["result"], "session_id")?;
        coordinator.stop(&owner.id, "fixture stop")?;
        service.reconcile_resources(&run.id).await?;
        assert_eq!(
            store.session(SessionKind::Interactive, session_id)?.state,
            SessionState::Closed
        );
        service.reconcile_resources(&run.id).await?;
        service.close().await;
        Ok(())
    }
}
