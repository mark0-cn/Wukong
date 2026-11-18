//! Build metadata of a local-release desktop package (`cargo package-local`).
//!
//! A build with the Cargo feature `local-release` bundles `local-release.json`
//! as a Tauri resource. That descriptor names the locally built Docker image (a
//! plain local tag, not a registry digest), the current client version and the
//! worker protocol/data schema it was built for, plus the optional absolute path
//! of a local Windows worker media template. First-run onboarding reads it
//! instead of the online signed `latest.json` catalog, so an unpublished build
//! can prepare its environment from an image that already exists on this host.
//!
//! The feature is off in release builds: CI installers always resolve their
//! runtime image through the signed catalog. The descriptor carries no
//! signature, secrets or image bytes; its trust boundary is the local build
//! itself, so it only grants local-image onboarding and never replaces an
//! online update identity.
use super::manifest::DATA_SCHEMA;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// Bundled Tauri resource name of the local-release descriptor.
pub const RESOURCE: &str = "local-release.json";
/// Descriptor schema understood by this build.
pub const SCHEMA: u32 = 1;
/// Environment variable with the absolute path of another descriptor.
///
/// This exists for `tauri dev` style runs where Tauri does not copy bundled
/// resources next to the development executable yet.
pub const DESCRIPTOR_OVERRIDE: &str = "WUKONG_LOCAL_RELEASE_JSON";

/// Version the packaging tool uses when it is not overridden.
pub const DEFAULT_IMAGE: &str = "wukong:local";

static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Record where the desktop host keeps bundled resources.
///
/// Called by the Tauri setup hook before any user action. Repeating the call is
/// harmless; only the first directory is kept.
pub fn initialize(resource_dir: &Path) {
    if RESOURCE_DIR.set(resource_dir.to_owned()).is_err() {
        tracing::debug!(
            directory = %resource_dir.display(),
            "local-release resource directory was already initialized"
        );
    }
}

fn resource_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(DESCRIPTOR_OVERRIDE) {
        ensure!(
            !path.is_empty(),
            format!("{DESCRIPTOR_OVERRIDE} must name a local-release.json file")
        );
        return Ok(PathBuf::from(path));
    }
    let directory = RESOURCE_DIR.get().context(
        "local-release build metadata was not initialized by the desktop host; this build cannot prepare a local environment",
    )?;
    Ok(directory.join(RESOURCE))
}

/// Read and validate the descriptor of this local-release package.
pub fn read() -> Result<LocalRelease> {
    read_at(&resource_path()?)
}

/// Read and validate one descriptor path; used by tests and error context.
pub(crate) fn read_at(path: &Path) -> Result<LocalRelease> {
    let bytes = std::fs::read(path).with_context(|| {
        format!(
            "the local-release build descriptor {} is missing or unreadable; rebuild this package with `cargo package-local`",
            path.display()
        )
    })?;
    LocalRelease::parse(&bytes)
        .with_context(|| format!("invalid local-release build descriptor {}", path.display()))
}

/// Version, image and build identity of a local-release package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalRelease {
    /// Always [`SCHEMA`]; other versions are refused instead of guessed.
    pub schema: u32,
    /// Client version that packaged this build; must match this executable.
    pub version: String,
    /// Worker runtime protocol the packaged image was built for.
    pub runtime_protocol: u32,
    /// Workspace data schema this client migrates to.
    pub data_schema: u32,
    /// Locally built Docker image tag, e.g. `wukong:local`.
    pub image: String,
    /// Image size recorded by the packaging host, in bytes.
    pub image_size: u64,
    /// Optional absolute path of a local Windows worker media template archive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_path: Option<String>,
    /// UTC packaging time, RFC 3339; informational only.
    pub built_at: String,
    /// Source commit the package was built from.
    pub source_revision: String,
}

impl LocalRelease {
    /// Parse and validate one descriptor document.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let descriptor: Self = serde_json::from_slice(bytes)?;
        descriptor.validate()?;
        Ok(descriptor)
    }

    /// The checks of `Manifest::validate` that still make sense without a signed
    /// catalog: stable version, schema, protocol/data schema, source revision and
    /// image identity. Signature, registry pinning, platforms and minimum-version
    /// ranges belong to online releases and are intentionally skipped.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == SCHEMA,
            "unsupported local-release descriptor schema {}; rebuild this package with `cargo package-local`",
            self.schema
        );
        let version = semver::Version::parse(&self.version)
            .context("local-release.json records an invalid client version")?;
        ensure!(
            version.pre.is_empty() && version.build.is_empty(),
            "local-release.json requires a stable client version"
        );
        ensure!(
            self.runtime_protocol == crate::adapters::worker::protocol::PROTOCOL
                && self.data_schema == DATA_SCHEMA,
            "local-release.json was built for runtime protocol {} and data schema {}, but this client requires {} and {}; rebuild this package with `cargo package-local`",
            self.runtime_protocol,
            self.data_schema,
            crate::adapters::worker::protocol::PROTOCOL,
            DATA_SCHEMA
        );
        local_image(&self.image)?;
        ensure!(
            self.image_size > 0,
            "local-release.json records an invalid Docker image size"
        );
        ensure!(
            self.source_revision.len() == 40
                && self
                    .source_revision
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "local-release.json records an invalid source revision"
        );
        chrono::DateTime::parse_from_rfc3339(&self.built_at)
            .context("local-release.json records an invalid build time")?;
        if let Some(template) = &self.template_path {
            ensure!(
                !template.is_empty() && template.len() <= 4096,
                "local-release.json records an invalid worker template path"
            );
            ensure!(
                Path::new(template).is_absolute(),
                "local-release.json must record an absolute worker template path"
            );
        }
        Ok(())
    }

    /// Absolute local worker media template path, when this build packaged one.
    pub fn template(&self) -> Option<&Path> {
        self.template_path.as_deref().map(Path::new)
    }
}

/// A local Docker image tag is passed to the Docker CLI as one argv element; it
/// must be a plausible image reference and never contain whitespace or control
/// bytes. Registry digests and remote names are allowed but not required.
fn local_image(image: &str) -> Result<()> {
    ensure!(
        !image.is_empty() && image.len() <= 512,
        "local-release.json records an invalid Docker image reference"
    );
    ensure!(
        !image.starts_with('-')
            && image
                .bytes()
                .all(|byte| { byte.is_ascii_alphanumeric() || b"._-/:@+".contains(&byte) }),
        "local-release.json records an invalid Docker image reference"
    );
    Ok(())
}

/// Parse a descriptor without a Tauri resource directory.
///
/// Only tests use this entry point directly; the application reads the bundled
/// resource through [`read`].
#[cfg(test)]
fn parse_for_tests(value: serde_json::Value) -> Result<LocalRelease> {
    LocalRelease::parse(&serde_json::to_vec(&value).expect("test descriptor serializes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

    fn descriptor() -> serde_json::Value {
        serde_json::json!({
            "schema": SCHEMA,
            "version": env!("CARGO_PKG_VERSION"),
            "runtime_protocol": crate::adapters::worker::protocol::PROTOCOL,
            "data_schema": DATA_SCHEMA,
            "image": DEFAULT_IMAGE,
            "image_size": 10_254_615_327u64,
            "built_at": "2026-11-18T06:00:00Z",
            "source_revision": REVISION,
        })
    }

    #[test]
    fn a_current_descriptor_is_accepted_and_needs_no_registry_digest() -> Result<()> {
        let release = parse_for_tests(descriptor())?;
        assert_eq!(release.image, DEFAULT_IMAGE);
        assert_eq!(release.image_size, 10_254_615_327);
        assert_eq!(release.template(), None);
        assert!(release.version == env!("CARGO_PKG_VERSION"));
        Ok(())
    }

    #[test]
    fn protocol_and_data_schema_must_match_this_client() {
        for (field, value) in [("runtime_protocol", 1u64), ("data_schema", 3u64)] {
            let mut value_document = descriptor();
            value_document[field] = value.into();
            let error = parse_for_tests(value_document).unwrap_err();
            assert!(
                format!("{error:#}").contains("rebuild this package with `cargo package-local`")
            );
        }
    }

    #[test]
    fn corrupt_identity_fields_are_refused() {
        for (field, value) in [
            ("schema", serde_json::json!(2)),
            ("version", serde_json::json!("1.0.0-rc.1")),
            ("version", serde_json::json!("not-a-version")),
            ("image", serde_json::json!("")),
            ("image", serde_json::json!("-bad tag")),
            ("image_size", serde_json::json!(0)),
            ("source_revision", serde_json::json!("ABC")),
            ("built_at", serde_json::json!("not a time")),
            ("template_path", serde_json::json!("relative/template.zip")),
        ] {
            let mut value_document = descriptor();
            value_document[field] = value;
            assert!(
                parse_for_tests(value_document).is_err(),
                "descriptor with invalid {field} must be refused"
            );
        }
    }

    #[test]
    fn a_packaged_local_template_is_exposed_as_an_absolute_path() -> Result<()> {
        let mut value_document = descriptor();
        let template = std::path::absolute("target/worker-media-template-gui.zip")?;
        value_document["template_path"] = serde_json::json!(template.display().to_string());
        let release = parse_for_tests(value_document)?;
        assert_eq!(release.template(), Some(template.as_path()));
        Ok(())
    }

    #[test]
    fn read_at_explains_how_to_rebuild_a_missing_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let error = read_at(&directory.path().join(RESOURCE)).unwrap_err();
        assert!(format!("{error:#}").contains("cargo package-local"));
    }

    #[test]
    fn read_at_rejects_corrupt_documents_and_accepts_the_packaged_shape() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join(RESOURCE);
        std::fs::write(&path, b"{")?;
        assert!(read_at(&path).is_err());
        std::fs::write(&path, serde_json::to_vec(&descriptor())?)?;
        read_at(&path)?;
        Ok(())
    }

    #[test]
    fn docker_image_references_never_smuggle_argv_whitespace() {
        assert!(local_image("wukong:local").is_ok());
        assert!(local_image("localhost:5000/team/wukong:local").is_ok());
        assert!(local_image("wukong@sha256:deadbeef").is_ok());
        assert!(local_image(" ").is_err());
        assert!(local_image("-rm").is_err());
        assert!(local_image("a\nb").is_err());
    }
}
