//! Authoritative Windows worker: public routing and resource-layer handles.
//!
//! The host builds the verified runtime context and the shared resource
//! manager before service readiness; this entry only holds handles to the
//! resource layer for admission, dispatch, status queries and cancellation.
use crate::{
    artifacts::inputs::Inputs,
    isolation::security::AclPolicy,
    resources::manager::ResourceManager,
    rpc::worker_leases::{Leases, require},
};
use anyhow::Result;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
pub use wukong_worker_protocol::WorkerIdentity as Identity;

pub(crate) struct Worker {
    identity: Identity,
    daemon_id: String,
    leases: Leases,
    inputs: Arc<tokio::sync::Mutex<Inputs>>,
    resources: Arc<ResourceManager>,
    qualified: AtomicBool,
}
impl Worker {
    pub(crate) fn open(
        identity: Identity,
        metadata: &Path,
        resources: Arc<ResourceManager>,
    ) -> Result<Self> {
        Self::with_policy(identity, metadata, AclPolicy::private_vm(), resources)
    }
    fn with_policy(
        identity: Identity,
        metadata: &Path,
        policy: AclPolicy,
        resources: Arc<ResourceManager>,
    ) -> Result<Self> {
        Ok(Self {
            identity: identity.clone(),
            daemon_id: hex::encode(crate::host::accounts::random_bytes::<16>()?),
            leases: Leases::open(identity.clone(), metadata, policy.clone())?,
            resources,
            inputs: Arc::new(tokio::sync::Mutex::new(Inputs::open(metadata, policy)?)),
            qualified: AtomicBool::new(false),
        })
    }
    /// The host's keep-alive loop owns the run fence input for resource
    /// cancellation; lease state itself never lives in the resource layer.
    pub(crate) fn lease_expired(&self) -> bool {
        self.leases.expired()
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
                crate::rpc::catalog::doctor(&self.identity, self.qualified.load(Ordering::Acquire))
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
        let native_resource =
            crate::rpc::catalog::owns_resource(&method, params.as_object().unwrap())
                && self.resources.initialized().await;
        // Serialize input publication independently of lease controls: hashing
        // or a slow worker upload must never prevent heartbeat renewal.
        let inputs = if crate::rpc::catalog::route(&method)
            == Some(crate::rpc::catalog::Route::Files)
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
                    crate::rpc::catalog::resource_operation(
                        operation,
                        params.as_object().unwrap(),
                    )?,
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
        self.qualified.store(true, Ordering::Release);
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
