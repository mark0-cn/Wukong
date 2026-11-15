//! Stable worker requests and their durable replies; no tool routing or role policy.
use super::*;
#[derive(Clone)]
pub(super) struct BoundWorker {
    client: RuntimeClient,
    identity: String,
}
impl BoundWorker {
    pub(super) fn new(client: RuntimeClient) -> Result<Self> {
        let identity = stable("worker", &serde_json::to_string(client.identity())?);
        Ok(Self { client, identity })
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
        let reply = self.runtime()?.call(method, params.clone()).await.context(
            crate::effects::WorkerDispatch {
                method: method.into(),
                request_id: request_id.clone(),
            },
        )?;
        self.effects()
            .record_worker_reply(agent, call_id, method, params, reply.clone())
            .context(crate::effects::WorkerDispatch {
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
