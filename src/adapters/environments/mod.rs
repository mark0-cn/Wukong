//! Registered analysis environments. Connections are lazy and never select a fallback worker.
use crate::adapters::worker::{RuntimeBackend, RuntimeClient, RuntimeConfig};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{Mutex, OnceCell};
pub mod lab_guard;
pub mod registration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Linux,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentConfig {
    pub name: String,
    pub platform: Platform,
    pub runtime: RuntimeConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_id: Option<String>,
}

pub fn validate_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
        "environment ID must contain 1–64 ASCII letters, digits, underscores or hyphens"
    );
    Ok(())
}

impl EnvironmentConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 256,
            "invalid environment name"
        );
        ensure!(
            self.runtime.backend() != RuntimeBackend::Disabled,
            "registered environment must have a transport"
        );
        ensure!(
            self.runtime.backend() != RuntimeBackend::Docker || self.platform == Platform::Linux,
            "Docker execution environments must use Linux"
        );
        ensure!(
            self.runtime.backend() != RuntimeBackend::TrustedLocal
                || self.platform == Platform::current(),
            "trusted fixture platform does not match this host"
        );
        ensure!(
            self.runtime.request_timeout_secs > 0,
            "environment timeout must be positive"
        );
        if let Some(ssh) = &self.runtime.managed_ssh() {
            ensure!(
                self.runtime.backend() == RuntimeBackend::ManagedSsh
                    && self.platform == Platform::Windows,
                "managed SSH requires a Windows SSH environment"
            );
            ssh.validate()?;
            if let Some(expected) = &self.installation_id {
                ensure!(
                    *expected == ssh.installation_id,
                    "managed SSH and environment installation identities differ"
                );
            }
        }
        Ok(())
    }
}

struct Entry {
    config: EnvironmentConfig,
    client: OnceCell<RuntimeClient>,
    lease_client: OnceCell<RuntimeClient>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct LeaseKey {
    environment: String,
    run: String,
    owner: String,
}
type LeaseCache =
    Arc<Mutex<BTreeMap<LeaseKey, crate::adapters::worker::protocol::EnvironmentLease>>>;

#[derive(Clone, Default)]
pub struct EnvironmentRegistry {
    entries: Arc<BTreeMap<String, Entry>>,
    leases: LeaseCache,
    lab_paths: Arc<BTreeMap<String, std::path::PathBuf>>,
    lab_guards: Arc<Mutex<BTreeMap<String, crate::adapters::files::locks::FileLock>>>,
}

impl EnvironmentRegistry {
    pub fn new(configs: BTreeMap<String, EnvironmentConfig>) -> Result<Self> {
        let mut entries = BTreeMap::new();
        for (id, config) in configs {
            validate_id(&id)?;
            config
                .validate()
                .with_context(|| format!("invalid environment {id}"))?;
            entries.insert(
                id,
                Entry {
                    config,
                    client: OnceCell::new(),
                    lease_client: OnceCell::new(),
                },
            );
        }
        Ok(Self {
            entries: Arc::new(entries),
            leases: Default::default(),
            lab_paths: Default::default(),
            lab_guards: Default::default(),
        })
    }

    pub fn with_lab_guards(mut self, labs: &crate::config::WindowsLabConfig) -> Result<Self> {
        let mut paths = BTreeMap::new();
        for vm in labs.vms.values() {
            if let Some(environment) = &vm.environment {
                let path = lab_guard::path(&vm.vmx)?;
                ensure!(
                    paths.insert(environment.clone(), path).is_none(),
                    "duplicate lab environment mapping"
                );
            }
        }
        self.lab_paths = Arc::new(paths);
        Ok(self)
    }

    pub fn config(&self, id: &str) -> Result<&EnvironmentConfig> {
        Ok(&self
            .entries
            .get(id)
            .with_context(|| format!("environment is not registered: {id}"))?
            .config)
    }

    pub fn list(&self) -> Value {
        json!(self.entries.iter().map(|(id, entry)| json!({
            "environment_id": id, "name": entry.config.name, "platform": entry.config.platform,
            "connected": entry.client.initialized(),
        })).collect::<Vec<_>>())
    }

    pub async fn client(&self, id: &str) -> Result<RuntimeClient> {
        let entry = self
            .entries
            .get(id)
            .with_context(|| format!("environment is not registered: {id}"))?;
        let client = entry
            .client
            .get_or_try_init(|| async {
                let client = RuntimeClient::connect(entry.config.runtime.clone()).await?;
                if let Some(expected) = &entry.config.installation_id {
                    ensure!(
                        &client.identity().installation_id == expected,
                        "worker installation identity does not match this registered environment"
                    );
                }
                let doctor = client.doctor().await?;
                let observed = doctor["os"]
                    .as_str()
                    .context("worker did not report its operating system")?;
                ensure!(
                    observed == entry.config.platform.as_str(),
                    "worker platform mismatch: expected {}, got {observed}",
                    entry.config.platform.as_str()
                );
                Ok::<_, anyhow::Error>(client)
            })
            .await?;
        Ok(client.clone())
    }

    pub async fn doctor(&self, id: &str) -> Result<Value> {
        let observation = self.client(id).await?.doctor().await?;
        Ok(json!({"environment_id": id, "observation": observation}))
    }

    async fn lease_client(&self, id: &str) -> Result<&RuntimeClient> {
        let entry = self.entries.get(id).context("environment not registered")?;
        let client = entry
            .lease_client
            .get_or_try_init(|| async {
                let mut configuration = entry.config.runtime.clone();
                configuration.request_timeout_secs = 10;
                let client = RuntimeClient::connect(configuration).await?;
                ensure!(
                    client.identity() == self.client(id).await?.identity(),
                    "lease channel connected to another worker"
                );
                Ok::<_, anyhow::Error>(client)
            })
            .await?;
        Ok(client)
    }

    pub(crate) async fn bind(
        &self,
        id: &str,
        run: &str,
        owner: Option<&str>,
    ) -> Result<RuntimeClient> {
        let mut leases = self.leases.lock().await;
        if let Some(path) = self.lab_paths.get(id) {
            let mut guards = self.lab_guards.lock().await;
            if !guards.contains_key(id) {
                guards.insert(
                    id.into(),
                    lab_guard::acquire(path, lab_guard::LockMode::Shared)?,
                );
            }
        }
        let client = self.client(id).await?;
        let config = self.config(id)?;
        if config.platform != Platform::Windows
            || config.runtime.backend() == RuntimeBackend::TrustedLocal
        {
            return Ok(client);
        }
        let owner = owner.context("Windows execution requires a supervised controller lease")?;
        let lease: crate::adapters::worker::protocol::EnvironmentLease = serde_json::from_value(
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.lease_client(id).await?.call(
                    "environment.lease.acquire",
                    json!({"run_id":run,"owner_id":owner}),
                ),
            )
            .await??,
        )?;
        lease.validate(run, owner, client.identity())?;
        leases.insert(
            LeaseKey {
                environment: id.into(),
                run: run.into(),
                owner: owner.into(),
            },
            lease.clone(),
        );
        Ok(client.with_lease(lease))
    }

    pub(crate) async fn renew_leases(&self, run: &str, owner: &str) -> Result<()> {
        let ids: Vec<_> = self
            .leases
            .lock()
            .await
            .keys()
            .filter(|key| key.run == run && key.owner == owner)
            .map(|key| key.environment.clone())
            .collect();
        for id in ids {
            // Recheck under the same lock as release. A supervisor snapshot
            // must never reacquire a scope that maintenance just released.
            let mut leases = self.leases.lock().await;
            let key = LeaseKey {
                environment: id.clone(),
                run: run.into(),
                owner: owner.into(),
            };
            let Some(previous) = leases.get(&key) else {
                continue;
            };
            let next: crate::adapters::worker::protocol::EnvironmentLease = serde_json::from_value(
                self.lease_client(&id)
                    .await?
                    .call(
                        "environment.lease.acquire",
                        json!({"run_id":run,"owner_id":owner}),
                    )
                    .await?,
            )?;
            next.validate(run, owner, self.client(&id).await?.identity())?;
            ensure!(
                next.same_fence(previous),
                "environment lease changed during renewal"
            );
            leases.insert(key, next);
        }
        Ok(())
    }

    /// A fresh exclusive worker lease also fences orphan resources left by a
    /// controller crash. The worker refuses takeover until its journal is idle.
    pub(crate) async fn maintenance(&self, environment: &str) -> Result<MaintenanceLease> {
        let config = self.config(environment)?;
        ensure!(
            config.platform == Platform::Windows
                && config.runtime.backend() != RuntimeBackend::TrustedLocal,
            "VM maintenance requires a registered native Windows worker"
        );
        let client = self.lease_client(environment).await?.clone();
        let identity = uuid::Uuid::new_v4().simple().to_string();
        let run = format!("maintenance-{identity}");
        let owner = run.clone();
        let lease: crate::adapters::worker::protocol::EnvironmentLease = serde_json::from_value(
            client
                .call(
                    "environment.lease.acquire",
                    json!({"run_id":run,"owner_id":owner}),
                )
                .await?,
        )?;
        lease.validate(&run, &owner, client.identity())?;
        let cancellation = tokio_util::sync::CancellationToken::new();
        let stop = cancellation.clone();
        let renewed = client.clone();
        let fence = lease.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(15)) => {
                        let next = renewed.call("environment.lease.acquire", json!({"run_id":run,"owner_id":owner})).await;
                        let valid = next.and_then(|v| Ok(serde_json::from_value::<crate::adapters::worker::protocol::EnvironmentLease>(v)?))
                            .is_ok_and(|next| next.same_fence(&fence));
                        if !valid { tracing::error!("maintenance lease heartbeat lost; VM operation requires reconciliation"); break; }
                    }
                }
            }
            let _ = renewed
                .call("environment.lease.release", json!({"lease":fence}))
                .await;
        });
        Ok(MaintenanceLease {
            cancellation,
            task: Some(task),
        })
    }

    /// Release only the exact caller scope, retaining other assessments and transports.
    pub(crate) async fn release_scope(
        &self,
        environment: &str,
        run: &str,
        owner: &str,
    ) -> Result<()> {
        let key = LeaseKey {
            environment: environment.into(),
            run: run.into(),
            owner: owner.into(),
        };
        let mut leases = self.leases.lock().await;
        if let Some(lease) = leases.get(&key) {
            self.lease_client(environment)
                .await?
                .call("environment.lease.release", json!({"lease":lease}))
                .await?;
            leases.remove(&key);
        }
        if !leases.keys().any(|key| key.environment == environment) {
            self.lab_guards.lock().await.remove(environment);
        }
        Ok(())
    }

    pub async fn close(&self) {
        let leases = std::mem::take(&mut *self.leases.lock().await);
        for (key, lease) in &leases {
            if let Ok(client) = self.lease_client(&key.environment).await
                && let Err(error) = client
                    .call("environment.lease.release", json!({"lease":lease}))
                    .await
            {
                tracing::warn!(%error,"environment lease retained until owned resources are reconciled");
            }
        }
        self.lab_guards.lock().await.clear();
        for entry in self.entries.values() {
            if let Some(client) = entry.client.get() {
                let _ = client.close().await;
            }
            if let Some(client) = entry.lease_client.get() {
                let _ = client.close().await;
            }
        }
    }
}

pub(crate) struct MaintenanceLease {
    cancellation: tokio_util::sync::CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for MaintenanceLease {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl MaintenanceLease {
    pub(crate) async fn finish(mut self) -> Result<()> {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.await?;
        }
        Ok(())
    }
}
