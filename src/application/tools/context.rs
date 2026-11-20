//! Per-invocation services borrow the controller and carry one explicit worker binding.
use super::*;

pub(crate) struct ToolContext {
    pub(super) coordination: coordination::CoordinationService,
    pub(super) documents: documents::DocumentService,
    pub(super) evidence: evidence::EvidenceService,
    pub(super) environments: environment_service::EnvironmentService,
    pub(super) local_ghidra: Option<PathBuf>,
    pub(super) static_gateway: Option<WorkerGateway>,
    pub(super) execution: ExecutionService,
    pub(super) analysis: AnalysisService,
    pub(super) lab: LabService,
    pub(super) bundle: bundle::BundleService,
}
impl ToolContext {
    pub(super) fn new(service: &ToolService, worker: Option<gateway::BoundWorker>) -> Self {
        Self::with_static(service, worker, None)
    }

    pub(super) fn with_static(
        service: &ToolService,
        worker: Option<gateway::BoundWorker>,
        static_worker: Option<gateway::BoundWorker>,
    ) -> Self {
        let gateway = WorkerGateway {
            store: service.store.clone(),
            worker,
            output_limit: service.limits.max_output_bytes,
        };
        let static_gateway = static_worker.map(|worker| WorkerGateway {
            store: service.store.clone(),
            worker: Some(worker),
            output_limit: service.limits.max_output_bytes,
        });
        let execution = ExecutionService::new(
            service.store.clone(),
            gateway.clone(),
            service.locks.clone(),
            service.sync_limits.clone(),
        );
        let analysis = AnalysisService {
            store: service.store.clone(),
            files: execution.files.clone(),
            gateway: gateway.clone(),
            static_files: static_gateway.as_ref().map(|gateway| FilesService {
                store: service.store.clone(),
                gateway: gateway.clone(),
            }),
            static_gateway: static_gateway.clone(),
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
            static_gateway,
            bundle: bundle::BundleService {
                store: service.store.clone(),
            },
            execution: execution.clone(),
            analysis,
            lab: LabService {
                operations: service.lab_operations.clone(),
                open_lab: service.open_lab,
                output_limit: service.limits.max_output_bytes,
                store: service.store.clone(),
                execution,
                config: service.windows_lab.clone(),
                state_dir: service.store.root().to_path_buf(),
                environments: service.environments.clone(),
            },
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
