//! Stable worker requests and their durable replies; no tool routing or role policy.
use super::*;
#[derive(Clone)]
pub(super) struct WorkerGateway {
    pub(super) store: Store,
    pub(super) client: Option<RuntimeClient>,
    pub(super) identity: String,
    pub(super) output_limit: usize,
}
impl WorkerGateway {
    pub(super) fn runtime(&self) -> Result<&RuntimeClient> {
        self.client.as_ref().context(
            "execution runtime is disabled; configure Docker/SSH or explicitly trusted_local",
        )
    }
    fn effects(&self) -> DurableEffects<'_> {
        DurableEffects::new(&self.store, self.output_limit)
    }
    pub(crate) fn worker_identity(&self) -> Result<String> {
        Ok(self.identity.clone())
    }

    pub(super) fn request_identity(&self, agent: &AgentMetadata, call_id: &str) -> String {
        DurableEffects::worker_request_id(agent, call_id)
    }

    pub(super) async fn worker_call(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        method: &str,
        mut params: Value,
    ) -> Result<Value> {
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
}
