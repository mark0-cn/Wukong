//! Per-invocation services borrow the controller and carry one explicit worker binding.
use super::*;

pub(crate) struct ToolContext<'service> {
    service: &'service ToolService,
    pub(super) execution: ExecutionService,
    pub(super) analysis: AnalysisService,
}
impl<'service> ToolContext<'service> {
    pub(super) fn new(
        service: &'service ToolService,
        worker: Option<gateway::BoundWorker>,
    ) -> Self {
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
            service,
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
impl std::ops::Deref for ToolContext<'_> {
    type Target = ToolService;
    fn deref(&self) -> &Self::Target {
        self.service
    }
}
