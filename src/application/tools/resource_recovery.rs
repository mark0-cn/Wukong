//! Reconcile durable cleanup intent with existing worker resource owners.
use super::*;

impl ToolService {
    pub(crate) async fn reconcile_resources(&self, run: &str) -> Result<()> {
        self.observe_once(run).await
    }

    pub(super) async fn reconcile_session(
        &self,
        run: &str,
        kind: SessionKind,
        id: &str,
    ) -> Result<()> {
        let session = self.store.session(kind, id)?;
        if matches!(session.state, SessionState::Closed | SessionState::Lost) {
            return Ok(());
        }
        let owner: AgentMetadata = require_in_run(&self.store, &session.agent_id, run)?;
        if !matches!(
            session.state,
            SessionState::Closing | SessionState::RecoveryRequired
        ) && !matches!(owner.status, Status::Stopped | Status::Failed)
        {
            return Ok(());
        }
        let tools = self.bound_to(&owner).await?;
        // SessionsService persists the first operation identity before dispatch;
        // manual and background closes reconcile that same effect after timeout.
        tools
            .execution
            .sessions
            .session_tool(
                &owner,
                &format!("cleanup-session:{id}"),
                if kind == SessionKind::Debugger {
                    "debug_close"
                } else {
                    "interactive_close"
                },
                json!({"session_id":id}),
            )
            .await?;
        Ok(())
    }

    pub(super) async fn observe_job(&self, run: &str, id: &str, sync: bool) -> Result<()> {
        let job: JobRecord = require_in_run(&self.store, id, run)?;
        let owner: AgentMetadata = require_in_run(&self.store, &job.agent_id, run)?;
        let tools = self.bound_to(&owner).await?;
        if sync {
            tools
                .execution
                .jobs
                .import_job_artifacts(&owner, &format!("job-sync:{id}"), id)
                .await?;
        } else {
            tools
                .execution
                .jobs
                .observe(&owner, &format!("job-observation:{id}"), id)
                .await?;
            // A cancellation failure must not suppress the status observation.
            let cancellation = if !job.status.settled()
                && matches!(owner.status, Status::Stopped | Status::Failed)
            {
                tools
                    .execution
                    .runtime_tool(
                        &owner,
                        &format!("cleanup-job:{id}"),
                        "job_cancel",
                        json!({"job_id":id}),
                    )
                    .await
                    .map(|_| ())
            } else {
                Ok(())
            };
            cancellation?;
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
        // A manual close may have persisted intent just before controller loss.
        // Recovery must use this identity instead of its background call ID.
        let mut interrupted = store.session(SessionKind::Interactive, session_id)?;
        interrupted.state = SessionState::RecoveryRequired;
        interrupted.cleanup_intent = Some(CleanupIntent {
            call_id: "manual-close-before-restart".into(),
            agent_id: owner.id.clone(),
        });
        store.save_session(SessionKind::Interactive, &interrupted)?;
        coordinator.stop(&owner.id, "fixture stop")?;
        service.reconcile_resources(&run.id).await?;
        assert_eq!(
            store.session(SessionKind::Interactive, session_id)?.state,
            SessionState::Closed
        );
        assert_eq!(
            store
                .session(SessionKind::Interactive, session_id)?
                .cleanup_intent
                .unwrap()
                .call_id,
            "manual-close-before-restart"
        );
        service.reconcile_resources(&run.id).await?;
        service.close().await;
        Ok(())
    }
}
