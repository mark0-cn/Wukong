//! Bounded-memory ingestion of large evidence artifacts.
use super::*;

impl Store {
    pub fn verified_evidence_path(&self, run: &str, evidence_id: &str) -> Result<PathBuf> {
        let record: Evidence = self.require("evidence", evidence_id)?;
        if record.run_id != run {
            bail!("evidence belongs to another run");
        }
        let path = fs::canonicalize(self.root.join(&record.relative_path))?;
        if !path.starts_with(self.root.as_path()) {
            bail!("artifact escapes state directory");
        }
        if !path.is_file()
            || fs::metadata(&path)?.len() != record.size
            || crate::content::sha256_file(&path)? != record.sha256
        {
            bail!("artifact integrity failure");
        }
        Ok(path)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn artifact_file(
        &self,
        run: &str,
        agent: &str,
        call: &str,
        source: &str,
        input: &Path,
        media_type: &str,
        metadata: Value,
    ) -> Result<Evidence> {
        let directory = self.root.join("artifacts");
        let mut staged = tempfile::NamedTempFile::new_in(&directory)?;
        let mut stream = fs::File::open(input)?;
        let mut digest = Sha256::new();
        let mut size = 0_u64;
        let mut buffer = vec![0; 1024 * 1024];
        loop {
            let count = stream.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            size += count as u64;
            if size > 16 * 1024 * 1024 * 1024 {
                bail!("artifact exceeds 16 GiB limit");
            }
            digest.update(&buffer[..count]);
            staged.write_all(&buffer[..count])?;
        }
        staged.as_file().sync_all()?;
        let hash = hex::encode(digest.finalize());
        let relative = format!("artifacts/{}/{}", &hash[..2], hash);
        let destination = self.root.join(&relative);
        let parent = destination.parent().context("artifact parent missing")?;
        fs::create_dir_all(parent)?;
        if let Err(error) = staged.persist_noclobber(&destination)
            && error.error.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(error.error).context("publish evidence artifact");
        }
        if fs::metadata(&destination)?.len() != size
            || crate::content::sha256_file(&destination)? != hash
        {
            bail!("published artifact integrity failure");
        }
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        let evidence = Evidence {
            id: id("ev"),
            run_id: run.into(),
            agent_id: agent.into(),
            tool_call_id: call.into(),
            sha256: hash,
            relative_path: relative,
            size,
            media_type: media_type.into(),
            source: source.into(),
            metadata,
            created_at: now(),
        };
        self.put("evidence", &evidence.id, run, &evidence)?;
        Ok(evidence)
    }
}
