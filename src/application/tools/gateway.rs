//! Stable worker requests and their durable replies; no tool routing or role policy.
use super::*;
use crate::adapters::worker::protocol;

fn stable_worker_identity(identity: &protocol::WorkerIdentity) -> Result<String> {
    Ok(stable(
        "worker",
        &serde_json::to_string(&json!({
            "protocol": identity.protocol,
            "installation_id": identity.installation_id,
            "package_sha256": identity.package_sha256,
        }))?,
    ))
}

fn legacy_worker_identity(identity: &protocol::WorkerIdentity) -> Result<String> {
    Ok(stable("worker", &serde_json::to_string(identity)?))
}

#[derive(Clone)]
pub(super) struct BoundWorker {
    client: RuntimeClient,
    identity: String,
    environment_id: String,
}
impl BoundWorker {
    pub(super) fn new(client: RuntimeClient, environment_id: &str) -> Result<Self> {
        let identity = stable_worker_identity(&client.identity())?;
        Ok(Self {
            client,
            identity,
            environment_id: environment_id.into(),
        })
    }
}
#[derive(Clone)]
pub(super) struct WorkerGateway {
    pub(super) store: Store,
    pub(super) worker: Option<BoundWorker>,
    pub(super) output_limit: usize,
}
impl WorkerGateway {
    pub(super) fn runtime(&self) -> Result<&RuntimeClient> {
        self.worker.as_ref().map(|worker| &worker.client).context(
            "execution runtime is disabled; configure Docker/SSH or explicitly trusted_local",
        )
    }
    fn effects(&self) -> DurableEffects<'_> {
        DurableEffects::new(&self.store, self.output_limit)
    }
    pub(crate) fn worker_identity(&self) -> Result<String> {
        Ok(self
            .worker
            .as_ref()
            .context("worker context is unbound")?
            .identity
            .clone())
    }

    /// Accept stable installation IDs plus legacy boot-bound IDs retained by the
    /// run's binding history. Identity fencing itself happens before this check.
    pub(super) fn accepts_worker_id(&self, run: &str, worker_id: &str) -> Result<bool> {
        let worker = self.worker.as_ref().context("worker context is unbound")?;
        let current = worker.client.identity();
        let binding_id = stable(
            "environment-binding",
            &format!("{run}:{}", worker.environment_id),
        );
        let binding = self.store.get_record::<EnvironmentBinding>(&binding_id)?;
        let mut identities = vec![current];
        if let Some(binding) = binding {
            identities.push(binding.identity);
            identities.extend(binding.prior_identities);
        }
        for identity in identities {
            if stable_worker_identity(&identity)? == worker_id
                || legacy_worker_identity(&identity)? == worker_id
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn request_identity(&self, agent: &AgentMetadata, call_id: &str) -> String {
        DurableEffects::worker_request_id(agent, call_id)
    }

    pub(super) async fn worker_call(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        request: wukong_worker_protocol::Request,
    ) -> Result<Value> {
        let method = request.method().name();
        let mut params = request.parameters()?;
        let request_id = self.request_identity(agent, call_id);
        params["request_id"] = json!(request_id);
        params["call_id"] = json!(request_id);
        if let Some(reply) = self
            .effects()
            .worker_reply(agent, call_id, method, &params)?
        {
            return Ok(reply);
        }
        if request.method().resources() == wukong_worker_protocol::ResourceDemand::Execution {
            let run: RunRecord = self.store.load(&agent.run_id)?;
            if let Some(labs) = run.config.get("windows_lab") {
                let labs: crate::config::WindowsLabConfig = serde_json::from_value(labs.clone())?;
                crate::application::windows_lab::check_execution_admission(
                    self.store.root(),
                    &labs,
                    self.worker
                        .as_ref()
                        .map(|worker| worker.environment_id.as_str()),
                )?;
            }
        }
        let reply = self
            .runtime()?
            .call(method, params.clone())
            .await
            .map_err(|error| {
                use crate::adapters::worker::protocol::{WorkerError, WorkerErrorKind};
                if error.downcast_ref::<WorkerError>().is_some_and(|e| {
                    matches!(
                        e.kind,
                        WorkerErrorKind::InvalidRequest
                            | WorkerErrorKind::Conflict
                            | WorkerErrorKind::SessionLost
                            | WorkerErrorKind::ResourceBusy
                    )
                }) {
                    error
                } else {
                    error.context(crate::application::effects::WorkerDispatch {
                        method: method.into(),
                        request_id: request_id.clone(),
                    })
                }
            })?;
        self.effects()
            .record_worker_reply(agent, call_id, method, params, reply.clone())
            .context(crate::application::effects::WorkerDispatch {
                method: method.into(),
                request_id,
            })?;
        Ok(reply)
    }

    /// Returns the original worker observation. Resource ownership belongs to use cases.
    pub(super) async fn worker_query(
        &self,
        request: wukong_worker_protocol::Request,
    ) -> Result<Value> {
        self.runtime()?
            .call(request.method().name(), request.parameters()?)
            .await
    }
}
