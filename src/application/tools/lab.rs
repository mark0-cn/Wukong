//! Controller-side Windows lab service shared by tools.
use super::*;
use crate::adapters::environments::EnvironmentRegistry;
use crate::application::windows_lab::WindowsLab;
use crate::config::{AppConfig, WindowsLabConfig};

pub(crate) type LabFactory = fn(&AppConfig, &EnvironmentRegistry, &str) -> Result<WindowsLab>;

#[derive(Clone)]
pub(crate) struct LabService {
    pub(super) operations: lab_operations::Operations,
    pub(super) open_lab: LabFactory,
    pub(super) output_limit: usize,
    pub(super) store: Store,
    pub(super) execution: ExecutionService,
    pub(super) config: WindowsLabConfig,
    pub(super) state_dir: PathBuf,
    pub(super) environments: EnvironmentRegistry,
}

impl LabService {
    pub(super) fn owner(&self, agent: &AgentMetadata) -> String {
        stable(
            "lab-owner",
            &format!("{}:{}:{}", agent.run_id, agent.id, agent.generation),
        )
    }

    pub(super) fn assessment_worker(&self, agent: &AgentMetadata, call: &str) -> AssessmentWorker {
        AssessmentWorker {
            execution: self.execution.clone(),
            environments: self.environments.clone(),
            agent: agent.clone(),
            call: call.into(),
        }
    }

    fn evidence_bytes(&self, agent: &AgentMetadata, id: &str, limit: u64) -> Result<Vec<u8>> {
        let evidence: Evidence = require_in_run(&self.store, id, &agent.run_id)?;
        ensure!(evidence.size <= limit, "lab evidence exceeds size budget");
        self.store.read_evidence_range(
            &agent.run_id,
            id,
            0,
            usize::try_from(evidence.size)
                .map_err(|_| anyhow::anyhow!("lab evidence exceeds the address space"))?,
        )
    }

    pub(super) fn materialize_spec(
        &self,
        agent: &AgentMetadata,
        input: args::AgentExperimentSpec,
    ) -> Result<crate::application::windows_lab::ExperimentSpec> {
        use crate::application::windows_lab::{
            ExperimentSpec,
            package::{Dependency, DependencySource},
        };
        let run: RunRecord = self.store.load(&agent.run_id)?;
        let target_sha256 = match input.target_evidence_id {
            Some(id) => {
                let bytes = self.evidence_bytes(
                    agent,
                    &id,
                    crate::application::windows_lab::package::MAX_ARTIFACT_BYTES,
                )?;
                let sha256 = hex::encode(Sha256::digest(&bytes));
                ensure!(
                    sha256 == run.target_sha256,
                    "target evidence {id} has SHA256 {sha256}, but the assessment entry is {}; use bundle_evidence for the entry or omit target_evidence_id",
                    run.target_sha256
                );
                sha256
            }
            None => run.target_sha256,
        };
        ensure!(
            input.dependencies.len() <= 256,
            "lab package supports at most 256 dependencies"
        );
        let mut total = 0_u64;
        let directory = self.state_dir.join("windows-lab/evidence-inputs");
        fs::create_dir_all(&directory)?;
        let mut dependencies = Vec::new();
        for dependency in input.dependencies {
            let bytes = self.evidence_bytes(
                agent,
                &dependency.evidence_id,
                crate::application::windows_lab::package::MAX_ARTIFACT_BYTES,
            )?;
            total += bytes.len() as u64;
            ensure!(
                total <= crate::application::windows_lab::package::MAX_PACKAGE_BYTES,
                "lab package exceeds size budget"
            );
            ensure!(
                bytes.len() as u64 <= crate::application::windows_lab::package::MAX_ARTIFACT_BYTES,
                "lab dependency exceeds size budget"
            );
            let sha256 = hex::encode(Sha256::digest(&bytes));
            let path = directory.join(&sha256);
            if path.exists() {
                ensure!(
                    crate::application::windows_lab::package::read_bounded(
                        &path,
                        crate::application::windows_lab::package::MAX_ARTIFACT_BYTES
                    )? == bytes,
                    "existing evidence copy differs"
                );
            } else {
                use std::io::Write;
                let mut file = tempfile::NamedTempFile::new_in(&directory)?;
                file.write_all(&bytes)?;
                match file.persist_noclobber(&path) {
                    Ok(_) => {}
                    // Another operation published the same content-addressed evidence
                    // first; identical bytes are equivalent, anything else is a bug.
                    Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                        ensure!(
                            crate::application::windows_lab::package::read_bounded(
                                &path,
                                crate::application::windows_lab::package::MAX_ARTIFACT_BYTES
                            )? == bytes,
                            "existing evidence copy differs"
                        );
                    }
                    Err(error) => return Err(error.error.into()),
                }
            }
            dependencies.push(Dependency {
                name: dependency.name,
                sha256,
                source: DependencySource::Path { path },
            });
        }
        let spec = ExperimentSpec {
            target_sha256,
            launch: input.launch,
            input_channel: input.input_channel,
            account: input.account,
            annotations: input.annotations,
            dependencies,
            install: input.install,
            recovery: String::new(),
        };
        spec.validate()?;
        Ok(spec)
    }

    pub(super) async fn run(
        &self,
        agent: &AgentMetadata,
        call: &str,
        input: args::LabRunArgs,
    ) -> Result<Value> {
        let reference = self.resolve_reference(agent, &input.lab_ref)?;
        let stdin = input
            .stdin_evidence_id
            .map(|id| self.evidence_bytes(agent, &id, 1024 * 1024))
            .transpose()?;
        let worker = self.assessment_worker(agent, call);
        let lab = self
            .open(&reference.vm, agent)?
            .with_worker(Arc::new(worker));
        lab.run(
            &reference.lab_id,
            reference.generation,
            &input.artifact,
            stdin,
            input.timeout_secs.unwrap_or(120),
        )
        .await
    }

    pub(super) fn list(&self) -> Vec<Value> {
        self.config
            .vms
            .iter()
            .map(|(id, vm)| {
                json!({
                    "vm": id,
                    "environment": vm.environment,
                    "baseline_snapshot": crate::application::windows_lab::BASELINE_SNAPSHOT,
                    "snapshot_prefix": vm.snapshot_prefix,
                    "start_after_restore": vm.start_after_restore,
                })
            })
            .collect()
    }

    pub(super) fn open(&self, vm_id: &str, agent: &AgentMetadata) -> Result<WindowsLab> {
        let available = self
            .config
            .vms
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        ensure!(
            self.config.vms.contains_key(vm_id),
            "windows_lab VM {vm_id} is not registered; configured VMs: {available}"
        );
        let vm = &self.config.vms[vm_id];
        ensure!(
            agent.environment_id == vm.environment && vm.environment.is_some(),
            "lab VM must match the agent's assigned environment"
        );
        let run: RunRecord = self.store.load(&agent.run_id)?;
        ensure!(
            run.config["windows_lab"]["vms"][vm_id] == serde_json::to_value(vm)?,
            "lab configuration changed since assessment creation"
        );
        let environment = vm
            .environment
            .as_deref()
            .context("lab environment missing")?;
        ensure!(
            run.config["environments"][environment]
                == serde_json::to_value(self.environments.config(environment)?)?,
            "lab worker configuration changed since assessment creation"
        );
        let config = AppConfig {
            state_dir: self.state_dir.clone(),
            windows_lab: self.config.clone(),
            ..Default::default()
        };
        (self.open_lab)(&config, &self.environments, vm_id)
            .map(|lab| lab.with_caller(self.owner(agent)))
    }
}

/// Lab execution participates in the same durable job ledger, heartbeat,
/// evidence import, cancellation and completion gates as ordinary jobs.
pub(super) struct AssessmentWorker {
    execution: ExecutionService,
    environments: EnvironmentRegistry,
    agent: AgentMetadata,
    call: String,
}
impl AssessmentWorker {
    fn outcome(
        &self,
        value: Value,
        executable: Option<String>,
    ) -> Result<crate::application::windows_lab::runner::LabWorkerOutcome> {
        let status: JobStatus = serde_json::from_value(value.clone())?;
        Ok(crate::application::windows_lab::runner::LabWorkerOutcome {
            environment_id: self
                .agent
                .environment_id
                .clone()
                .context("missing environment")?,
            installation_id: self
                .execution
                .gateway
                .runtime()?
                .identity()
                .installation_id
                .clone(),
            job_id: text(&value, "job_id")?.to_owned(),
            state: value["state"].as_str().unwrap_or("unknown").into(),
            settled: status.settled(),
            exit_code: value["result"]["exit_code"].as_i64(),
            executable_path: executable,
            stdout: value["stdout"].as_str().unwrap_or_default().into(),
            stderr: value["stderr"].as_str().unwrap_or_default().into(),
            stdout_bytes: value["stdout_bytes"].as_u64().unwrap_or_default(),
            stderr_bytes: value["stderr_bytes"].as_u64().unwrap_or_default(),
        })
    }
}
#[async_trait::async_trait]
impl crate::application::windows_lab::runner::LabWorker for AssessmentWorker {
    async fn doctor(&self) -> Result<Value> {
        let environment = self
            .agent
            .environment_id
            .as_deref()
            .context("missing environment")?;
        self.environments.doctor(environment).await
    }
    async fn execute(
        &self,
        request: crate::application::windows_lab::runner::LabWorkerRequest,
    ) -> Result<crate::application::windows_lab::runner::LabWorkerOutcome> {
        let digest = hex::encode(Sha256::digest(&request.executable));
        let upload = self.execution.files.file_put(&self.agent, &format!("{}:executable", self.call), &json!({"path":format!("lab/{digest}/{}",request.artifact_name),"data_base64":BASE64.encode(&request.executable),"executable":true})).await?;
        let path = text(&upload["file"], "path")?.to_owned();
        let mut args =
            json!({"argv":[path],"timeout_secs":request.timeout_secs,"output_limit":1024*1024});
        if let Some(stdin) = request.stdin {
            let digest = hex::encode(Sha256::digest(&stdin));
            let uploaded = self.execution.files.file_put(&self.agent, &format!("{}:stdin",self.call), &json!({"path":format!("lab/{digest}/stdin.bin"),"data_base64":BASE64.encode(stdin)})).await?;
            args["input"] = uploaded["file"]["path"].clone();
        }
        let value = self
            .execution
            .runtime_tool(&self.agent, &self.call, "job_start", args)
            .await?;
        self.outcome(value, Some(path))
    }
    async fn observe(
        &self,
        job_id: &str,
    ) -> Result<Option<crate::application::windows_lab::runner::LabWorkerOutcome>> {
        let value = self
            .execution
            .jobs
            .observe(&self.agent, &format!("{}:observe", self.call), job_id)
            .await?;
        self.outcome(value, None).map(Some)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct LabReference {
    pub vm: String,
    pub lab_id: String,
    pub generation: u64,
    run_id: String,
    agent_id: String,
    agent_generation: u64,
}
impl LabService {
    pub(super) fn authorize_vm(&self, agent: &AgentMetadata, vm: &str) -> Result<()> {
        let config = self.config.vms.get(vm).context("VM is not registered")?;
        ensure!(
            agent.environment_id.is_some() && agent.environment_id == config.environment,
            "lab VM is not assigned to this task environment"
        );
        Ok(())
    }
    pub(super) fn bind_reference(
        &self,
        agent: &AgentMetadata,
        vm: &str,
        lab_id: &str,
        generation: u64,
    ) -> Result<String> {
        self.authorize_vm(agent, vm)?;
        let id = stable(
            "lab-ref",
            &format!(
                "{}:{}:{vm}:{lab_id}:{generation}",
                agent.run_id,
                self.owner(agent)
            ),
        );
        self.store.put(
            "lab_reference",
            &id,
            &agent.run_id,
            &LabReference {
                vm: vm.into(),
                lab_id: lab_id.into(),
                generation,
                run_id: agent.run_id.clone(),
                agent_id: agent.id.clone(),
                agent_generation: agent.generation,
            },
        )?;
        Ok(id)
    }
    pub(super) fn resolve_reference(
        &self,
        agent: &AgentMetadata,
        id: &str,
    ) -> Result<LabReference> {
        let reference: LabReference = self
            .store
            .get("lab_reference", id)?
            .context("unknown lab_ref; use the reference returned by lab_prepare")?;
        ensure!(
            reference.run_id == agent.run_id
                && reference.agent_id == agent.id
                && reference.agent_generation == agent.generation,
            "lab_ref belongs to another run or agent"
        );
        self.authorize_vm(agent, &reference.vm)?;
        Ok(reference)
    }
}

#[cfg(test)]
mod reference_tests {
    use super::*;
    #[tokio::test]
    async fn references_are_bound_to_run_agent_generation_and_environment() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let target = temporary.path().join("target");
        fs::write(&target, b"trusted fixture")?;
        let mut config = AppConfig::default();
        config.environments.insert(
            "windows".into(),
            crate::adapters::environments::EnvironmentConfig {
                name: "Reference fixture".into(),
                platform: crate::adapters::environments::Platform::Windows,
                installation_id: None,
                runtime: crate::adapters::worker::RuntimeConfig {
                    transport: crate::adapters::worker::TransportConfig::Ssh(
                        crate::adapters::worker::SshConfig::new("unused-fixture".into()),
                    ),
                    ..Default::default()
                },
            },
        );
        config.default_environment = Some("windows".into());
        config.windows_lab.vms.insert(
            "fixture".into(),
            crate::config::WindowsLabVm {
                environment: Some("windows".into()),
                vmx: temporary.path().join("fixture.vmx"),
                vmrun: temporary.path().join("vmrun.exe"),
                ..Default::default()
            },
        );
        let store = Store::open(temporary.path().join("state"))?;
        let run = Coordinator::new(store.clone()).create_run(
            &target,
            "reference test",
            Budget::default(),
            serde_json::to_value(&config)?,
        )?;
        let mut agent: AgentMetadata = store.load(&run.root_id)?;
        agent.environment_id = Some("windows".into());
        let tools = ToolService::new(config, store).await?;
        let service = ToolContext::new(&tools, None).lab;
        let reference = service.bind_reference(&agent, "fixture", "lease", 4)?;
        assert_eq!(service.resolve_reference(&agent, &reference)?.generation, 4);
        for changed in 0..4 {
            let mut actor = agent.clone();
            match changed {
                0 => actor.run_id = "foreign-run".into(),
                1 => actor.id = "foreign-agent".into(),
                2 => actor.generation += 1,
                3 => actor.environment_id = Some("other".into()),
                _ => unreachable!(),
            }
            assert!(service.resolve_reference(&actor, &reference).is_err());
        }
        tools.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn prepare_target_evidence_must_match_the_assessment_entry() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let target = temporary.path().join("target.exe");
        fs::write(&target, b"original executable")?;
        let config = AppConfig::default();
        let store = Store::open(temporary.path().join("state"))?;
        let run = Coordinator::new(store.clone()).create_run(
            &target,
            "target evidence test",
            Budget::default(),
            serde_json::to_value(&config)?,
        )?;
        let agent: AgentMetadata = store.load(&run.root_id)?;
        let wrong = store.artifact(
            &run.id,
            &agent.id,
            "inspect-call",
            "binary_inspect.full",
            br#"{"format":"pe"}"#,
            "application/json",
            json!({}),
        )?;
        let original = store.artifact(
            &run.id,
            &agent.id,
            "bundle-call",
            "bundle.member",
            b"original executable",
            "application/octet-stream",
            json!({"path":"target.exe"}),
        )?;
        let tools = ToolService::new(config, store).await?;
        let service = ToolContext::new(&tools, None).lab;
        let spec = |evidence_id: &str| {
            serde_json::from_value::<args::AgentExperimentSpec>(json!({
                "target_evidence_id": evidence_id,
                "launch": "exe",
                "input_channel": "stdin",
                "account": "user",
                "dependencies": []
            }))
            .unwrap()
        };
        let error = service
            .materialize_spec(&agent, spec(&wrong.id))
            .unwrap_err();
        assert!(format!("{error:#}").contains("assessment entry"));
        let accepted = service.materialize_spec(&agent, spec(&original.id))?;
        assert_eq!(accepted.target_sha256, run.target_sha256);
        tools.close().await;
        Ok(())
    }
}
