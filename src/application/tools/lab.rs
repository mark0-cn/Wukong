//! Controller-side Windows lab service shared by tools.
use super::*;
use crate::adapters::environments::EnvironmentRegistry;
use crate::application::windows_lab::WindowsLab;
use crate::config::{AppConfig, WindowsLabConfig};

#[derive(Clone)]
pub(crate) struct LabService {
    pub(super) operations: lab_operations::Operations,
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

    fn evidence_bytes(&self, agent: &AgentMetadata, id: &str, limit: u64) -> Result<Vec<u8>> {
        let evidence: Evidence = require_in_run(&self.store, id, &agent.run_id)?;
        ensure!(evidence.size <= limit, "lab evidence exceeds size budget");
        self.store
            .read_evidence_range(&agent.run_id, id, 0, evidence.size as usize)
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
            Some(id) => hex::encode(Sha256::digest(self.evidence_bytes(
                agent,
                &id,
                crate::application::windows_lab::package::MAX_ARTIFACT_BYTES,
            )?)),
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
                file.persist_noclobber(&path).map_err(|e| e.error)?;
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
        let stdin = input
            .stdin_evidence_id
            .map(|id| self.evidence_bytes(agent, &id, 1024 * 1024))
            .transpose()?;
        let worker = AssessmentWorker {
            execution: self.execution.clone(),
            agent: agent.clone(),
            call: call.into(),
        };
        let lab = self.open(&input.vm, agent)?.with_worker(Arc::new(worker));
        lab.run(
            &input.lab_id,
            input.generation,
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
                    "baseline_snapshot": vm.baseline_snapshot,
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
        crate::application::windows_lab::service::open_configured(
            &config,
            &self.environments,
            vm_id,
        )
        .map(|lab| lab.with_caller(self.owner(agent)))
    }
}

/// Lab execution participates in the same durable job ledger, heartbeat,
/// evidence import, cancellation and completion gates as ordinary jobs.
struct AssessmentWorker {
    execution: ExecutionService,
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
