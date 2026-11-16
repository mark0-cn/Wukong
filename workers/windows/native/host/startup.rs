//! Bounded startup facts, never isolation evidence or execution authority.
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub(crate) const MAX_REPORT_BYTES: usize = 16 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    DeploymentIdentity,
    ResourceRecovery,
    DependencySelftest,
    GuestControlsSelftest,
    CommunicationReady,
}
impl Stage {
    fn failure(self) -> &'static str {
        match self {
            Self::DeploymentIdentity => "deployment_identity_failed",
            Self::ResourceRecovery => "resource_recovery_failed",
            Self::DependencySelftest => "dependency_selftest_failed",
            Self::GuestControlsSelftest => "guest_controls_failed",
            Self::CommunicationReady => "communication_readiness_failed",
        }
    }
    fn summary(self) -> &'static str {
        match self {
            Self::DeploymentIdentity => {
                "Deployment identity or protected configuration could not be verified."
            }
            Self::ResourceRecovery => "Owned resources could not be reconciled before startup.",
            Self::DependencySelftest => "Sealed dependency self-test did not pass.",
            Self::GuestControlsSelftest => {
                "Guest controls check did not pass; execution remains unavailable."
            }
            Self::CommunicationReady => {
                "Authenticated worker communication could not become ready."
            }
        }
    }
}
#[derive(Serialize)]
struct Event {
    stage: Stage,
    status: &'static str,
    elapsed_ms: u64,
    code: &'static str,
    summary: &'static str,
}
#[derive(Serialize)]
pub(crate) struct Report {
    schema: u32,
    installation_id: String,
    package_sha256: Option<String>,
    boot_id: Option<String>,
    attempt_id: String,
    started_at_ms: u64,
    ready: bool,
    events: Vec<Event>,
    execution_boundary: wukong_worker_package::ExecutionBoundary,
    external_isolation_verified: bool,
    #[serde(skip)]
    directory: PathBuf,
    #[serde(skip)]
    started: Instant,
    #[serde(skip)]
    active: Option<(Stage, Instant)>,
    #[serde(skip)]
    publisher: fn(&Path, &[u8]) -> Result<()>,
}
impl Report {
    pub(crate) fn new(configuration: &Path, installation: &str) -> Result<Self> {
        Self::create(
            configuration,
            installation,
            crate::artifacts::io::replace_private,
        )
    }
    fn create(
        configuration: &Path,
        installation: &str,
        publisher: fn(&Path, &[u8]) -> Result<()>,
    ) -> Result<Self> {
        ensure!(
            !installation.is_empty()
                && installation.len() <= 128
                && installation
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
            "invalid diagnostic installation identity"
        );
        let report = Self {
            schema: 2,
            installation_id: installation.into(),
            package_sha256: None,
            boot_id: None,
            attempt_id: hex::encode(crate::host::accounts::random_bytes::<16>()?),
            started_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_millis()
                .min(u64::MAX as u128) as u64,
            ready: false,
            events: vec![],
            execution_boundary: wukong_worker_package::ExecutionBoundary::DisposableVm,
            external_isolation_verified: false,
            directory: configuration
                .parent()
                .context("startup configuration parent missing")?
                .into(),
            started: Instant::now(),
            active: None,
            publisher,
        };
        report.publish("startup_pending")?;
        Ok(report)
    }
    pub(crate) fn bind(&mut self, package: &str, boot: &str) -> Result<()> {
        ensure!(
            package.len() == 64
                && package.bytes().all(|c| c.is_ascii_hexdigit())
                && boot.len() <= 128,
            "invalid diagnostic package or boot identity"
        );
        self.package_sha256 = Some(package.into());
        self.boot_id = Some(boot.into());
        self.publish("startup_pending")
    }
    pub(crate) fn begin(&mut self, stage: Stage) -> Result<()> {
        let order = [
            Stage::DeploymentIdentity,
            Stage::ResourceRecovery,
            Stage::DependencySelftest,
            Stage::GuestControlsSelftest,
            Stage::CommunicationReady,
        ];
        ensure!(
            self.active.is_none()
                && self.events.len() < 10
                && self
                    .events
                    .last()
                    .is_none_or(|event| event.status == "succeeded")
                && order[self.events.len() / 2] == stage,
            "invalid startup stage transition"
        );
        self.active = Some((stage, Instant::now()));
        self.events.push(Event {
            stage,
            status: "started",
            elapsed_ms: 0,
            code: "stage_started",
            summary: "Startup stage started.",
        });
        self.publish("startup_pending")
    }
    pub(crate) fn succeed(&mut self) -> Result<()> {
        let (stage, started) = self.active.take().context("startup stage missing")?;
        self.events.push(Event {
            stage,
            status: "succeeded",
            elapsed_ms: millis(started),
            code: "stage_succeeded",
            summary: "Startup stage succeeded.",
        });
        if stage == Stage::CommunicationReady {
            self.ready = true;
        }
        self.publish(if self.ready {
            "startup_ready"
        } else {
            "startup_pending"
        })
    }
    pub(crate) fn fail(&mut self, _error: &anyhow::Error) -> Result<()> {
        let code = if let Some((stage, started)) = self.active.take() {
            self.events.push(Event {
                stage,
                status: "failed",
                elapsed_ms: millis(started),
                code: stage.failure(),
                summary: stage.summary(),
            });
            stage.failure()
        } else {
            "service_runtime_failed"
        };
        // Error text may contain paths, commands or secrets. Only the typed
        // classification and bounded counters cross this diagnostic boundary.
        self.publish(code)
    }
    fn encoded(&self) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(self)?;
        ensure!(
            bytes.len() <= MAX_REPORT_BYTES,
            "startup diagnostic exceeds size limit"
        );
        Ok(bytes)
    }
    fn publish(&self, code: &str) -> Result<()> {
        (self.publisher)(
            &self.directory.join("startup-diagnostics.json"),
            &self.encoded()?,
        )?;
        (self.publisher)(
            &self.directory.join("last-service-error.txt"),
            format!(
                "attempt={} installation={} code={} elapsed_ms={}\n",
                self.attempt_id,
                self.installation_id,
                code,
                millis(self.started)
            )
            .as_bytes(),
        )
    }
}
fn millis(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u64::MAX as u128) as u64
}

pub(crate) fn failure_summary(
    configuration: &Path,
    installation: &str,
    package: &str,
) -> Result<String> {
    let path = configuration
        .parent()
        .context("diagnostic parent missing")?
        .join("startup-diagnostics.json");
    crate::isolation::security::AclPolicy::private_vm().verify(&path, false)?;
    let value: serde_json::Value = serde_json::from_slice(&crate::artifacts::io::read_bytes(
        &path,
        MAX_REPORT_BYTES as u64,
    )?)?;
    ensure!(
        value["schema"] == 2
            && value["execution_boundary"] == "disposable_vm"
            && value["external_isolation_verified"] == false
            && value["installation_id"] == installation
            && (value["package_sha256"].is_null() || value["package_sha256"] == package),
        "diagnostic identity mismatch"
    );
    let attempt = value["attempt_id"]
        .as_str()
        .context("diagnostic attempt missing")?;
    ensure!(
        attempt.len() == 32 && attempt.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid diagnostic attempt"
    );
    let event = value["events"]
        .as_array()
        .and_then(|events| events.last())
        .context("diagnostic stage missing")?;
    let code = event["code"].as_str().context("diagnostic code missing")?;
    ensure!(
        code.len() <= 64 && code.bytes().all(|c| c.is_ascii_lowercase() || c == b'_'),
        "invalid diagnostic code"
    );
    Ok(serde_json::json!({"attempt_id":attempt,"installation_id":installation,"package_sha256":package,"code":code,"execution_boundary":"disposable_vm","external_isolation_verified":false}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codes_are_stable_and_failure_summaries_do_not_copy_arbitrary_errors() {
        let stages = [
            Stage::DeploymentIdentity,
            Stage::ResourceRecovery,
            Stage::DependencySelftest,
            Stage::GuestControlsSelftest,
            Stage::CommunicationReady,
        ];
        let codes: std::collections::BTreeSet<_> = stages.iter().map(|s| s.failure()).collect();
        assert_eq!(codes.len(), 5);
        for stage in stages {
            assert!(stage.summary().len() < 128);
        }
        let report = Report {
            schema: 2,
            installation_id: "installation".into(),
            package_sha256: Some("a".repeat(64)),
            boot_id: Some("boot".into()),
            attempt_id: "b".repeat(32),
            started_at_ms: 0,
            ready: false,
            events: stages
                .iter()
                .map(|&stage| Event {
                    stage,
                    status: "failed",
                    elapsed_ms: 42,
                    code: stage.failure(),
                    summary: stage.summary(),
                })
                .collect(),
            execution_boundary: wukong_worker_package::ExecutionBoundary::DisposableVm,
            external_isolation_verified: false,
            directory: PathBuf::new(),
            started: Instant::now(),
            active: None,
            publisher: crate::artifacts::io::replace_private,
        };
        let bytes = report.encoded().unwrap();
        assert!(bytes.len() < MAX_REPORT_BYTES);
        let mut oversized = report;
        oversized.boot_id = Some("x".repeat(MAX_REPORT_BYTES));
        assert!(oversized.encoded().is_err());
    }

    #[test]
    fn every_stage_failure_replaces_previous_attempt_and_redacts_untrusted_error() -> Result<()> {
        fn fixture_publish(path: &Path, bytes: &[u8]) -> Result<()> {
            std::fs::write(path, bytes)?;
            Ok(())
        }
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("deployment.json");
        let stages = [
            Stage::DeploymentIdentity,
            Stage::ResourceRecovery,
            Stage::DependencySelftest,
            Stage::GuestControlsSelftest,
            Stage::CommunicationReady,
        ];
        let mut previous = String::new();
        for failed in 0..stages.len() {
            let mut report = Report::create(&path, "installation", fixture_publish)?;
            assert_ne!(report.attempt_id, previous);
            previous = report.attempt_id.clone();
            report.bind(&"a".repeat(64), "boot")?;
            for &stage in &stages[..failed] {
                report.begin(stage)?;
                report.succeed()?;
            }
            report.begin(stages[failed])?;
            report.fail(&anyhow::anyhow!("token=PRIVATE-CANARY"))?;
            assert!(!report.external_isolation_verified);
            let raw = std::fs::read_to_string(directory.path().join("startup-diagnostics.json"))?;
            let summary = std::fs::read_to_string(directory.path().join("last-service-error.txt"))?;
            assert!(!raw.contains("PRIVATE-CANARY"));
            assert!(summary.contains(&report.attempt_id));
            assert!(summary.contains(stages[failed].failure()));
            assert!(!report.ready);
            assert!(report.begin(Stage::CommunicationReady).is_err());
        }
        let mut report = Report::create(&path, "installation", fixture_publish)?;
        for stage in stages {
            report.begin(stage)?;
            report.succeed()?;
        }
        assert!(report.ready);
        assert!(
            std::fs::read_to_string(directory.path().join("last-service-error.txt"))?
                .contains("startup_ready")
        );
        Ok(())
    }
}
