//! Transport choices cannot carry fields belonging to another transport.
use super::ssh;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeBackend {
    Disabled,
    TrustedLocal,
    Docker,
    Ssh,
    ManagedSsh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(flatten)]
    pub transport: TransportConfig,
    #[serde(default = "default_timeout")]
    pub request_timeout_secs: u64,
}
impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            transport: TransportConfig::Disabled,
            request_timeout_secs: default_timeout(),
        }
    }
}
fn default_timeout() -> u64 {
    300
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransportConfig {
    #[default]
    Disabled,
    TrustedLocal(TrustedLocalConfig),
    Docker(DockerConfig),
    Ssh(SshConfig),
    ManagedSsh(ManagedSshConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TrustedLocalConfig {
    pub state_dir: PathBuf,
    pub python: String,
    pub bridge_path: PathBuf,
}
impl Default for TrustedLocalConfig {
    fn default() -> Self {
        Self {
            state_dir: PathBuf::from(".wukong/worker"),
            python: "python".into(),
            bridge_path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("workers/runtime/bridge.py"),
        }
    }
}
fn default_ssh() -> String {
    "ssh".into()
}
fn default_bridge() -> String {
    "/opt/wukong/runtime/bridge.py".into()
}
fn default_state() -> String {
    "/state".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DockerConfig {
    pub docker_container: String,
    #[serde(default = "super::default_docker_executable")]
    pub docker_executable: String,
    #[serde(default = "default_bridge")]
    pub remote_bridge_path: String,
    #[serde(default = "default_state")]
    pub remote_state_dir: String,
}
impl DockerConfig {
    pub fn new(container: String) -> Self {
        Self {
            docker_container: container,
            docker_executable: super::default_docker_executable(),
            remote_bridge_path: default_bridge(),
            remote_state_dir: default_state(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshConfig {
    pub ssh_host: String,
    #[serde(default = "default_ssh")]
    pub ssh_executable: String,
    #[serde(default = "default_bridge")]
    pub remote_bridge_path: String,
    #[serde(default = "default_state")]
    pub remote_state_dir: String,
}
impl SshConfig {
    pub fn new(host: String) -> Self {
        Self {
            ssh_host: host,
            ssh_executable: default_ssh(),
            remote_bridge_path: default_bridge(),
            remote_state_dir: default_state(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedSshConfig {
    pub ssh_host: String,
    #[serde(default = "default_ssh")]
    pub ssh_executable: String,
    pub managed_ssh: ssh::ManagedSsh,
}

impl RuntimeConfig {
    pub fn trusted_local(&self) -> Option<&TrustedLocalConfig> {
        match &self.transport {
            TransportConfig::TrustedLocal(config) => Some(config),
            _ => None,
        }
    }
    pub fn trusted_local_mut(&mut self) -> Option<&mut TrustedLocalConfig> {
        match &mut self.transport {
            TransportConfig::TrustedLocal(config) => Some(config),
            _ => None,
        }
    }
    pub fn backend(&self) -> RuntimeBackend {
        match self.transport {
            TransportConfig::Disabled => RuntimeBackend::Disabled,
            TransportConfig::TrustedLocal(_) => RuntimeBackend::TrustedLocal,
            TransportConfig::Docker(_) => RuntimeBackend::Docker,
            TransportConfig::Ssh(_) => RuntimeBackend::Ssh,
            TransportConfig::ManagedSsh(_) => RuntimeBackend::ManagedSsh,
        }
    }
    pub fn managed_ssh(&self) -> Option<&ssh::ManagedSsh> {
        match &self.transport {
            TransportConfig::ManagedSsh(config) => Some(&config.managed_ssh),
            _ => None,
        }
    }
    pub fn managed_ssh_mut(&mut self) -> Option<&mut ssh::ManagedSsh> {
        match &mut self.transport {
            TransportConfig::ManagedSsh(config) => Some(&mut config.managed_ssh),
            _ => None,
        }
    }
    pub fn docker_container(&self) -> Option<&str> {
        match &self.transport {
            TransportConfig::Docker(config) => Some(&config.docker_container),
            _ => None,
        }
    }
    pub fn ssh_host(&self) -> Option<&str> {
        match &self.transport {
            TransportConfig::Ssh(config) => Some(&config.ssh_host),
            TransportConfig::ManagedSsh(config) => Some(&config.ssh_host),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_rejects_mixed_or_missing_fields() {
        for value in [
            serde_json::json!({"backend":"docker"}),
            serde_json::json!({"backend":"docker","docker_container":"worker","ssh_host":"host"}),
            serde_json::json!({"backend":"ssh","ssh_host":"host","managed_ssh":{}}),
            serde_json::json!({"backend":"trusted_local","docker_container":"worker"}),
        ] {
            assert!(serde_json::from_value::<RuntimeConfig>(value).is_err());
        }
        let config: RuntimeConfig = serde_json::from_value(serde_json::json!({"backend":"docker","docker_container":"worker","request_timeout_secs":12})).unwrap();
        assert_eq!(config.request_timeout_secs, 12);
        assert_eq!(config.docker_container(), Some("worker"));
    }
}
