//! Analysis tool operations.
use super::*;

#[derive(Clone)]
pub(super) struct AnalysisService {
    pub(super) store: Store,
    pub(super) gateway: WorkerGateway,
    pub(super) files: FilesService,
    pub(super) static_gateway: Option<WorkerGateway>,
    pub(super) static_files: Option<FilesService>,
    pub(super) config: crate::config::StaticConfig,
}
impl AnalysisService {
    fn remote_backend(&self) -> Option<(&FilesService, &WorkerGateway)> {
        match (&self.static_files, &self.static_gateway) {
            (Some(files), Some(gateway)) => Some((files, gateway)),
            _ if self.gateway.worker.is_some() => Some((&self.files, &self.gateway)),
            _ => None,
        }
    }

    /// Resolve and keep secondary bytes alive for every static backend.
    pub(super) fn resolve_target(
        &self,
        agent: &AgentMetadata,
        args: &Value,
    ) -> Result<(RunRecord, Option<tempfile::NamedTempFile>)> {
        let mut run = target(&self.store, agent)?;
        if let Some(reference) = args.get("target_ref").filter(|v| !v.is_null()) {
            match serde_json::from_value::<args::TargetRef>(reference.clone())? {
                args::TargetRef::Primary => {}
                args::TargetRef::Evidence { evidence_id } => {
                    let record: Evidence =
                        require_in_run(&self.store, &evidence_id, &agent.run_id)?;
                    ensure!(
                        record.size <= 32 * 1024 * 1024,
                        "secondary binary exceeds 32 MiB"
                    );
                    let bytes = self.store.read_evidence(&agent.run_id, &evidence_id)?;
                    let mut file = tempfile::NamedTempFile::new()?;
                    std::io::Write::write_all(&mut file, &bytes)?;
                    run.target_path = file.path().into();
                    run.target_sha256 = record.sha256;
                    return Ok((run, Some(file)));
                }
            }
        }
        Ok((run, None))
    }

    pub(super) async fn ghidra(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let (run, secondary) = self.resolve_target(agent, args)?;
        let query = match name {
            "functions_search" => GhidraQuery::Functions {
                pattern: args["pattern"].as_str().map(str::to_owned),
                offset: crate::application::tools::host_usize(number(args, "offset", 0)),
                limit: crate::application::tools::host_usize(number(args, "limit", 25)),
            },
            "function_instructions" => GhidraQuery::Instructions {
                address: static_address(args, &run)?,
                offset: crate::application::tools::host_usize(number(args, "offset", 0)),
                limit: crate::application::tools::host_usize(number(args, "limit", 100)),
            },
            "function_decompile" => GhidraQuery::Decompile {
                address: static_address(args, &run)?,
            },
            _ => GhidraQuery::Xrefs {
                address: static_address(args, &run)?,
                direction: match args["direction"].as_str().unwrap_or("both") {
                    "to" => XrefDirection::To,
                    "from" => XrefDirection::From,
                    _ => XrefDirection::Both,
                },
                limit: crate::application::tools::host_usize(number(args, "limit", 100)),
            },
        };
        let executable = self
            .config
            .ghidra_executable
            .clone()
            .or_else(|| std::env::var_os("WUKONG_GHIDRA_HEADLESS").map(PathBuf::from));
        if executable.is_none()
            && let Some((files, gateway)) = self.remote_backend()
        {
            let staged = if secondary.is_some() {
                files.file_put(agent, &format!("{call_id}:target"), &json!({"path":format!("files/{}.bin",run.target_sha256),"evidence_id":args["target_ref"]["evidence_id"],"executable":false})).await?["file"].clone()
            } else {
                files
                    .stage_target(agent, &format!("{call_id}:target"))
                    .await?
            };
            let result=gateway.worker_call(agent, call_id, Request::GhidraQuery(serde_json::from_value(json!({"target":staged["path"],"target_sha256":run.target_sha256,"query":query,"timeout_secs":self.config.timeout_seconds,"analysis_timeout_secs":self.config.analysis_timeout_seconds,"max_cpu":self.config.max_cpu}))?)).await?;
            ensure!(
                result["target_sha256"] == run.target_sha256,
                "remote Ghidra analyzed another target"
            );
            let mut result = result;
            let mut evidence_ids = Vec::new();
            if let Some(artifacts) = result["artifacts"].as_array() {
                for artifact in artifacts {
                    let raw = gateway
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
                    evidence_ids.push(files.import_file(agent, call_id, &raw)?.id);
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
        let executable=executable.context("Ghidra is not configured; set analysis.ghidra_executable, WUKONG_GHIDRA_HEADLESS, or a worker with Ghidra")?;
        let backend = GhidraBackend::new(GhidraConfig {
            executable,
            cache_dir: self.config.ghidra_cache_dir.clone(),
            script_dir: self.config.ghidra_script_dir.clone(),
            timeout_seconds: self.config.timeout_seconds,
            analysis_timeout_seconds: self.config.analysis_timeout_seconds,
            max_cpu: self.config.max_cpu,
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

fn static_address(args: &Value, run: &RunRecord) -> Result<u64> {
    let address: args::StaticAddress = serde_json::from_value(args["address"].clone())
        .context("address requires a target-bound module-relative AddressRef")?;
    address.validate()?;
    ensure!(
        address.module_sha256 == run.target_sha256,
        "address belongs to another module SHA256"
    );
    ensure!(
        address.space == binary::AddressSpace::ModuleRelative,
        "static analysis requires module_relative byte offsets"
    );
    let offset = address.value;
    if let Ok(profile) = binary::inspect(&run.target_path)
        && !binary::contains_module_offset(&run.target_path, profile.image_base, offset)?
    {
        let hint = offset.checked_sub(profile.image_base).filter(|_| profile.image_base > 0)
                .map(|relative| format!(" If {offset:#x} is a link-time VA, its module offset is {relative:#x} (image_base={:#x}); confirm with binary_inspect before retrying.",profile.image_base))
                .unwrap_or_default();
        anyhow::bail!(
            "module-relative byte offset {offset:#x} is outside the target mapped segments.{hint} Retrieve the symbol with functions_search and reuse its target-bound AddressRef; never guess offsets."
        );
    }
    Ok(offset)
}

#[cfg(test)]
mod address_tests {
    use super::*;

    #[test]
    fn static_addresses_preserve_space_hash_and_segment_bounds() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("address.elf");
        let mut bytes = vec![0u8; 0x300];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        for (offset, value) in [(16, 2u16), (18, 62), (52, 64), (54, 56), (56, 1), (58, 64)] {
            bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, value) in [(20, 1u32), (64, 1), (68, 5)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, value) in [
            (24, 0x401080u64),
            (32, 64),
            (80, 0x400000),
            (88, 0x400000),
            (96, 0x300),
            (104, 0x2000),
            (112, 0x1000),
        ] {
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        fs::write(&path, bytes)?;
        let store = Store::open(temp.path().join("state"))?;
        let run = Coordinator::new(store).create_run(
            &path,
            "static fixture",
            Budget::default(),
            json!({}),
        )?;
        let profile = binary::inspect(&path)?;
        assert_eq!(
            static_address(&json!({"address":profile.entry}), &run)?,
            0x1080
        );
        assert!(static_address(&json!({"address":0x1080}), &run).is_err());
        for address in [
            binary::AddressRef::file_offset(&run.target_sha256, 0x1080),
            binary::AddressRef::runtime(&run.target_sha256, 0x401080, "instance", 0x400000)?,
            binary::AddressRef::module_relative(&"a".repeat(64), 0x1080),
        ] {
            assert!(static_address(&json!({"address":address}), &run).is_err());
        }
        assert!(static_address(&json!({}), &run).is_err());
        Ok(())
    }
}

#[cfg(test)]
mod static_backend_tests {
    use super::super::gateway::BoundWorker;
    use super::*;
    use crate::adapters::environments::{EnvironmentConfig, EnvironmentRegistry, Platform};

    #[tokio::test]
    async fn artifacts_use_the_selected_static_fileservice_without_a_primary_worker() -> Result<()>
    {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("state"))?;
        let target = temp.path().join("sample");
        fs::write(&target, b"fixture")?;
        let run = Coordinator::new(store.clone()).create_run(
            &target,
            "static fixture",
            Budget::default(),
            json!({}),
        )?;
        let agent: AgentMetadata = store.load(&run.root_id)?;
        let registry = EnvironmentRegistry::new(std::collections::BTreeMap::from([(
            "static".into(),
            EnvironmentConfig {
                installation_id: None,
                name: "Trusted static fixture".into(),
                platform: Platform::current(),
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
        )]))?;
        let client = registry.client("static").await?;
        let primary = WorkerGateway {
            store: store.clone(),
            worker: None,
            output_limit: 12000,
        };
        let static_gateway = WorkerGateway {
            store: store.clone(),
            worker: Some(BoundWorker::new(client.clone(), "static")?),
            output_limit: 12000,
        };
        let service = AnalysisService {
            store: store.clone(),
            files: FilesService {
                store: store.clone(),
                gateway: primary,
            },
            gateway: WorkerGateway {
                store: store.clone(),
                worker: None,
                output_limit: 12000,
            },
            static_files: Some(FilesService {
                store: store.clone(),
                gateway: static_gateway.clone(),
            }),
            static_gateway: Some(static_gateway.clone()),
            config: Default::default(),
        };
        let bytes = b"static analysis";
        let raw = json!({
            "path":"analysis.json",
            "sha256":binary::sha256_bytes(bytes),
            "size":bytes.len(),
            "data_base64":BASE64.encode(bytes),
        });
        let (files, gateway) = service.remote_backend().unwrap();
        let evidence = files.import_file(&agent, "static-artifact", &raw)?;
        let record = store
            .records::<WorkerFileRecord>(&run.id)?
            .into_iter()
            .find(|file| file.path == "analysis.json")
            .unwrap();
        assert_eq!(record.worker_id, gateway.worker_identity()?);
        assert_eq!(record.evidence_id.as_deref(), Some(evidence.id.as_str()));
        client.call("worker.shutdown", json!({})).await?;
        registry.close().await;
        Ok(())
    }
}
