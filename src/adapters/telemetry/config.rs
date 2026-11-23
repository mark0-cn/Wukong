//! Persisted observability settings. Credentials always remain in environment variables.

use anyhow::{Context, Result, ensure};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ObservabilityConfig {
    pub service_name: String,
    /// Base OTLP/HTTP endpoint. Standard OTEL_EXPORTER_OTLP variables override it.
    pub otlp_endpoint: Option<String>,
    pub traces: bool,
    pub metrics: bool,
    pub logs: bool,
    pub local_jsonl: bool,
    pub log_retention_days: u64,
    pub shutdown_timeout_secs: u64,
    pub query: QueryConfig,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            service_name: "wukong".into(),
            otlp_endpoint: None,
            traces: true,
            metrics: true,
            logs: false,
            local_jsonl: true,
            log_retention_days: 7,
            shutdown_timeout_secs: 3,
            query: QueryConfig::default(),
        }
    }
}

impl ObservabilityConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.service_name.trim().is_empty() && self.service_name.len() <= 128,
            "observability.service_name must contain 1–128 characters"
        );
        ensure!(
            (1..=90).contains(&self.log_retention_days),
            "observability.log_retention_days must be 1–90"
        );
        ensure!(
            (1..=30).contains(&self.shutdown_timeout_secs),
            "observability.shutdown_timeout_secs must be 1–30"
        );
        if let Some(endpoint) = &self.otlp_endpoint {
            validate_endpoint("observability.otlp_endpoint", endpoint)?;
        }
        self.query.validate()
    }

    pub fn sdk_disabled() -> bool {
        std::env::var("OTEL_SDK_DISABLED").is_ok_and(|value| value.eq_ignore_ascii_case("true"))
    }

    pub fn export_configured(&self) -> bool {
        !Self::sdk_disabled()
            && (self.otlp_endpoint.is_some()
                || [
                    "OTEL_EXPORTER_OTLP_ENDPOINT",
                    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                    "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
                    "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
                ]
                .iter()
                .any(|name| std::env::var_os(name).is_some()))
    }

    pub fn signal_configured(&self, signal: Signal) -> bool {
        if Self::sdk_disabled() {
            return false;
        }
        let specific = signal_endpoint_variable(signal);
        self.otlp_endpoint.is_some()
            || std::env::var_os(specific).is_some()
            || std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_some()
    }

    pub fn signal_enabled(&self, signal: Signal) -> bool {
        match signal {
            Signal::Traces => self.traces,
            Signal::Metrics => self.metrics,
            Signal::Logs => self.logs,
        }
    }

    /// Resolve the full per-signal endpoint. Base endpoints receive the OTLP
    /// standard path, while a signal-specific endpoint is already complete.
    pub(super) fn exporter_endpoint(&self, signal: Signal) -> Result<Option<String>> {
        if Self::sdk_disabled() {
            return Ok(None);
        }
        if let Some(endpoint) = environment(signal_endpoint_variable(signal))? {
            validate_endpoint("OTEL_EXPORTER_OTLP signal endpoint", &endpoint)?;
            return Ok(Some(endpoint));
        }
        let base = match environment("OTEL_EXPORTER_OTLP_ENDPOINT")? {
            Some(endpoint) => Some(endpoint),
            None => self.otlp_endpoint.clone(),
        };
        let Some(base) = base else {
            return Ok(None);
        };
        validate_endpoint("OTEL_EXPORTER_OTLP base endpoint", &base)?;
        let mut url = reqwest::Url::parse(&base).context("invalid OTLP base endpoint")?;
        let path = format!(
            "{}/v1/{}",
            url.path().trim_end_matches('/'),
            match signal {
                Signal::Traces => "traces",
                Signal::Metrics => "metrics",
                Signal::Logs => "logs",
            }
        );
        url.set_path(&path);
        Ok(Some(url.into()))
    }

    /// Credentials are read only at activation and are never persisted or returned.
    pub(super) fn exporter_headers(&self, signal: Signal) -> Result<HashMap<String, String>> {
        if Self::sdk_disabled() {
            return Ok(HashMap::new());
        }
        let specific = match signal {
            Signal::Traces => "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
            Signal::Metrics => "OTEL_EXPORTER_OTLP_METRICS_HEADERS",
            Signal::Logs => "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
        };
        let value = match environment(specific)? {
            Some(value) => Some(value),
            None => environment("OTEL_EXPORTER_OTLP_HEADERS")?,
        };
        value.map_or_else(|| Ok(HashMap::new()), |value| parse_headers(&value))
    }
}

fn signal_endpoint_variable(signal: Signal) -> &'static str {
    match signal {
        Signal::Traces => "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        Signal::Metrics => "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
        Signal::Logs => "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
    }
}

fn environment(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            anyhow::bail!("{name} must contain valid UTF-8")
        }
    }
}

fn parse_headers(value: &str) -> Result<HashMap<String, String>> {
    let mut headers = HashMap::new();
    for pair in value.split(',').filter(|pair| !pair.trim().is_empty()) {
        let (name, value) = pair
            .split_once('=')
            .context("OTLP headers must use comma-separated name=value pairs")?;
        let name = name.trim();
        ensure!(!name.is_empty(), "OTLP header name cannot be empty");
        let decoded = percent_decode_str(value.trim())
            .decode_utf8()
            .context("OTLP header value must be valid UTF-8")?;
        headers.insert(name.to_owned(), decoded.into_owned());
    }
    Ok(headers)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Traces,
    Metrics,
    Logs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QueryConfig {
    pub tempo: Option<BackendConfig>,
    pub loki: Option<BackendConfig>,
    pub prometheus: Option<BackendConfig>,
    pub grafana_url: Option<String>,
    pub timeout_secs: u64,
    pub max_range_secs: u64,
}

impl Default for QueryConfig {
    fn default() -> Self {
        Self {
            tempo: None,
            loki: None,
            prometheus: None,
            grafana_url: None,
            timeout_secs: 5,
            max_range_secs: 86_400,
        }
    }
}

impl QueryConfig {
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    pub fn max_range_secs(&self) -> u64 {
        self.max_range_secs
    }

    fn validate(&self) -> Result<()> {
        for (name, backend) in [
            ("tempo", &self.tempo),
            ("loki", &self.loki),
            ("prometheus", &self.prometheus),
        ] {
            if let Some(backend) = backend {
                backend.validate(name)?;
            }
        }
        if let Some(url) = &self.grafana_url {
            validate_endpoint("observability.query.grafana_url", url)?;
        }
        ensure!(
            (1..=60).contains(&self.timeout_secs()),
            "observability.query.timeout_secs must be 1–60"
        );
        ensure!(
            (60..=31 * 86_400).contains(&self.max_range_secs()),
            "observability.query.max_range_secs must be 60 seconds–31 days"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    pub url: String,
    /// Name of an environment variable containing a bearer token, never the token.
    pub credential_env: Option<String>,
}

impl BackendConfig {
    fn validate(&self, name: &str) -> Result<()> {
        validate_endpoint(&format!("observability.query.{name}.url"), &self.url)?;
        if let Some(variable) = &self.credential_env {
            ensure!(
                !variable.is_empty()
                    && variable
                        .bytes()
                        .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
                    && variable
                        .as_bytes()
                        .first()
                        .is_some_and(|byte| *byte == b'_' || byte.is_ascii_alphabetic()),
                "observability.query.{name}.credential_env must be an environment variable name"
            );
        }
        Ok(())
    }
}

fn validate_endpoint(name: &str, value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| anyhow::anyhow!("{name} must be an absolute HTTP(S) URL"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "{name} must use HTTP(S)"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "{name} must not contain credentials"
    );
    ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "{name} must not contain query parameters or a fragment"
    );
    if url.scheme() == "http" {
        let loopback = url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
        });
        ensure!(
            loopback,
            "{name} allows plain HTTP only for loopback endpoints"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_reject_credentials_and_remote_plain_http() {
        let mut config = ObservabilityConfig {
            otlp_endpoint: Some("https://user:secret@example.com".into()),
            ..Default::default()
        };
        assert!(config.validate().is_err());
        config.otlp_endpoint = Some("http://example.com".into());
        assert!(config.validate().is_err());
        config.otlp_endpoint = Some("http://127.0.0.1:4318".into());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn query_credentials_are_environment_names() {
        let mut config = ObservabilityConfig::default();
        config.query.tempo = Some(BackendConfig {
            url: "https://tempo.example.com".into(),
            credential_env: Some("not a variable".into()),
        });
        assert!(config.validate().is_err());
    }

    #[test]
    fn query_limits_reject_zero_instead_of_coalescing_to_defaults() {
        let timeout = ObservabilityConfig {
            query: QueryConfig {
                timeout_secs: 0,
                ..QueryConfig::default()
            },
            ..ObservabilityConfig::default()
        };
        assert!(timeout.validate().is_err());
        assert_eq!(timeout.query.timeout_secs(), 0);
        let range = ObservabilityConfig {
            query: QueryConfig {
                max_range_secs: 0,
                ..QueryConfig::default()
            },
            ..ObservabilityConfig::default()
        };
        assert!(range.validate().is_err());
        assert_eq!(range.query.max_range_secs(), 0);
    }
}
