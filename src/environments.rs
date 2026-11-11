//! Registered analysis environments. Connections are lazy and never select a fallback worker.
use crate::runtime::{RuntimeBackend, RuntimeClient, RuntimeConfig};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::OnceCell;

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
            self.runtime.backend != RuntimeBackend::Disabled,
            "registered environment must have a transport"
        );
        ensure!(
            self.runtime.backend != RuntimeBackend::Docker || self.platform == Platform::Linux,
            "Docker execution environments must use Linux"
        );
        ensure!(
            self.runtime.backend != RuntimeBackend::TrustedLocal
                || self.platform == Platform::current(),
            "trusted fixture platform does not match this host"
        );
        ensure!(
            self.runtime.request_timeout_secs > 0,
            "environment timeout must be positive"
        );
        Ok(())
    }
}

struct Entry {
    config: EnvironmentConfig,
    client: OnceCell<RuntimeClient>,
}

#[derive(Clone, Default)]
pub struct EnvironmentRegistry {
    entries: Arc<BTreeMap<String, Entry>>,
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
                },
            );
        }
        Ok(Self {
            entries: Arc::new(entries),
        })
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

    pub async fn close(&self) {
        for entry in self.entries.values() {
            if let Some(client) = entry.client.get() {
                let _ = client.close().await;
            }
        }
    }
}
