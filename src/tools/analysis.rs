//! Analysis tool operations.
use super::*;

impl ToolService {
    pub(super) async fn ghidra(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let run = self.target(agent)?;
        let query = match name {
            "functions_search" => GhidraQuery::Functions {
                pattern: args["pattern"].as_str().map(str::to_owned),
                offset: number(args, "offset", 0) as usize,
                limit: number(args, "limit", 25) as usize,
            },
            "function_decompile" => GhidraQuery::Decompile {
                address: number(args, "address", 0),
            },
            _ => GhidraQuery::Xrefs {
                address: number(args, "address", 0),
                direction: match args["direction"].as_str().unwrap_or("both") {
                    "to" => XrefDirection::To,
                    "from" => XrefDirection::From,
                    _ => XrefDirection::Both,
                },
                limit: number(args, "limit", 100) as usize,
            },
        };
        let executable = self
            .config
            .analysis
            .ghidra_executable
            .clone()
            .or_else(|| std::env::var_os("RE_AGENT_GHIDRA_HEADLESS").map(PathBuf::from));
        if executable.is_none() && self.runtime.is_some() {
            let staged = self
                .stage_target(agent, &format!("{call_id}:target"))
                .await?;
            let result=self.worker_call(agent,call_id,"ghidra.query",json!({"target":staged["path"],"target_sha256":run.target_sha256,"query":query,"timeout_secs":self.config.analysis.timeout_seconds,"analysis_timeout_secs":self.config.analysis.analysis_timeout_seconds,"max_cpu":self.config.analysis.max_cpu})).await?;
            ensure!(
                result["target_sha256"] == run.target_sha256,
                "remote Ghidra analyzed another target"
            );
            let mut result = result;
            let mut evidence_ids = Vec::new();
            if let Some(artifacts) = result["artifacts"].as_array() {
                for artifact in artifacts {
                    let raw = self
                        .runtime()?
                        .call(
                            "file.get",
                            json!({"path":artifact["path"],"max_bytes":32*1024*1024}),
                        )
                        .await?;
                    ensure!(
                        raw["sha256"] == artifact["sha256"],
                        "remote Ghidra artifact changed during import"
                    );
                    evidence_ids.push(self.import_file(agent, call_id, &raw)?.id);
                }
            }
            // The per-artifact metadata (binding/execution/request/response
            // digests) is redundant inline: the artifacts themselves are
            // already imported as evidence above. Keep the inline result to
            // the analysis payload the model actually reads.
            if let Some(object) = result.as_object_mut() {
                object.remove("artifacts");
            }
            return Ok(json!({"analysis":result,"artifact_evidence_ids":evidence_ids}));
        }
        let executable=executable.context("Ghidra is not configured; set analysis.ghidra_executable, RE_AGENT_GHIDRA_HEADLESS, or a worker with Ghidra")?;
        let backend = GhidraBackend::new(GhidraConfig {
            executable,
            cache_dir: self.config.analysis.ghidra_cache_dir.clone(),
            script_dir: self.config.analysis.ghidra_script_dir.clone(),
            timeout_seconds: self.config.analysis.timeout_seconds,
            analysis_timeout_seconds: self.config.analysis.analysis_timeout_seconds,
            max_cpu: self.config.analysis.max_cpu,
        })?;
        let result = backend.query(&run.target_path, &query).await?;
        let mut logs = Vec::new();
        for (kind, path) in [
            ("stdout", &result.stdout_path),
            ("stderr", &result.stderr_path),
        ] {
            let bytes = fs::read(path)?;
            let evidence = self.store.artifact(
                &agent.run_id,
                &agent.id,
                call_id,
                &format!("ghidra.{kind}"),
                &bytes,
                "text/plain",
                json!({"target_sha256":run.target_sha256,"tool_version":result.tool_version}),
            )?;
            logs.push(evidence.id);
        }
        Ok(json!({"analysis":result,"log_evidence_ids":logs}))
    }
}
