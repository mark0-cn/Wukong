//! Streamed artifact transfer. Model results contain references, never large byte payloads.
use super::*;
use std::io::{Read, Seek, SeekFrom, Write};

const CHUNK_BYTES: usize = 1024 * 1024;
const MAX_BYTES: u64 = 16 * 1024 * 1024 * 1024;

impl FilesService {
    pub(super) async fn upload_path(
        &self,
        agent: &AgentMetadata,
        call: &str,
        source: &std::path::Path,
        path: &str,
        expected_hash: &str,
        executable: bool,
    ) -> Result<Value> {
        let mut file = fs::File::open(source)?;
        let size = file.metadata()?.len();
        ensure!(size <= MAX_BYTES, "artifact exceeds 16 GiB transfer limit");
        ensure!(
            crate::content::sha256_file(source)? == expected_hash,
            "source artifact digest mismatch"
        );
        let begin = self
            .gateway
            .worker_call(
                agent,
                &format!("{call}:begin"),
                Request::UploadBegin(serde_json::from_value(
                    json!({"path":path,"sha256":expected_hash,"size":size,"executable":executable}),
                )?),
            )
            .await?;
        let transfer = text(&begin, "transfer_id")?;
        let status = self
            .gateway
            .worker_query(Request::UploadStatus(serde_json::from_value(
                json!({"transfer_id":transfer}),
            )?))
            .await?;
        let mut offset = status["offset"].as_u64().context("upload offset missing")?;
        ensure!(offset <= size, "invalid worker upload offset");
        file.seek(SeekFrom::Start(offset))?;
        let mut buffer = vec![0; CHUNK_BYTES];
        while offset < size {
            let count = file.read(&mut buffer)?;
            ensure!(count > 0, "source artifact was truncated during transfer");
            let next = self.gateway.worker_call(agent, &format!("{call}:chunk:{offset}"), Request::UploadWrite(serde_json::from_value(json!({"transfer_id":transfer,"offset":offset,"data_base64":BASE64.encode(&buffer[..count])}))?)).await?;
            offset += count as u64;
            ensure!(
                next["offset"].as_u64() == Some(offset),
                "worker acknowledged the wrong upload offset"
            );
        }
        let raw = self
            .gateway
            .worker_call(
                agent,
                call,
                Request::UploadCommit(serde_json::from_value(json!({"transfer_id":transfer}))?),
            )
            .await?;
        ensure!(
            raw["sha256"] == expected_hash && raw["size"].as_u64() == Some(size),
            "uploaded artifact integrity mismatch"
        );
        Ok(raw)
    }

    pub(super) async fn stage_artifact(
        &self,
        agent: &AgentMetadata,
        call: &str,
        args: &Value,
    ) -> Result<Value> {
        let evidence_id = text(args, "artifact_id")?;
        let evidence: Evidence = require_in_run(&self.store, evidence_id, &agent.run_id)?;
        let path = self
            .store
            .verified_evidence_path(&agent.run_id, evidence_id)?;
        let executable = args["executable"].as_bool().unwrap_or(false);
        let remote = format!(
            "artifacts/{}/{}",
            evidence.sha256,
            if executable {
                "executable.bin"
            } else {
                "content.bin"
            }
        );
        let raw = self
            .upload_path(agent, call, &path, &remote, &evidence.sha256, executable)
            .await?;
        self.register_file(agent, &raw, Some(evidence_id))?;
        Ok(
            json!({"file":raw,"content_evidence_id":evidence_id,"environment_id":agent.environment_id}),
        )
    }

    pub(super) async fn download_file(
        &self,
        agent: &AgentMetadata,
        call: &str,
        path: &str,
        maximum: u64,
    ) -> Result<Value> {
        let metadata = self
            .gateway
            .worker_query(Request::FileStat(serde_json::from_value(
                json!({"path":path}),
            )?))
            .await?;
        let size = metadata["size"].as_u64().context("artifact size missing")?;
        ensure!(
            size <= maximum.min(MAX_BYTES),
            "artifact exceeds requested transfer limit"
        );
        let mut file = tempfile::NamedTempFile::new()?;
        let mut digest = Sha256::new();
        let mut offset = 0_u64;
        while offset < size {
            let raw = self
                .gateway
                .worker_query(Request::FileRead(serde_json::from_value(
                    json!({"path":path,"offset":offset,"max_bytes":CHUNK_BYTES}),
                )?))
                .await?;
            let bytes = BASE64.decode(text(&raw, "data_base64")?)?;
            ensure!(
                !bytes.is_empty()
                    && bytes.len() <= CHUNK_BYTES
                    && offset + bytes.len() as u64 <= size
                    && raw["offset"].as_u64() == Some(offset)
                    && raw["size"].as_u64() == Some(size)
                    && raw["chunk_sha256"] == binary::sha256_bytes(&bytes),
                "invalid artifact chunk"
            );
            digest.update(&bytes);
            file.write_all(&bytes)?;
            offset += bytes.len() as u64;
        }
        ensure!(
            metadata["sha256"] == hex::encode(digest.finalize()),
            "downloaded artifact digest mismatch"
        );
        file.as_file().sync_all()?;
        let evidence = self.store.artifact_file(
            &agent.run_id,
            &agent.id,
            call,
            "runtime.file",
            file.path(),
            "application/octet-stream",
            json!({"worker_path":path,"worker_id":self.gateway.worker_identity()?}),
        )?;
        ensure!(
            metadata["sha256"] == evidence.sha256 && size == evidence.size,
            "imported artifact integrity mismatch"
        );
        self.register_file(agent, &metadata, Some(&evidence.id))?;
        Ok(json!({"file":metadata,"content_evidence_id":evidence.id}))
    }
}
