//! Authoritative Windows worker: public routing and resource observation.
use crate::{
    deployment,
    inputs::Inputs,
    resource_journal::Journal,
    resource_runtime::{Dispatch, Runtime},
    security::AclPolicy,
    worker_leases::{Leases, Rejected, require},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
pub use wukong_worker_protocol::WorkerIdentity as Identity;

struct Resources {
    journal: Journal,
    runtime: Option<Runtime>,
}
pub(crate) struct Worker {
    identity: Identity,
    daemon_id: String,
    leases: Leases,
    inputs: tokio::sync::Mutex<Inputs>,
    resources: tokio::sync::Mutex<Resources>,
    qualified: std::sync::atomic::AtomicBool,
}
impl Worker {
    pub fn open(identity: Identity, metadata: &Path) -> Result<Self> {
        Self::with_policy(identity, metadata, AclPolicy::private_vm())
    }
    fn with_policy(identity: Identity, metadata: &Path, policy: AclPolicy) -> Result<Self> {
        Ok(Self {
            identity: identity.clone(),
            daemon_id: hex::encode(crate::accounts::random_bytes::<16>()?),
            leases: Leases::open(identity.clone(), metadata, policy.clone())?,
            resources: tokio::sync::Mutex::new(Resources {
                journal: Journal::with_policy(metadata, identity, policy.clone())?,
                runtime: None,
            }),
            inputs: tokio::sync::Mutex::new(Inputs::open(metadata, policy)?),
            qualified: std::sync::atomic::AtomicBool::new(false),
        })
    }
    pub(crate) fn open_resources(
        &self,
        root: &Path,
        deployment: &deployment::Deployment,
        manifest: &wukong_worker_package::VerifiedManifest,
    ) -> Result<()> {
        let mut resources = self
            .resources
            .try_lock()
            .context("native resource runtime is busy during initialization")?;
        ensure!(
            resources.runtime.is_none(),
            "native resource runtime is already open"
        );
        resources.runtime = Some(Runtime::open(root, deployment, manifest)?);
        Ok(())
    }
    pub(crate) fn poll_resources(&self) -> Result<bool> {
        // A busy resource is not idle. The SCM loop must keep serving lease
        // renewals and stop notifications instead of waiting on another RPC.
        let Ok(mut resources) = self.resources.try_lock() else {
            return Ok(false);
        };
        let expired = self.leases.expired();
        let Resources { journal, runtime } = &mut *resources;
        if let Some(runtime) = runtime {
            if expired {
                runtime.cancel_active()?;
            }
            runtime.poll(journal)?;
        }
        journal.idle()
    }
    pub(crate) async fn cancel_resources(&self) -> Result<()> {
        if let Some(runtime) = &mut self.resources.lock().await.runtime {
            runtime.cancel_active()?;
        }
        Ok(())
    }
    // Called while holding the lease state. Never wait for resource work here:
    // that work may need to recheck its admission after acquiring this lock.
    // A busy resource host cannot establish the idle barrier for a transition.
    fn resources_idle(&self) -> Result<bool> {
        match self.resources.try_lock() {
            Ok(resources) => resources.journal.idle(),
            Err(_) => Ok(false),
        }
    }
    pub async fn call(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        let mut request: Value = serde_json::from_slice(bytes)?;
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        match self.dispatch(&mut request).await {
            Ok(reply) => Ok(reply),
            Err(error) => {
                let kind = error.downcast_ref::<Rejected>().map(|e| e.kind).unwrap_or(
                    if error.is::<wukong_worker_protocol::CapacityBusy>() {
                        wukong_worker_protocol::WorkerErrorKind::ResourceBusy
                    } else if error.is::<serde_json::Error>() {
                        wukong_worker_protocol::WorkerErrorKind::InvalidRequest
                    } else {
                        wukong_worker_protocol::WorkerErrorKind::Internal
                    },
                );
                encode(&json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,
                "message":format!("{error:#}").chars().take(2048).collect::<String>(),
                "data":{"kind":kind}}}))
            }
        }
    }
    async fn dispatch(&self, request: &mut Value) -> Result<Vec<u8>> {
        ensure!(
            request["jsonrpc"] == "2.0" && request.is_object(),
            "invalid native RPC envelope"
        );
        let method = request["method"]
            .as_str()
            .context("missing native RPC method")?
            .to_owned();
        ensure!(
            !request.as_object().unwrap().contains_key("auth"),
            "authentication is service-owned"
        );
        if matches!(method.as_str(), "ping" | "doctor") {
            let result = if method == "ping" {
                json!({"protocol":self.identity.protocol,"identity":self.identity,
                    "daemon_id":self.daemon_id,"daemon_pid":std::process::id()})
            } else {
                crate::worker_catalog::doctor(
                    &self.identity,
                    self.qualified.load(std::sync::atomic::Ordering::Acquire),
                )
            };
            return encode(&json!({"jsonrpc":"2.0","id":request["id"],"result":result}));
        }
        if !request.get("params").is_some_and(Value::is_object) {
            anyhow::bail!("native RPC parameters must be an object");
        }
        if method == "environment.lease.acquire" {
            let lease = self
                .leases
                .acquire(request["params"].as_object().unwrap(), || {
                    self.resources_idle()
                })?;
            return encode(&json!({"jsonrpc":"2.0","id":request["id"],"result":lease}));
        }
        if method == "environment.lease.release" {
            let epoch = self
                .leases
                .release(request["params"].as_object().unwrap(), || {
                    self.resources_idle()
                })?;
            return encode(
                &json!({"jsonrpc":"2.0","id":request["id"],"result":{"released":true,"epoch":epoch}}),
            );
        }
        require(
            crate::worker_catalog::route(&method).is_some(),
            wukong_worker_protocol::WorkerErrorKind::InvalidRequest,
            "unsupported Windows worker method",
        )?;
        let outer_lease = request["params"]["_lease"].clone();
        let admission = self.leases.admit(request["params"].as_object().unwrap())?;
        let native_resource = Runtime::handles(&method, request["params"].as_object().unwrap())
            && self.resources.lock().await.runtime.is_some();
        // Serialize input publication independently of lease controls: hashing
        // or a slow worker upload must never prevent heartbeat renewal.
        let inputs = if Inputs::handles(&method)
            || crate::worker_catalog::route(&method) == Some(crate::worker_catalog::Route::Analysis)
            || native_resource
                && matches!(
                    method.as_str(),
                    "job.start"
                        | "debug.start"
                        | "interact.start"
                        | "exec.run"
                        | "py.run"
                        | "crash.replay"
                        | "input.minimize"
                        | "poc.verify"
                ) {
            Some(self.inputs.lock().await)
        } else {
            None
        };
        if native_resource {
            let mut resources = self.resources.lock().await;
            let result = (|| {
                // An input publication can hold the independent input lock
                // past expiry. Resource serialization can do the same. Recheck
                // before native effects, but never hold lease state during I/O.
                self.leases.validate(&outer_lease)?;
                let Resources { journal, runtime } = &mut *resources;
                runtime
                    .as_mut()
                    .context("native resource runtime vanished")?
                    .prepare_rpc(
                        journal,
                        inputs.as_deref(),
                        &admission.owner,
                        &method,
                        request["params"].as_object().unwrap(),
                    )
            })();
            // The journal, not a discarded RPC future, retains an unfinished
            // native side effect and its cleanup blocker.
            let prepared = result?;
            drop(resources);
            drop(inputs);
            let value = match prepared {
                Dispatch::Ready(value) => value,
                Dispatch::Session(pending) => {
                    let completed = pending.exchange().await;
                    let mut resources = self.resources.lock().await;
                    let Resources { journal, runtime } = &mut *resources;
                    runtime
                        .as_mut()
                        .context("native resource runtime vanished")?
                        .complete_session(journal, completed)?
                }
                pending @ (Dispatch::Analysis(_) | Dispatch::Command(_)) => {
                    tokio::time::timeout(Duration::from_secs(300), async {
                        let mut ticks = tokio::time::interval(Duration::from_millis(100));
                        loop {
                            ticks.tick().await;
                            let mut resources = self.resources.lock().await;
                            let Resources { journal, runtime } = &mut *resources;
                            let runtime = runtime.as_mut().context("native runtime vanished")?;
                            let value = match &pending {
                                Dispatch::Analysis(pending) => {
                                    runtime.poll_analysis(journal, pending)?
                                }
                                Dispatch::Command(pending) => {
                                    runtime.poll_command(journal, pending)?
                                }
                                _ => unreachable!(),
                            };
                            if let Some(value) = value {
                                return Ok::<_, anyhow::Error>(value);
                            }
                        }
                    })
                    .await
                    .context("resource observation timed out; reconcile its original request")??
                }
            };
            return encode(&json!({"jsonrpc":"2.0","id":request["id"],"result":value}));
        }
        let result = if let Some(inputs) = &inputs {
            self.leases.validate(&outer_lease)?;
            inputs.call(
                admission.owner.clone(),
                &method,
                request["params"].as_object().unwrap(),
            )
        } else {
            Err(anyhow::anyhow!(
                "Windows execution runtime is not initialized"
            ))
        };
        encode(&json!({"jsonrpc":"2.0","id":request["id"],"result":result?}))
    }
    pub(crate) fn qualify(&self) {
        self.qualified
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

fn encode(value: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}
#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
