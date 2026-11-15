//! Authoritative Windows worker: public routing and resource observation.
use crate::host::context;
use crate::{
    artifacts::inputs::Inputs,
    host::deployment,
    isolation::security::AclPolicy,
    resources::manager::ResourceManager,
    resources::state::ResourceState,
    rpc::worker_leases::{Leases, require},
};
use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;
pub use wukong_worker_protocol::WorkerIdentity as Identity;

pub(crate) struct Worker {
    identity: Identity,
    daemon_id: String,
    leases: Leases,
    inputs: std::sync::Arc<tokio::sync::Mutex<Inputs>>,
    resources: ResourceManager,
    qualified: std::sync::atomic::AtomicBool,
}
impl Worker {
    pub fn open(identity: Identity, metadata: &Path) -> Result<Self> {
        Self::with_policy(identity, metadata, AclPolicy::private_vm())
    }
    fn with_policy(identity: Identity, metadata: &Path, policy: AclPolicy) -> Result<Self> {
        Ok(Self {
            identity: identity.clone(),
            daemon_id: hex::encode(crate::host::accounts::random_bytes::<16>()?),
            leases: Leases::open(identity.clone(), metadata, policy.clone())?,
            resources: ResourceManager::open(metadata, identity, policy.clone())?,
            inputs: std::sync::Arc::new(tokio::sync::Mutex::new(Inputs::open(metadata, policy)?)),
            qualified: std::sync::atomic::AtomicBool::new(false),
        })
    }
    pub(crate) fn open_resources(
        &self,
        root: &Path,
        deployment: &deployment::Deployment,
        manifest: &wukong_worker_package::VerifiedManifest,
    ) -> Result<()> {
        self.resources.initialize(context::RuntimeContext::verified(
            root, deployment, manifest,
        )?)
    }
    pub(crate) fn poll_resources(&self) -> Result<bool> {
        self.resources.poll(self.leases.expired())
    }
    pub(crate) async fn cancel_resources(&self) -> Result<()> {
        self.resources.cancel().await
    }
    fn resources_idle(&self) -> Result<bool> {
        self.resources.idle()
    }
    pub(crate) async fn dispatch(
        &self,
        operation: wukong_worker_protocol::Method,
        params: Value,
    ) -> Result<Value> {
        let method = operation.name().to_owned();
        if matches!(method.as_str(), "ping" | "doctor") {
            let result = if method == "ping" {
                json!({"protocol":self.identity.protocol,"identity":self.identity,
                    "daemon_id":self.daemon_id,"daemon_pid":std::process::id()})
            } else {
                crate::rpc::catalog::doctor(
                    &self.identity,
                    self.qualified.load(std::sync::atomic::Ordering::Acquire),
                )
            };
            return Ok(result);
        }
        if !params.is_object() {
            anyhow::bail!("native RPC parameters must be an object");
        }
        if method == "environment.lease.acquire" {
            let lease = self
                .leases
                .acquire(params.as_object().unwrap(), || self.resources_idle())?;
            return Ok(serde_json::to_value(lease)?);
        }
        if method == "environment.lease.release" {
            let epoch = self
                .leases
                .release(params.as_object().unwrap(), || self.resources_idle())?;
            return Ok(json!({"released":true,"epoch":epoch}));
        }
        require(
            crate::rpc::catalog::route(&method).is_some(),
            wukong_worker_protocol::WorkerErrorKind::InvalidRequest,
            "unsupported Windows worker method",
        )?;
        let outer_lease = params["_lease"].clone();
        let admission = self.leases.admit(params.as_object().unwrap())?;
        let native_resource = ResourceState::handles(&method, params.as_object().unwrap())
            && self.resources.initialized().await;
        // Serialize input publication independently of lease controls: hashing
        // or a slow worker upload must never prevent heartbeat renewal.
        let inputs = if Inputs::handles(&method)
            || crate::rpc::catalog::route(&method) == Some(crate::rpc::catalog::Route::Analysis)
            || native_resource
                && operation.resources() == wukong_worker_protocol::ResourceDemand::Execution
        {
            Some(self.inputs.clone().lock_owned().await)
        } else {
            None
        };
        if native_resource {
            return self
                .resources
                .execute(
                    inputs,
                    &admission.owner,
                    &method,
                    params.as_object().unwrap(),
                    || self.leases.validate(&outer_lease),
                )
                .await;
        }

        if let Some(inputs) = &inputs {
            self.leases.validate(&outer_lease)?;
            inputs.call(
                admission.owner.clone(),
                &method,
                params.as_object().unwrap(),
            )
        } else {
            Err(anyhow::anyhow!(
                "Windows execution runtime is not initialized"
            ))
        }
    }
    pub(crate) fn qualify(&self) {
        self.qualified
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
