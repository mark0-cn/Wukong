use crate::{paths, validate_path};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PACKAGE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PayloadKind {
    Runtime,
    WorkerSource,
    OriginalInstaller,
    License,
    Data,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub sha256: String,
    pub size: u64,
    pub kind: PayloadKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryPoints {
    pub worker: String,
    pub python: String,
    pub bridge: String,
    /// Legacy packages can still be authenticated for maintenance, but their
    /// helper is never executed by the new native debugger backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dbgeng_helper: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windbg_mcp: Option<String>,
    pub dbgeng_library: String,
    pub sshd: String,
    pub ssh_keygen: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub version: String,
    pub channel: String,
    pub original_payloads: Vec<String>,
    pub licenses: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub platform: String,
    pub version: String,
    pub source_revision: String,
    pub toolchain_lock_sha256: String,
    pub toolchain_lock_path: String,
    pub runtime_protocol: u32,
    pub minimum_controller_version: String,
    pub entrypoints: EntryPoints,
    pub components: BTreeMap<String, Component>,
    pub license_notices: Vec<String>,
    pub files: BTreeMap<String, FileEntry>,
}

pub struct Compatibility<'a> {
    pub runtime_protocol: u32,
    pub controller_version: &'a str,
}

impl Manifest {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1 && self.platform == "windows-amd64",
            "unsupported worker package format or platform"
        );
        ensure!(self.runtime_protocol > 0, "missing worker protocol");
        semver::Version::parse(&self.version).context("invalid worker package version")?;
        semver::Version::parse(&self.minimum_controller_version)
            .context("invalid minimum controller version")?;
        ensure!(
            self.source_revision.len() == 40
                && self
                    .source_revision
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "invalid package source revision"
        );
        ensure!(
            paths::hash(&self.toolchain_lock_sha256),
            "invalid toolchain lock hash"
        );
        ensure!(
            !self.files.is_empty() && self.files.len() <= 20_000,
            "invalid package file count"
        );
        let mut names = BTreeSet::new();
        let mut total = 0u64;
        for (path, entry) in &self.files {
            validate_path(path)?;
            let folded = path.to_ascii_lowercase();
            ensure!(
                !["manifest.json", "manifest.sig", "manifest.pub"].contains(&folded.as_str()),
                "package metadata cannot hash itself"
            );
            ensure!(names.insert(folded), "case-colliding package paths");
            ensure!(
                paths::hash(&entry.sha256) && entry.size <= 2 * 1024 * 1024 * 1024,
                "invalid file identity or size"
            );
            total = total
                .checked_add(entry.size)
                .context("package size overflow")?;
        }
        ensure!(total <= MAX_PACKAGE_BYTES, "worker package exceeds 8 GiB");
        // A file cannot also be an ancestor directory of another file.
        for path in &names {
            let mut current = path.as_str();
            while let Some((parent, _)) = current.rsplit_once('/') {
                ensure!(!names.contains(parent), "package file/directory collision");
                current = parent;
            }
        }
        for path in [
            &self.entrypoints.worker,
            &self.entrypoints.python,
            &self.entrypoints.dbgeng_library,
            &self.entrypoints.sshd,
            &self.entrypoints.ssh_keygen,
        ] {
            self.require_kind(path, PayloadKind::Runtime)?;
        }
        let external_debugger = self.entrypoints.windbg_mcp.is_some();
        ensure!(
            external_debugger != self.entrypoints.dbgeng_helper.is_some(),
            "package must select one debugger backend"
        );
        self.require_kind(
            self.entrypoints
                .windbg_mcp
                .as_ref()
                .or(self.entrypoints.dbgeng_helper.as_ref())
                .unwrap(),
            PayloadKind::Runtime,
        )?;
        self.require_kind(&self.entrypoints.bridge, PayloadKind::WorkerSource)?;
        self.require_kind(&self.toolchain_lock_path, PayloadKind::Data)?;
        ensure!(
            self.files[&self.toolchain_lock_path].sha256 == self.toolchain_lock_sha256,
            "toolchain lock is not bound to package contents"
        );
        ensure!(
            !self.license_notices.is_empty(),
            "native worker license notices are required"
        );
        for path in &self.license_notices {
            self.require_kind(path, PayloadKind::License)?;
        }
        let mut original_payloads = BTreeSet::new();
        let mut required_components = vec![
            "python",
            "debugging_tools",
            "openssh",
            "worker_python_dependencies",
        ];
        if external_debugger {
            required_components.push("windbg_mcp");
        }
        for name in &required_components {
            let component = self
                .components
                .get(*name)
                .with_context(|| format!("missing offline component: {name}"))?;
            ensure!(
                !component.version.is_empty() && component.version.len() <= 128,
                "invalid component version"
            );
            ensure!(
                ["stable", "preview"].contains(&component.channel.as_str()),
                "invalid component channel"
            );
            ensure!(
                !component.original_payloads.is_empty() && !component.licenses.is_empty(),
                "component payloads and licenses are required"
            );
            for path in &component.original_payloads {
                self.require_kind(path, PayloadKind::OriginalInstaller)?;
                ensure!(
                    original_payloads.insert(path),
                    "duplicate original installation payload"
                );
            }
            for path in &component.licenses {
                self.require_kind(path, PayloadKind::License)?;
            }
        }
        ensure!(
            self.components.len() == required_components.len(),
            "unsupported offline component"
        );
        Ok(())
    }

    fn require_kind(&self, path: &str, expected: PayloadKind) -> Result<()> {
        let entry = self
            .files
            .get(path)
            .with_context(|| format!("missing offline package member: {path}"))?;
        ensure!(
            entry.kind == expected && entry.size > 0,
            "invalid package member kind or empty required payload: {path}"
        );
        Ok(())
    }

    pub fn check_compatibility(&self, expected: &Compatibility<'_>) -> Result<()> {
        self.validate()?;
        ensure!(
            self.runtime_protocol == expected.runtime_protocol,
            "worker package protocol is incompatible"
        );
        ensure!(
            semver::Version::parse(expected.controller_version)?
                >= semver::Version::parse(&self.minimum_controller_version)?,
            "worker package requires a newer controller"
        );
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() <= MAX_MANIFEST_BYTES,
            "worker manifest exceeds 4 MiB"
        );
        Ok(bytes)
    }
}
