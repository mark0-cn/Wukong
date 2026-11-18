//! Download and verify the released offline Windows worker media template.
//!
//! The signed desktop release catalog may name the prebuilt template for exactly
//! this controller version. Trust comes only from the signed `latest.json`:
//! [`crate::application::maintenance::updates::manifest::fetch`] verifies
//! `latest.json.sig` before parsing, and the template bytes are accepted only
//! after their exact size and SHA-256 match the signed entry. Local templates
//! chosen by the operator bypass the download but go through the same template
//! validation inside the shared media generator. A `local-release` build may
//! instead name the absolute path of a template built from this checkout; that
//! path has no online signature, so the same shared template validation remains
//! its only trust boundary and the GUI still offers local file picking when the
//! descriptor omits it.
use crate::application::maintenance::updates::manifest::{self, Manifest};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use zip::ZipArchive;

/// Maximum accepted template size, matching the shared media generator.
pub const MAX_TEMPLATE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Template cache below the worker-media root.
pub const TEMPLATE_CACHE_DIR: &str = "templates";
/// Per-request network bound; a large template can legitimately take a while.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(900);

/// One signed catalog entry projected for this controller version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedTemplate {
    pub version: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub url: String,
}

/// Where the template used by one generation came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateSource {
    /// Operator-selected local file; the shared generator validates its identity.
    Local,
    /// Downloaded from the signed release catalog into the cache.
    Downloaded,
    /// Previously downloaded and re-verified from the cache.
    Cached,
}

/// A template path resolved for one generation.
#[derive(Clone, Debug)]
pub struct ResolvedTemplate {
    pub path: PathBuf,
    pub source: TemplateSource,
    pub version: Option<String>,
    pub sha256: Option<String>,
}

/// Project the signed catalog into the template this controller may download.
///
/// The catalog version must be this controller's version (the template embeds
/// the payload built by that release), and the optional entry must describe the
/// same version. Older catalogs simply omit the entry; the GUI then offers a
/// local template file instead.
pub fn expected_for_controller(
    manifest: &Manifest,
    controller_version: &str,
) -> Result<ExpectedTemplate> {
    let media = manifest.worker_media.as_ref().context(
        "this release does not publish a Windows worker media template; choose a local template file",
    )?;
    ensure!(
        media.version == manifest.version,
        "the published worker media template version differs from its release"
    );
    ensure!(
        media.version == controller_version,
        "the published Windows worker media template is version {} but this controller is {controller_version}; choose a local template file built by this version",
        media.version
    );
    ensure!(
        media.size > 0 && media.size <= MAX_TEMPLATE_BYTES,
        "the published Windows worker media template size is invalid"
    );
    Ok(ExpectedTemplate {
        version: media.version.clone(),
        name: media.name.clone(),
        size: media.size,
        sha256: media.sha256.clone(),
        url: media.url.clone(),
    })
}

/// Cache path for one template version.
pub fn cache_path(worker_media_root: &Path, version: &str) -> PathBuf {
    worker_media_root
        .join(TEMPLATE_CACHE_DIR)
        .join(format!("{version}.zip"))
}

/// Verify one downloaded or cached template file against the signed entry.
pub fn verify_file(path: &Path, expected: &ExpectedTemplate) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect the worker media template {}", path.display()))?;
    ensure!(
        metadata.is_file(),
        "the worker media template is not a regular file: {}",
        path.display()
    );
    ensure!(
        metadata.len() == expected.size,
        "the Windows worker media template size does not match the signed release"
    );
    let mut file = fs::File::open(path)
        .with_context(|| format!("open the worker media template {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    verify_digest(expected, expected.size, &hex::encode(digest.finalize()))
}

/// Pure size/checksum acceptance used by the streaming downloader and tests.
pub(crate) fn verify_digest(expected: &ExpectedTemplate, size: u64, sha256: &str) -> Result<()> {
    ensure!(
        size == expected.size && sha256 == expected.sha256,
        "the Windows worker media template download does not match its signed size or SHA-256"
    );
    Ok(())
}

/// Return the cached template when it still matches the signed entry.
pub fn verify_cache(path: &Path, expected: &ExpectedTemplate) -> Result<bool> {
    if !path.try_exists()? {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect the worker media template {}", path.display()))?;
    if !metadata.is_file() || metadata.len() != expected.size {
        return Ok(false);
    }
    let mut file = fs::File::open(path)
        .with_context(|| format!("open the worker media template {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()) == expected.sha256)
}

/// Resolve the current controller release's template, downloading it when the
/// cache is empty or stale.
pub async fn resolve_downloaded(
    worker_media_root: &Path,
    controller_version: &str,
) -> Result<ResolvedTemplate> {
    // A local-release package may bundle the template path of this checkout. It
    // has no online signature, so the shared media generator's identity checks
    // remain the trust boundary; only the signed-catalog download is skipped.
    #[cfg(feature = "local-release")]
    if let Some(resolved) = local_template(controller_version)? {
        return Ok(resolved);
    }
    // Trust boundary: this verifies latest.json.sig before parsing the catalog.
    let (catalog, _bytes, _signature) = manifest::fetch(Some(controller_version))
        .await
        .context("fetch the signed release catalog for the Windows worker media template")?;
    let expected = expected_for_controller(&catalog, controller_version)?;
    let cache = cache_path(worker_media_root, &expected.version);
    if verify_cache(&cache, &expected)? {
        return Ok(ResolvedTemplate {
            path: cache,
            source: TemplateSource::Cached,
            version: Some(expected.version),
            sha256: Some(expected.sha256),
        });
    }
    download_to_cache(&expected, &cache).await?;
    Ok(ResolvedTemplate {
        path: cache,
        source: TemplateSource::Downloaded,
        version: Some(expected.version),
        sha256: Some(expected.sha256),
    })
}

/// Resolve the template a local-release descriptor names, if this build packages one.
#[cfg(feature = "local-release")]
fn local_template(controller_version: &str) -> Result<Option<ResolvedTemplate>> {
    let release = crate::application::maintenance::updates::local_release::read()?;
    let Some(template) = release.template() else {
        // The GUI keys local template picking on this wording.
        anyhow::bail!(
            "this release does not publish a Windows worker media template; choose a local template file"
        );
    };
    ensure!(
        release.version == controller_version,
        "the local-release worker media template is built for client version {} but this controller is {controller_version}; rebuild this package with `cargo package-local`",
        release.version
    );
    let path = std::path::absolute(template).with_context(|| {
        format!(
            "resolve the local-release worker media template {}",
            template.display()
        )
    })?;
    let metadata = fs::symlink_metadata(&path).with_context(|| {
        format!(
            "the local-release worker media template is missing: {}; rebuild the template or rerun `cargo package-local` without it",
            path.display()
        )
    })?;
    ensure!(
        metadata.is_file(),
        "the local-release worker media template is not a regular file: {}",
        path.display()
    );
    ensure!(
        metadata.len() > 0 && metadata.len() <= MAX_TEMPLATE_BYTES,
        "the local-release worker media template size is invalid: {}",
        path.display()
    );
    ensure_template_archive(&path)?;
    Ok(Some(ResolvedTemplate {
        path,
        source: TemplateSource::Local,
        version: Some(release.version),
        sha256: None,
    }))
}

/// Download one catalog-named template into the cache and verify it before use.
async fn download_to_cache(expected: &ExpectedTemplate, cache: &Path) -> Result<()> {
    let parent = cache
        .parent()
        .context("the worker media template cache has no directory")?;
    fs::create_dir_all(parent)?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .context("prepare the worker media template download")?;
    let mut response = client
        .get(&expected.url)
        .send()
        .await
        .with_context(|| {
            format!(
                "download the Windows worker media template {}",
                expected.url
            )
        })?
        .error_for_status()
        .with_context(|| {
            format!(
                "download the Windows worker media template {}",
                expected.url
            )
        })?;
    if let Some(length) = response.content_length() {
        ensure!(
            length == expected.size,
            "the published Windows worker media template size changed"
        );
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut digest = Sha256::new();
    let mut written = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .context("read the Windows worker media template download")?
    {
        written = written
            .checked_add(u64::try_from(chunk.len()).context("template chunk size overflow")?)
            .context("worker media template size overflow")?;
        ensure!(
            written <= expected.size,
            "the Windows worker media template download exceeds its signed size"
        );
        digest.update(&chunk);
        temporary
            .write_all(&chunk)
            .context("write the Windows worker media template download")?;
    }
    temporary
        .as_file()
        .sync_all()
        .context("flush the Windows worker media template download")?;
    verify_digest(expected, written, &hex::encode(digest.finalize()))?;
    temporary
        .persist(cache)
        .map_err(|error| error.error)
        .with_context(|| format!("publish the worker media template {}", cache.display()))?;
    verify_file(cache, expected)?;
    Ok(())
}

/// A downloaded template must be a ZIP archive; the shared generator extracts it.
pub fn ensure_template_archive(path: &Path) -> Result<()> {
    let file = fs::File::open(path)
        .with_context(|| format!("open the worker media template {}", path.display()))?;
    ZipArchive::new(file).with_context(|| {
        format!(
            "the worker media template is not a ZIP archive: {}",
            path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP_VERSION: &str = "1.0.0";

    fn catalog(version: &str, media: Option<serde_json::Value>) -> Manifest {
        let mut value = serde_json::json!({
            "schema": 1,
            "version": version,
            "notes": "",
            "minimum_version": "0.1.2",
            "runtime_protocol": crate::adapters::worker::protocol::PROTOCOL,
            "data_schema": crate::application::maintenance::updates::manifest::DATA_SCHEMA,
            "image": format!("hlmark0/wukong@sha256:{}", "d".repeat(64)),
            "image_size": 1024,
            "source_revision": "e".repeat(40),
            "platforms": {
                "windows-x86_64": {"url": format!("https://github.com/mark0-cn/Wukong/releases/download/v{version}/wukong.exe"), "signature": "signature", "sha256": "f".repeat(64), "size": 1024},
                "linux-x86_64": {"url": format!("https://github.com/mark0-cn/Wukong/releases/download/v{version}/wukong.AppImage"), "signature": "signature", "sha256": "0".repeat(64), "size": 1024},
            },
        });
        if let Some(media) = media {
            value["worker_media"] = media;
        }
        serde_json::from_value(value).expect("fixture catalog")
    }

    fn media(version: &str) -> serde_json::Value {
        let name = format!("wukong-windows-worker-media-{version}.zip");
        serde_json::json!({
            "version": version,
            "name": name,
            "size": 4096,
            "sha256": "a".repeat(64),
            "url": format!("https://github.com/mark0-cn/Wukong/releases/download/v{version}/{name}"),
        })
    }

    #[test]
    fn the_controller_accepts_only_its_own_signed_template() -> Result<()> {
        let valid = catalog(APP_VERSION, Some(media(APP_VERSION)));
        valid.validate()?;
        let expected = expected_for_controller(&valid, APP_VERSION)?;
        assert_eq!(expected.version, APP_VERSION);
        assert_eq!(expected.size, 4096);
        assert_eq!(
            expected.name,
            format!("wukong-windows-worker-media-{APP_VERSION}.zip")
        );
        let older =
            expected_for_controller(&catalog(APP_VERSION, Some(media("0.9.0"))), APP_VERSION);
        assert!(
            older.is_err(),
            "a template from another release must be refused"
        );
        let newer =
            expected_for_controller(&catalog(APP_VERSION, Some(media(APP_VERSION))), "1.0.1");
        assert!(
            newer.is_err(),
            "this controller may not use another version's template"
        );
        assert!(
            expected_for_controller(&catalog(APP_VERSION, None), APP_VERSION).is_err(),
            "a catalog without a template requests a local file"
        );
        Ok(())
    }

    fn expected() -> ExpectedTemplate {
        ExpectedTemplate {
            version: APP_VERSION.into(),
            name: format!("wukong-windows-worker-media-{APP_VERSION}.zip"),
            size: 7,
            sha256: {
                use sha2::{Digest, Sha256};
                hex::encode(Sha256::digest(b"payload"))
            },
            url: format!(
                "https://github.com/mark0-cn/Wukong/releases/download/v{APP_VERSION}/wukong-windows-worker-media-{APP_VERSION}.zip"
            ),
        }
    }

    #[test]
    fn size_and_checksum_mismatches_are_rejected() -> Result<()> {
        let expected = expected();
        verify_digest(&expected, 7, &expected.sha256)?;
        assert!(verify_digest(&expected, 8, &expected.sha256).is_err());
        assert!(verify_digest(&expected, 7, &"0".repeat(64)).is_err());
        Ok(())
    }

    #[test]
    fn cached_templates_are_reverified_before_reuse() -> Result<()> {
        let root = tempfile::tempdir()?;
        let expected = expected();
        let cache = cache_path(root.path(), APP_VERSION);
        assert!(!verify_cache(&cache, &expected)?);
        std::fs::create_dir_all(cache.parent().expect("cache parent"))?;
        std::fs::write(&cache, b"payload")?;
        assert!(verify_cache(&cache, &expected)?);
        std::fs::write(&cache, b"tampered")?;
        assert!(
            !verify_cache(&cache, &expected)?,
            "a tampered cache entry must be re-downloaded, never used"
        );
        Ok(())
    }

    #[test]
    fn local_template_archives_are_checked_for_zip_container_shape() -> Result<()> {
        use zip::{ZipWriter, write::SimpleFileOptions};
        let root = tempfile::tempdir()?;
        let path = root.path().join("local.zip");
        std::fs::write(&path, b"not a zip")?;
        assert!(ensure_template_archive(&path).is_err());
        {
            let mut archive = ZipWriter::new(std::fs::File::create(&path)?);
            archive.start_file("media-template.json", SimpleFileOptions::default())?;
            archive.write_all(b"{}")?;
            archive.finish()?;
        }
        ensure_template_archive(&path)?;
        Ok(())
    }
}
