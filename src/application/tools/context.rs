//! Per-invocation services borrow the controller and carry one explicit worker binding.
use super::*;

pub(crate) struct ToolContext {
    pub(super) coordination: coordination::CoordinationService,
    pub(super) documents: documents::DocumentService,
    pub(super) evidence: evidence::EvidenceService,
    pub(super) environments: environment_service::EnvironmentService,
    pub(super) local_ghidra: Option<PathBuf>,
    pub(super) execution: ExecutionService,
    pub(super) analysis: AnalysisService,
}
impl ToolContext {
    pub(super) fn new(service: &ToolService, worker: Option<gateway::BoundWorker>) -> Self {
        let gateway = WorkerGateway {
            store: service.store.clone(),
            worker,
            output_limit: service.limits.max_output_bytes,
        };
        let execution = ExecutionService::new(
            service.store.clone(),
            gateway.clone(),
            service.locks.clone(),
        );
        let analysis = AnalysisService {
            store: service.store.clone(),
            files: execution.files.clone(),
            gateway,
            config: service.analysis_config.clone(),
        };
        Self {
            coordination: service.coordination_service(),
            documents: documents::DocumentService {
                store: service.store.clone(),
                output_limit: service.limits.max_output_bytes,
            },
            evidence: evidence::EvidenceService {
                store: service.store.clone(),
                output_limit: service.limits.max_output_bytes,
                read_limit: service.limits.max_evidence_read_bytes,
            },
            environments: environment_service::EnvironmentService {
                store: service.store.clone(),
                registry: service.environments.clone(),
            },
            local_ghidra: service.local_ghidra(),
            execution,
            analysis,
        }
    }
    #[cfg(test)]
    pub(crate) fn jobs(&self) -> &jobs::JobService {
        &self.execution.jobs
    }
    #[cfg(test)]
    pub(crate) fn worker_identity(&self) -> Result<String> {
        self.execution.gateway.worker_identity()
    }
    #[cfg(test)]
    pub(crate) fn runtime(&self) -> Result<&RuntimeClient> {
        self.execution.gateway.runtime()
    }
}
