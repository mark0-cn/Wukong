//! Install and cleanup plans bound to one experiment generation.
//!
//! The controller only validates and records plans. Guest installation is a
//! later phase; every target, service and account still needs an explicit
//! cleanup action so ownership is unambiguous before execution exists.
use crate::application::windows_lab::LaunchKind;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstallPlan {
    pub targets: Vec<String>,
    #[serde(default)]
    pub service: Option<ServiceInstall>,
    #[serde(default)]
    pub account: Option<AccountInstall>,
    /// Optional ownership notes. The disposable VM is discarded by a baseline
    /// restore, so cleanup is not a precondition in this phase.
    #[serde(default)]
    pub cleanup: Vec<CleanupAction>,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceInstall {
    pub name: String,
    pub binary: String,
    pub kernel_driver: bool,
    #[serde(default)]
    pub start: bool,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AccountInstall {
    pub name: String,
}

#[derive(schemars::JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CleanupAction {
    pub kind: CleanupKind,
    pub target: String,
}

#[derive(
    schemars::JsonSchema, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum CleanupKind {
    File,
    Directory,
    Service,
    Account,
}

impl InstallPlan {
    pub fn validate(&self, launch: LaunchKind) -> Result<()> {
        ensure!(
            !self.targets.is_empty(),
            "install plan needs at least one target"
        );
        let mut seen = std::collections::BTreeSet::new();
        for target in &self.targets {
            ensure!(
                valid_windows_path(target),
                "install target must be an absolute Windows path without dot segments: {target}"
            );
            ensure!(
                seen.insert(target.to_ascii_lowercase()),
                "duplicate install target: {target}"
            );
        }
        if let Some(service) = &self.service {
            ensure!(
                valid_name(&service.name, 256),
                "service name must be 1–256 safe characters"
            );
            ensure!(
                self.targets
                    .iter()
                    .any(|target| target.eq_ignore_ascii_case(&service.binary)),
                "service binary must be one of the install targets"
            );
            if service.kernel_driver {
                ensure!(
                    launch == LaunchKind::Driver,
                    "a kernel driver service requires launch=driver"
                );
            }
        }
        if launch == LaunchKind::Driver {
            let service = self
                .service
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("launch=driver requires an installed service"))?;
            ensure!(
                service.kernel_driver,
                "launch=driver requires a kernel driver service"
            );
        }
        if let Some(account) = &self.account {
            ensure!(
                valid_name(&account.name, 64),
                "account name must be 1–64 safe characters"
            );
        }
        let mut cleanup = std::collections::BTreeSet::new();
        for action in &self.cleanup {
            ensure!(
                valid_cleanup_target(action),
                "invalid cleanup target: {}",
                action.target
            );
            ensure!(
                cleanup.insert((action.kind, action.target.to_ascii_lowercase())),
                "duplicate cleanup action for {}",
                action.target
            );
        }
        Ok(())
    }

    pub fn sha256(&self, launch: LaunchKind) -> Result<String> {
        self.validate(launch)?;
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(self)?)))
    }
}

fn valid_name(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn valid_windows_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    !value.is_empty()
        && value.len() <= 512
        && !value.contains('\0')
        && !value.starts_with(r"\\?\")
        && bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
        && !value
            .split(['\\', '/'])
            .any(|part| part == "." || part == "..")
}

fn valid_cleanup_target(action: &CleanupAction) -> bool {
    match action.kind {
        CleanupKind::File | CleanupKind::Directory => valid_windows_path(&action.target),
        CleanupKind::Service => valid_name(&action.target, 256),
        CleanupKind::Account => valid_name(&action.target, 64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> InstallPlan {
        InstallPlan {
            targets: vec![r"C:\lab\app.exe".into(), r"C:\lab\data.bin".into()],
            service: Some(ServiceInstall {
                name: "wukong-lab-app".into(),
                binary: r"C:\lab\app.exe".into(),
                kernel_driver: false,
                start: true,
            }),
            account: None,
            cleanup: vec![
                CleanupAction {
                    kind: CleanupKind::Directory,
                    target: r"C:\lab".into(),
                },
                CleanupAction {
                    kind: CleanupKind::Service,
                    target: "wukong-lab-app".into(),
                },
            ],
        }
    }

    #[test]
    fn complete_plan_is_accepted() {
        assert!(sample().validate(LaunchKind::Service).is_ok());
        assert!(!sample().sha256(LaunchKind::Service).unwrap().is_empty());
    }

    #[test]
    fn cleanup_actions_are_optional_notes() {
        let mut plan = sample();
        plan.cleanup.clear();
        assert!(plan.validate(LaunchKind::Service).is_ok());
        plan.account = Some(AccountInstall {
            name: "wukong-lab".into(),
        });
        assert!(plan.validate(LaunchKind::Service).is_ok());
    }

    #[test]
    fn paths_and_driver_consistency_are_checked() {
        let mut plan = sample();
        plan.targets = vec!["C:\\lab\\..\\app.exe".into()];
        assert!(plan.validate(LaunchKind::Exe).is_err());
        let mut plan = sample();
        plan.service.as_mut().unwrap().kernel_driver = true;
        assert!(plan.validate(LaunchKind::Service).is_err());
        assert!(plan.validate(LaunchKind::Driver).is_ok());
        plan.service = None;
        assert!(plan.validate(LaunchKind::Driver).is_err());
    }

    #[test]
    fn duplicate_targets_are_rejected() {
        let mut plan = sample();
        plan.targets.push(r"C:\LAB\APP.EXE".into());
        assert!(plan.validate(LaunchKind::Service).is_err());
    }
}
