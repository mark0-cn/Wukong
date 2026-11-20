use anyhow::{Context, Result, ensure};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const PUBLIC_KEY: &str = include_str!("../../../interfaces/desktop/update-public.key");
pub const RELEASES: &str = "https://github.com/mark0-cn/Wukong/releases";
pub const DATA_SCHEMA: u32 = 5;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artifact {
    pub url: String,
    pub signature: String,
    pub sha256: String,
    pub size: u64,
}
/// Optional offline Windows worker media template published beside one release.
///
/// The desktop downloads this prebuilt template, verifies it against the signed
/// catalog and then seals it locally for a specific installation. Older releases
/// (and rehearsals without a staged template) omit the entry entirely.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerMedia {
    /// Release version that built the template; must equal the catalog version.
    pub version: String,
    /// Pinned release asset name, the last path segment of `url`.
    pub name: String,
    pub size: u64,
    pub sha256: String,
    /// Version-pinned immutable release asset URL.
    pub url: String,
}
impl WorkerMedia {
    /// Validate the optional template entry against the signed catalog version
    /// and the release asset URL rules. The controller-version check belongs to
    /// the downloader: an update check may legitimately see a newer catalog.
    pub fn validate(&self, manifest_version: &str) -> Result<()> {
        ensure!(
            self.version == manifest_version,
            "worker media template version differs from the release"
        );
        let prefix = format!("{RELEASES}/download/v{manifest_version}/");
        ensure!(
            self.url.strip_prefix(&prefix) == Some(self.name.as_str()),
            "worker media template must be this release's pinned asset"
        );
        ensure!(
            self.name.ends_with(".zip")
                && self.name.len() <= 128
                && self
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)),
            "invalid worker media template asset name"
        );
        let hex_sha256 = self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
        ensure!(
            hex_sha256 && self.size > 0 && self.size <= 8 * 1024 * 1024 * 1024,
            "invalid worker media template size or checksum"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub version: String,
    pub notes: String,
    pub minimum_version: String,
    pub runtime_protocol: u32,
    pub data_schema: u32,
    pub image: String,
    pub image_size: u64,
    pub source_revision: String,
    pub platforms: BTreeMap<String, Artifact>,
    /// Optional prebuilt offline Windows worker media template for this version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_media: Option<WorkerMedia>,
}

pub fn target() -> Result<&'static str> {
    ensure!(
        cfg!(target_arch = "x86_64"),
        "only x64 desktop releases are supported"
    );
    if cfg!(windows) {
        Ok("windows-x86_64")
    } else if cfg!(target_os = "linux") {
        Ok("linux-x86_64")
    } else {
        anyhow::bail!("desktop updates support Windows and Linux")
    }
}

pub fn verify(data: &[u8], signature: &str) -> Result<()> {
    verify_with_key(data, signature, PUBLIC_KEY)
}
fn verify_with_key(data: &[u8], signature: &str, key: &str) -> Result<()> {
    let decode = |value: &str| -> Result<String> {
        Ok(String::from_utf8(
            base64::engine::general_purpose::STANDARD.decode(value.trim())?,
        )?)
    };
    let public = minisign_verify::PublicKey::decode(&decode(key)?)?;
    public.verify(
        data,
        &minisign_verify::Signature::decode(&decode(signature)?)?,
        false,
    )?;
    Ok(())
}

impl Manifest {
    pub fn parse(data: &[u8], signature: &str) -> Result<Self> {
        verify(data, signature).context("release signature verification failed")?;
        let manifest: Self = serde_json::from_slice(data)?;
        manifest.validate()?;
        Ok(manifest)
    }
    pub fn validate(&self) -> Result<()> {
        let version = semver::Version::parse(&self.version)?;
        ensure!(
            version.pre.is_empty() && version.build.is_empty(),
            "stable version required"
        );
        ensure!(
            self.schema == 1
                && self.runtime_protocol == crate::adapters::worker::protocol::PROTOCOL
                && self.data_schema == DATA_SCHEMA,
            "unsupported release format, runtime protocol or data migration"
        );
        ensure!(
            semver::Version::parse(&self.minimum_version)? <= version,
            "invalid upgrade range"
        );
        let hex = |s: &str, len| {
            s.len() == len
                && s.bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        };
        ensure!(hex(&self.source_revision, 40), "invalid source revision");
        let digest = self
            .image
            .strip_prefix("hlmark0/wukong@sha256:")
            .context("image must be pinned to the Wukong repository")?;
        ensure!(
            hex(digest, 64) && self.image_size > 0,
            "invalid image identity"
        );
        for platform in ["windows-x86_64", "linux-x86_64"] {
            let artifact = self
                .platforms
                .get(platform)
                .context("release is missing a supported platform")?;
            let prefix = format!("{RELEASES}/download/v{}/", self.version);
            let filename = artifact
                .url
                .strip_prefix(&prefix)
                .context("artifact must belong to this release")?;
            ensure!(
                !filename.is_empty()
                    && filename
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)),
                "invalid artifact URL"
            );
            ensure!(
                hex(&artifact.sha256, 64)
                    && artifact.size > 0
                    && artifact.size <= 1024 * 1024 * 1024,
                "invalid artifact size or checksum"
            );
            ensure!(!artifact.signature.is_empty(), "unsigned artifact");
        }
        if let Some(media) = &self.worker_media {
            media.validate(&self.version)?;
        }
        Ok(())
    }
    pub fn artifact(&self) -> Result<&Artifact> {
        self.platforms
            .get(target()?)
            .context("unsupported platform")
    }
    pub fn can_upgrade(&self, current: &str) -> Result<bool> {
        let current = semver::Version::parse(current)?;
        ensure!(
            current >= semver::Version::parse(&self.minimum_version)?,
            "install an intermediate supported version first"
        );
        Ok(semver::Version::parse(&self.version)? > current)
    }
}

pub fn verify_file(path: &Path, artifact: &Artifact) -> Result<()> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    ensure!(
        bytes.len() as u64 == artifact.size
            && hex::encode(Sha256::digest(&bytes)) == artifact.sha256,
        "download checksum mismatch"
    );
    verify(&bytes, &artifact.signature)
}

pub async fn fetch(version: Option<&str>) -> Result<(Manifest, Vec<u8>, String)> {
    let base = match version {
        Some(version) => {
            semver::Version::parse(version)?;
            format!("{RELEASES}/download/v{version}")
        }
        None => format!("{RELEASES}/latest/download"),
    };
    let client = reqwest::Client::builder()
        .https_only(true)
        .timeout(std::time::Duration::from_secs(45))
        .build()?;
    let read = async |name: &str| -> Result<Vec<u8>> {
        let mut response = client
            .get(format!("{base}/{name}"))
            .send()
            .await?
            .error_for_status()?;
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                data.len() + chunk.len() <= 1024 * 1024,
                "release metadata exceeds limit"
            );
            data.extend_from_slice(&chunk);
        }
        Ok(data)
    };
    let bytes = read("latest.json").await?;
    let signature = String::from_utf8(read("latest.json.sig").await?)?;
    let manifest = Manifest::parse(&bytes, &signature)?;
    if let Some(version) = version {
        ensure!(manifest.version == version, "release version mismatch");
    }
    Ok((manifest, bytes, signature))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsigned_metadata_is_rejected() {
        assert!(Manifest::parse(b"{}", "").is_err());
        assert!(verify(b"changed", "not-a-signature").is_err());
    }
    #[test]
    fn optional_worker_media_must_be_the_pinned_asset_of_this_release() -> Result<()> {
        let media = |version: &str, name: &str, size: u64, sha256: &str, url: &str| WorkerMedia {
            version: version.into(),
            name: name.into(),
            size,
            sha256: sha256.into(),
            url: url.into(),
        };
        let name = "wukong-windows-worker-media-1.0.0.zip";
        let url = format!("{RELEASES}/download/v1.0.0/{name}");
        let valid = media("1.0.0", name, 1024, &"a".repeat(64), &url);
        valid.validate("1.0.0")?;
        assert!(
            media("0.9.0", name, 1024, &"a".repeat(64), &url)
                .validate("1.0.0")
                .is_err()
        );
        assert!(
            media(
                "1.0.0",
                name,
                1024,
                &"a".repeat(64),
                "https://example.com/x.zip"
            )
            .validate("1.0.0")
            .is_err()
        );
        assert!(
            media("1.0.0", name, 0, &"a".repeat(64), &url)
                .validate("1.0.0")
                .is_err()
        );
        assert!(
            media("1.0.0", name, 1024, "not-a-hash", &url)
                .validate("1.0.0")
                .is_err()
        );
        assert!(
            media("1.0.0", "other.zip", 1024, &"a".repeat(64), &url)
                .validate("1.0.0")
                .is_err()
        );
        Ok(())
    }
    #[test]
    fn real_signature_rejects_modified_content_and_wrong_key() -> Result<()> {
        let data = include_bytes!("../../../../tests/fixtures/update-signature/message.txt");
        let signature = include_str!("../../../../tests/fixtures/update-signature/message.txt.sig");
        let key = include_str!("../../../../tests/fixtures/update-signature/test.pub");
        verify_with_key(data, signature, key)?;
        assert!(verify_with_key(b"modified", signature, key).is_err());
        assert!(verify(data, signature).is_err());
        Ok(())
    }
}
