//! Cursor reads of an owned resource's declared output, with immutable chunk evidence.
use super::*;

impl ExecutionService {
    pub(super) async fn resource_read(
        &self,
        agent: &AgentMetadata,
        call: &str,
        args: &Value,
    ) -> Result<Value> {
        let id = text(args, "resource_id")?;
        let offset = number(args, "offset", 0);
        let length = number(args, "length", 8192).min(65536);
        let (path, settled, snapshot) = match text(args, "kind")? {
            "job" => {
                let job = self.jobs.known_job(agent, id, true)?;
                let stream = text(args, "stream")?;
                ensure!(
                    matches!(stream, "stdout" | "stderr"),
                    "job output requires stdout or stderr"
                );
                let manifest = self
                    .gateway
                    .worker_query(
                        Method::JobArtifacts.decode(json!({"job_id":job.worker_resource_id}))?,
                    )
                    .await?;
                let path = manifest
                    .as_array()
                    .context("worker artifact manifest is not an array")?
                    .iter()
                    .filter_map(|item| item["path"].as_str())
                    .find(|path| {
                        path.ends_with(&format!("/{stream}"))
                            || path.ends_with(&format!("/output.{stream}"))
                    })
                    .context(
                        "output stream not published by worker yet; query job_status and retry",
                    )?;
                (path.to_owned(), job.status.settled(), None)
            }
            kind => {
                let kind = match kind {
                    "debugger" => SessionKind::Debugger,
                    "interactive" => SessionKind::Interactive,
                    _ => bail!("unsupported resource kind"),
                };
                ensure!(
                    args["stream"].as_str() == Some("transcript"),
                    "session output requires transcript"
                );
                let session = self.store.session(kind, id)?;
                ensure!(
                    session.run_id == agent.run_id
                        && session.agent_id == agent.id
                        && session.worker_id == self.gateway.worker_identity()?,
                    "resource belongs to another run, agent or worker"
                );
                let settled = session.state == SessionState::Closed;
                let path = session
                    .worker
                    .as_ref()
                    .and_then(|worker| worker["transcript"].as_str())
                    .map(str::to_owned)
                    .unwrap_or_default();
                (path, settled, session.transcript_evidence_id)
            }
        };
        let (bytes, total) = if let Some(id) = snapshot {
            let evidence: Evidence = require_in_run(&self.store, &id, &agent.run_id)?;
            ensure!(
                offset <= evidence.size,
                "offset is beyond this transcript snapshot"
            );
            (
                self.store
                    .read_evidence_range(&agent.run_id, &id, offset, length as usize)?,
                evidence.size,
            )
        } else {
            ensure!(
                !path.is_empty(),
                "no transcript snapshot has been published; inspect/send first"
            );
            let raw = self
                .gateway
                .worker_query(Request::FileRead(serde_json::from_value(
                    json!({"path":path,"offset":offset,"max_bytes":length}),
                )?))
                .await?;
            let bytes = BASE64.decode(text(&raw, "data_base64")?)?;
            ensure!(
                bytes.len() <= length as usize
                    && raw["offset"] == offset
                    && raw["chunk_sha256"] == hex::encode(Sha256::digest(&bytes)),
                "worker chunk identity or bound mismatch"
            );
            let size = raw["size"].as_u64().context("worker omitted output size")?;
            ensure!(
                offset <= size && bytes.len() as u64 <= size - offset,
                "worker output size or offset mismatch"
            );
            (bytes, size)
        };
        let next = offset
            .checked_add(bytes.len() as u64)
            .context("offset overflow")?;
        let evidence = self.store.artifact(
            &agent.run_id,
            &agent.id,
            call,
            "resource.output.chunk",
            &bytes,
            "application/octet-stream",
            json!({"resource_id":id,"path":path,"offset":offset,"observed_total":total}),
        )?;
        Ok(
            json!({"resource_id":id,"offset":offset,"next_offset":next,"observed_total":total,"at_snapshot_end":next>=total,"eof":settled && next>=total,"data_base64":BASE64.encode(bytes),"evidence_id":evidence.id}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn transcript_pages_preserve_owner_offsets_and_lifecycle() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let target = temp.path().join("target");
        fs::write(&target, b"trusted fixture")?;
        let mut config = AppConfig {
            default_environment: Some("fixture".into()),
            ..Default::default()
        };
        config.environments.insert(
            "fixture".into(),
            crate::adapters::environments::EnvironmentConfig {
                name: "Fixture".into(),
                platform: crate::adapters::environments::Platform::current(),
                installation_id: None,
                runtime: crate::adapters::worker::RuntimeConfig {
                    transport: crate::adapters::worker::TransportConfig::TrustedLocal(
                        crate::adapters::worker::TrustedLocalConfig {
                            state_dir: temp.path().join("worker"),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                },
            },
        );
        let store = Store::open(temp.path().join("state"))?;
        let coordinator = Coordinator::new(store.clone());
        let run = coordinator.create_run(
            &target,
            "resource pages",
            Budget::default(),
            serde_json::to_value(&config)?,
        )?;
        let agent = coordinator
            .create_child(
                &run.root_id,
                crate::application::coordinator::ChildRequest {
                    name: "Reader".into(),
                    role: Role::Analyst,
                    task: "read trusted transcript".into(),
                    scope: "fixture".into(),
                    environment_id: None,
                    skills: vec![],
                    inherit_context: false,
                    wait_for_predecessors: vec![],
                    dependency_policy: Default::default(),
                    contract: Default::default(),
                },
            )?
            .agent;
        let tools = ToolService::new(config, store.clone()).await?;
        let execution = tools.bound_to(&agent).await?.execution;
        let original = store.artifact(
            &run.id,
            &agent.id,
            "transcript",
            "fixture",
            b"abcdef",
            "text/plain",
            json!({}),
        )?;
        let mut session = SessionRecord {
            id: "fixture-session".into(),
            worker_resource_id: "fixture-worker-session".into(),
            run_id: run.id.clone(),
            agent_id: agent.id.clone(),
            worker_id: execution.gateway.worker_identity()?,
            state: SessionState::Running,
            worker: None,
            transcript_evidence_id: Some(original.id),
            cleanup_intent: None,
            created_at: now(),
            updated_at: now(),
        };
        store.save_session(SessionKind::Debugger, &session)?;
        let mut input =
            json!({"kind":"debugger","resource_id":session.id,"stream":"transcript","length":3});
        let first = execution.resource_read(&agent, "read-one", &input).await?;
        assert_eq!(first["data_base64"], BASE64.encode(b"abc"));
        assert_eq!(first["next_offset"], 3);
        assert_eq!(first["at_snapshot_end"], false);
        assert_eq!(
            store.read_evidence(&run.id, first["evidence_id"].as_str().unwrap())?,
            b"abc"
        );
        input["offset"] = first["next_offset"].clone();
        for state in [
            SessionState::Running,
            SessionState::Lost,
            SessionState::Closed,
        ] {
            session.state = state;
            store.save_session(SessionKind::Debugger, &session)?;
            let end = execution
                .resource_read(&agent, &format!("read-{state:?}"), &input)
                .await?;
            assert_eq!(end["at_snapshot_end"], true);
            assert_eq!(end["eof"], state == SessionState::Closed);
        }
        let mut other = agent.clone();
        other.metadata.id = "foreign-agent".into();
        assert!(
            execution
                .resource_read(&other, "foreign-read", &input)
                .await
                .is_err()
        );
        input["offset"] = json!(7);
        assert!(
            execution
                .resource_read(&agent, "past-end", &input)
                .await
                .is_err()
        );
        tools.close().await;
        Ok(())
    }
}
