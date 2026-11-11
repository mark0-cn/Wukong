//! Local raw logs stay local. Export only a closed projection of diagnostic facts.
use crate::i18n::Message;
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Diagnosis {
    Daemon,
    Credentials,
    Network,
    RateLimit,
    Disk,
    Unknown,
}
pub fn classify(message: &Message) -> Diagnosis {
    let text = format!(
        "{} {}",
        message.code,
        message.detail.as_deref().unwrap_or_default()
    )
    .to_lowercase();
    if [
        "credential",
        "unauthorized",
        "authentication required",
        "denied: requested access",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Diagnosis::Credentials
    } else if [
        "no space left",
        "disk full",
        "insufficient-space",
        "not enough space",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Diagnosis::Disk
    } else if ["toomanyrequests", "too many requests", "429", "rate limit"]
        .iter()
        .any(|s| text.contains(s))
    {
        Diagnosis::RateLimit
    } else if [
        "docker-host",
        "daemon",
        "dockerdesktoplinuxengine",
        "docker_engine",
        "docker-linux-required",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Diagnosis::Daemon
    } else if [
        "timeout",
        "timed out",
        "tls handshake",
        "connection reset",
        "no such host",
        "network is unreachable",
        "dns",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Diagnosis::Network
    } else {
        Diagnosis::Unknown
    }
}

pub fn log_directory(log: &Path, cache: &Path) -> Result<PathBuf> {
    ensure!(
        log.file_name().is_some_and(|n| n == "pull.log"),
        "invalid diagnostic log"
    );
    let probes = std::fs::canonicalize(cache.join("probes"))?;
    let directory = std::fs::canonicalize(log.parent().context("missing log parent")?)?;
    ensure!(
        directory.starts_with(&probes) && directory != probes,
        "diagnostic log escaped its cache"
    );
    if log.exists() {
        ensure!(
            std::fs::canonicalize(log)?.parent() == Some(directory.as_path()),
            "diagnostic log points outside its directory"
        );
    }
    Ok(directory)
}

/// Do not serialize Status wholesale: it contains paths, errors and arbitrary text.
/// Dropping free-form strings is intentional redaction, not a regex promise.
pub fn bundle(status: &super::updates::Status) -> serde_json::Value {
    let phase = match status.phase.as_str() {
        "idle" | "starting" | "checking" | "downloading" | "environment" | "docker" | "pulling"
        | "validating" | "activating" | "installing" | "storage" | "health" | "tools" | "error"
        | "ready" | "cancelled" => status.phase.as_str(),
        _ => "unknown",
    };
    serde_json::json!({
        "schema": 1,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "client_version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "operation_id": status.operation_id.as_ref().and_then(|id| uuid::Uuid::parse_str(id).ok()).map(|id| id.to_string()),
        "revision": status.revision, "phase": phase, "action": status.action,
        "busy": status.busy, "cancelling": status.cancelling,
        "diagnosis": status.diagnosis,
        "client_bytes": status.downloaded, "client_total": status.total,
        "image": status.image_progress.as_ref().map(|p| serde_json::json!({
            "observed_layers": p.layers.len(), "elapsed_ms": p.elapsed_ms,
            "quiet_ms": p.quiet_ms, "finished": p.finished, "cached": p.cached,
        })),
        "health": { "configured": status.health.configured, "docker": status.health.docker,
            "running": status.health.running, "tools": status.health.tools,
            "checks_passed": status.health.checks.values().filter(|v| **v).count(),
            "checks_total": status.health.checks.len() },
        "redaction": "Raw messages, logs, paths, URLs, configuration, credentials and user-supplied strings are omitted. Full raw logs remain on this computer."
    })
}
pub fn export(status: &super::updates::Status) -> Result<Option<PathBuf>> {
    let Some(path) = rfd::FileDialog::new()
        .set_title("Wukong diagnostics")
        .add_filter("JSON", &["json"])
        .set_file_name("wukong-diagnostics.json")
        .save_file()
    else {
        return Ok(None);
    };
    let parent = path
        .parent()
        .context("diagnostic destination needs a parent")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut file, &bundle(status))?;
    file.as_file().sync_all()?;
    file.persist(&path).map_err(|e| e.error)?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn common_failures_have_distinct_actions_and_unknown_is_preserved() {
        for (text, expected) in [
            (
                "docker-credential-desktop executable file not found",
                Diagnosis::Credentials,
            ),
            ("Cannot connect to the Docker daemon", Diagnosis::Daemon),
            ("TLS handshake timeout", Diagnosis::Network),
            ("toomanyrequests: rate limit", Diagnosis::RateLimit),
            ("no space left on device", Diagnosis::Disk),
            ("unknown registry response", Diagnosis::Unknown),
        ] {
            assert_eq!(
                classify(&Message::new("error-operation").detail(text)),
                expected
            );
        }
    }
    #[test]
    fn export_omits_raw_secrets_paths_and_untrusted_strings() {
        let mut state = super::super::updates::Controller::default().status();
        let secret = "Authorization: Bearer MY_SECRET https://user:MY_SECRET@host/?token=MY_SECRET C:\\Users\\private\\state -----BEGIN PRIVATE KEY-----";
        state.message = Message::new("error-operation").detail(secret);
        state.health.error = Some(state.message.clone());
        state.health.checks.insert(secret.into(), false);
        state.health.image = Some(secret.into());
        state.notes = secret.into();
        state.phase = secret.into();
        state.operation_id = Some(secret.into());
        let text = bundle(&state).to_string();
        assert!(!text.contains("MY_SECRET"));
        assert!(!text.contains("private"));
        assert!(!text.contains("Authorization"));
        assert_eq!(bundle(&state)["phase"], "unknown");
    }
    #[test]
    fn only_registered_probe_logs_can_be_opened() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("probes/id/logs");
        std::fs::create_dir_all(&directory).unwrap();
        assert!(log_directory(&directory.join("pull.log"), root.path()).is_ok());
        let other = tempfile::tempdir().unwrap();
        assert!(log_directory(&other.path().join("pull.log"), root.path()).is_err());
        assert!(log_directory(&directory.join("config.json"), root.path()).is_err());
    }
}
