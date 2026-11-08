//! Files tool operations.
use super::*;

impl ToolService {
    pub(super) fn register_file(
        &self,
        agent: &AgentRecord,
        raw: &Value,
        evidence_id: Option<&str>,
    ) -> Result<()> {
        let path = raw["path"].as_str().context("worker file path missing")?;
        let file_id = stable("file", &format!("{}:{path}", agent.run_id));
        let previous: Option<Value> = self.store.get("worker_file", &file_id)?;
        let retained = previous
            .as_ref()
            .filter(|record| record["sha256"] == raw["sha256"])
            .and_then(|record| record["evidence_id"].as_str());
        let record = json!({"id":file_id,"run_id":agent.run_id,"agent_id":agent.id,"worker_id":self.worker_identity()?,"path":path,"sha256":raw["sha256"],"size":raw["size"],"evidence_id":evidence_id.or(retained),"created_at":now()});
        self.store.put(
            "worker_file",
            record["id"].as_str().context("file ID missing")?,
            &agent.run_id,
            &record,
        )
    }

    pub(super) fn known_file(
        &self,
        agent: &AgentRecord,
        path: &str,
        require_target: bool,
    ) -> Result<Value> {
        let id = stable("file", &format!("{}:{path}", agent.run_id));
        let record: Value = self.store.require("worker_file", &id)?;
        ensure!(
            record["run_id"] == agent.run_id && record["worker_id"] == self.worker_identity()?,
            "worker file belongs to another assessment or runtime"
        );
        if require_target {
            let run: RunRecord = self.store.require("run", &agent.run_id)?;
            ensure!(
                record["sha256"] == run.target_sha256,
                "worker target does not match this run's sample SHA256"
            );
        }
        Ok(record)
    }

    pub(super) async fn stage_target(&self, agent: &AgentRecord, call_id: &str) -> Result<Value> {
        let run = self.target(agent)?;
        let bytes = fs::read(&run.target_path)?;
        ensure!(
            bytes.len() <= 32 * 1024 * 1024,
            "worker staging currently supports samples up to 32 MiB"
        );
        let raw=self.worker_call(agent,call_id,"file.put",json!({"path":format!("targets/{}/sample.bin",run.target_sha256),"data_base64":BASE64.encode(&bytes),"executable":true})).await?;
        ensure!(
            raw["sha256"] == run.target_sha256,
            "worker staged bytes differ from run target"
        );
        self.register_file(agent, &raw, None)?;
        Ok(raw)
    }

    pub(super) async fn file_put(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        args: &Value,
    ) -> Result<Value> {
        ensure!(
            args.get("data_base64").is_some() != args.get("evidence_id").is_some(),
            "supply exactly one of data_base64 or evidence_id"
        );
        let bytes = if let Some(evidence) = args["evidence_id"].as_str() {
            self.store.read_evidence(&agent.run_id, evidence)?
        } else {
            BASE64
                .decode(
                    args["data_base64"]
                        .as_str()
                        .context("data_base64 must be a string")?,
                )
                .context("invalid base64 upload")?
        };
        ensure!(
            bytes.len() <= 32 * 1024 * 1024,
            "file upload exceeds 32 MiB"
        );
        let raw=self.worker_call(agent,call_id,"file.put",json!({"path":text(args,"path")?,"data_base64":BASE64.encode(&bytes),"executable":args["executable"].as_bool().unwrap_or(false)})).await?;
        ensure!(
            raw["sha256"] == binary::sha256_bytes(&bytes),
            "worker upload hash mismatch"
        );
        let evidence = self.store.artifact(
            &agent.run_id,
            &agent.id,
            call_id,
            "file_put.original",
            &bytes,
            "application/octet-stream",
            json!({"worker_path":raw["path"]}),
        )?;
        self.register_file(agent, &raw, Some(&evidence.id))?;
        Ok(json!({"file":raw,"content_evidence_id":evidence.id}))
    }

    pub(super) async fn file_get(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        args: &Value,
    ) -> Result<Value> {
        let path = text(args, "path")?;
        if self.known_file(agent, path, false).is_err() {
            let allowed = self
                .store
                .list::<Value>("job", &agent.run_id)?
                .iter()
                .any(|job| {
                    job["worker_id"] == self.worker_identity().unwrap_or_default()
                        && job["id"]
                            .as_str()
                            .is_some_and(|id| path.starts_with(&format!("jobs/{id}/")))
                });
            ensure!(allowed, "worker path has not been declared for this run");
        }
        let raw = self
            .runtime()?
            .call(
                "file.get",
                json!({"path":path,"max_bytes":number(args,"max_bytes",32*1024*1024)}),
            )
            .await?;
        let evidence = self.import_file(agent, call_id, &raw)?;
        Ok(json!({"file":raw,"content_evidence_id":evidence.id}))
    }

    pub(super) fn import_file(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        raw: &Value,
    ) -> Result<Evidence> {
        let bytes = BASE64
            .decode(
                raw["data_base64"]
                    .as_str()
                    .context("worker file has no base64 content")?,
            )
            .context("worker returned invalid file base64")?;
        ensure!(
            raw["sha256"] == binary::sha256_bytes(&bytes)
                && raw["size"].as_u64() == Some(bytes.len() as u64),
            "worker file integrity mismatch"
        );
        let evidence = self.store.artifact(
            &agent.run_id,
            &agent.id,
            call_id,
            "runtime.file",
            &bytes,
            "application/octet-stream",
            json!({"worker_path":raw["path"],"worker_id":self.worker_identity()?}),
        )?;
        self.register_file(agent, raw, Some(&evidence.id))?;
        Ok(evidence)
    }
}
