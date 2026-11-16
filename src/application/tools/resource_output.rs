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
                let path = manifest["artifacts"]
                    .as_array()
                    .and_then(|items| {
                        items
                            .iter()
                            .filter_map(|item| item["path"].as_str())
                            .find(|path| {
                                path.ends_with(&format!("/{stream}"))
                                    || path.ends_with(&format!("/output.{stream}"))
                            })
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
            (bytes, number(&raw, "size", 0))
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
