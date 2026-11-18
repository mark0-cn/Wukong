//! Validation and extraction of released offline worker media templates.
//!
//! A template is the CI-produced unsigned input for finalization: `payload/`,
//! `binaries/{wukong-worker.exe,wukong-package.exe}`, `manifest-draft.json`,
//! `native-build.json` and `media-template.json`. Its identity is the SHA-256 of
//! `media-template.json`, which binds the manifest draft, the native binaries and
//! the toolchain lock. The template may be an unpacked directory or the
//! deterministic release ZIP (`wukong-windows-worker-media-<version>/` wrapper).
use super::{
    MAX_MEDIA_ENTRIES, MAX_TEMPLATE_BYTES, Workspace, read_bounded, reject_link, relative_path,
    sha256_bytes, sha256_file,
};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use wukong_worker_package::Manifest;

/// Version of the running controller; a template must have been built from it.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) const TEMPLATE_SCHEMA: u32 = 1;
const TEMPLATE_RECORD: &str = "media-template.json";
const MANIFEST_DRAFT: &str = "manifest-draft.json";
const NATIVE_BUILD: &str = "native-build.json";
const PAYLOAD: &str = "payload";
const BINARIES: &str = "binaries";
const BINARY_NAMES: [&str; 2] = ["wukong-worker.exe", "wukong-package.exe"];
const WORKER_BINARY: &str = "wukong-worker.exe";
const WORKER_MEMBER: &str = "runtime/native/wukong-worker.exe";
const TOOLCHAIN_LOCK: &str = "toolchain.lock.json";
const MAX_TEMPLATE_RECORD_BYTES: usize = 1024 * 1024;
const MAX_MANIFEST_DRAFT_BYTES: usize = 4 * 1024 * 1024;
const MAX_NATIVE_BUILD_BYTES: usize = 64 * 1024;
const TEMPLATE_ROOT_ENTRIES: [&str; 5] = [
    BINARIES,
    MANIFEST_DRAFT,
    NATIVE_BUILD,
    PAYLOAD,
    TEMPLATE_RECORD,
];

#[derive(Clone, Debug, Deserialize)]
struct BinaryRecord {
    sha256: String,
    size: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateRecord {
    schema: u32,
    version: String,
    source_revision: String,
    payload_manifest_draft_sha256: String,
    binaries: BTreeMap<String, BinaryRecord>,
    toolchain_lock_sha256: String,
}

#[derive(Deserialize)]
struct NativeBuildRecord {
    schema: u32,
    platform: String,
    source_revision: String,
    files: BTreeMap<String, BinaryRecord>,
}

/// A validated unsigned template, ready for sealing.
#[derive(Debug)]
pub(crate) struct Template {
    root: PathBuf,
    pub(crate) manifest: Manifest,
    pub(crate) version: String,
    pub(crate) template_sha256: String,
    binaries: BTreeMap<String, BinaryRecord>,
}

impl Template {
    /// Open and validate a template directory or release ZIP archive. Archives
    /// are extracted into the caller's short-lived workspace and must contain
    /// either the template root or one wrapping release directory.
    pub(crate) fn open(path: &Path, workspace: &Workspace) -> Result<Self> {
        crate::adapters::files::safety::safe(path)?;
        let metadata = reject_link(path)?;
        let root = if metadata.is_dir() {
            path.to_path_buf()
        } else if metadata.is_file() {
            extract_archive(path, workspace)?
        } else {
            bail!(
                "offline media template is not a ZIP archive or directory: {}",
                path.display()
            );
        };
        Self::validate(&root)
    }

    pub(crate) fn payload_root(&self) -> PathBuf {
        self.root.join(PAYLOAD)
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn payload_path(&self, relative: &str) -> PathBuf {
        self.payload_root().join(relative_path(relative))
    }

    pub(crate) fn worker_sha256(&self) -> Result<&str> {
        self.binaries
            .get(WORKER_BINARY)
            .map(|binary| binary.sha256.as_str())
            .context("offline media template omits its worker binary")
    }

    pub(crate) fn longest_payload_member(&self) -> usize {
        self.manifest
            .files
            .keys()
            .map(String::len)
            .max()
            .unwrap_or(0)
    }

    fn validate(root: &Path) -> Result<Self> {
        crate::adapters::files::safety::safe(root)?;
        let metadata = reject_link(root)?;
        ensure!(
            metadata.is_dir(),
            "offline media template directory is missing: {}",
            root.display()
        );
        let expected_root: BTreeSet<String> = TEMPLATE_ROOT_ENTRIES
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        let mut observed_root = BTreeSet::new();
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("offline media template entry name is not UTF-8"))?;
            ensure!(
                expected_root.contains(name.as_str()),
                "offline media template contains an unsupported entry: {name}"
            );
            observed_root.insert(name);
        }
        ensure!(
            observed_root == expected_root,
            "offline media template is missing required entries"
        );

        let record_bytes = read_bounded(
            &root.join(TEMPLATE_RECORD),
            MAX_TEMPLATE_RECORD_BYTES,
            "offline media template record",
        )?;
        let template_sha256 = sha256_bytes(&record_bytes);
        let record: TemplateRecord =
            serde_json::from_slice(&record_bytes).context("parse offline media template record")?;
        ensure!(
            record.schema == TEMPLATE_SCHEMA,
            "unsupported offline media template record"
        );
        let version = canonical_semver(&record.version, "offline media template version")?;
        ensure!(
            version == APP_VERSION,
            "offline media template version {version} does not match this controller ({APP_VERSION}); download the matching release template"
        );
        ensure!(
            lowercase_hex(&record.source_revision, 40),
            "offline media template source revision is invalid"
        );
        ensure!(
            lowercase_hex(&record.payload_manifest_draft_sha256, 64),
            "offline media template manifest draft hash is invalid"
        );
        ensure!(
            lowercase_hex(&record.toolchain_lock_sha256, 64),
            "offline media template toolchain lock hash is invalid"
        );
        ensure!(
            record.binaries.len() == BINARY_NAMES.len()
                && BINARY_NAMES
                    .iter()
                    .all(|name| record.binaries.contains_key(*name)),
            "offline media template binary set is unsupported"
        );
        for name in BINARY_NAMES {
            let binary = record
                .binaries
                .get(name)
                .context("offline media template binary is missing")?;
            ensure!(
                lowercase_hex(&binary.sha256, 64) && binary.size > 0,
                "offline media template binary identity is invalid: {name}"
            );
        }

        let draft_bytes = read_bounded(
            &root.join(MANIFEST_DRAFT),
            MAX_MANIFEST_DRAFT_BYTES,
            "offline media template manifest draft",
        )?;
        ensure!(
            sha256_bytes(&draft_bytes) == record.payload_manifest_draft_sha256,
            "offline media template manifest draft changed"
        );
        let manifest: Manifest = serde_json::from_slice(&draft_bytes)
            .context("parse offline media template manifest draft")?;
        manifest
            .validate()
            .context("offline media template manifest draft is invalid")?;
        ensure!(
            manifest.schema == 2 && manifest.platform == "windows-amd64",
            "unsupported offline media template payload format"
        );
        ensure!(
            manifest.version == record.version,
            "offline media template manifest draft version differs from its record"
        );
        ensure!(
            manifest.source_revision == record.source_revision,
            "offline media template manifest draft source revision differs from its record"
        );
        ensure!(
            manifest.toolchain_lock_sha256 == record.toolchain_lock_sha256,
            "offline media template manifest draft toolchain lock differs from its record"
        );
        let worker_entry = manifest
            .files
            .get(WORKER_MEMBER)
            .context("offline media template payload omits the worker executable")?;
        let worker_binary = record
            .binaries
            .get(WORKER_BINARY)
            .context("offline media template worker binary is missing")?;
        ensure!(
            worker_entry.sha256 == worker_binary.sha256 && worker_entry.size == worker_binary.size,
            "offline media template worker executable differs from its recorded binary"
        );

        let native_bytes = read_bounded(
            &root.join(NATIVE_BUILD),
            MAX_NATIVE_BUILD_BYTES,
            "offline media template native build metadata",
        )?;
        let native: NativeBuildRecord = serde_json::from_slice(&native_bytes)
            .context("parse offline media template native build metadata")?;
        ensure!(
            native.schema == TEMPLATE_SCHEMA && native.platform == "windows-amd64",
            "unsupported offline media template native build metadata"
        );
        ensure!(
            native.source_revision == record.source_revision,
            "offline media template native build source revision differs from its record"
        );
        ensure!(
            native.files.len() == BINARY_NAMES.len()
                && BINARY_NAMES
                    .iter()
                    .all(|name| native.files.contains_key(*name)),
            "offline media template native build metadata is incomplete"
        );
        for name in BINARY_NAMES {
            let native_entry = native
                .files
                .get(name)
                .context("offline media template native build metadata is incomplete")?;
            let binary = record
                .binaries
                .get(name)
                .context("offline media template binary is missing")?;
            ensure!(
                native_entry.sha256 == binary.sha256 && native_entry.size == binary.size,
                "offline media template native build metadata differs from its record: {name}"
            );
        }

        for name in BINARY_NAMES {
            let path = root.join(BINARIES).join(name);
            let binary = record
                .binaries
                .get(name)
                .context("offline media template binary is missing")?;
            let metadata = reject_link(&path)?;
            ensure!(
                metadata.is_file() && metadata.len() == binary.size,
                "offline media template binary changed: {name}"
            );
            ensure!(
                sha256_file(&path)? == binary.sha256,
                "offline media template binary changed: {name}"
            );
        }
        let mut observed_binaries = BTreeSet::new();
        for entry in fs::read_dir(root.join(BINARIES))? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("offline media template binary name is not UTF-8"))?;
            ensure!(
                BINARY_NAMES.contains(&name.as_str()),
                "offline media template contains an unsupported binary: {name}"
            );
            observed_binaries.insert(name);
        }
        ensure!(
            observed_binaries.len() == BINARY_NAMES.len(),
            "offline media template binaries are incomplete"
        );

        let payload_root = root.join(PAYLOAD);
        let payload_metadata = reject_link(&payload_root)?;
        ensure!(
            payload_metadata.is_dir(),
            "offline media template payload is missing"
        );
        let expected_directories = expected_payload_directories(&manifest);
        let mut observed_payload = BTreeMap::new();
        let mut total = 0u64;
        let mut pending = vec![(payload_root, String::new())];
        while let Some((directory, parent)) = pending.pop() {
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let name = entry.file_name().into_string().map_err(|_| {
                    anyhow::anyhow!("offline media template payload member name is not UTF-8")
                })?;
                let relative = if parent.is_empty() {
                    name
                } else {
                    format!("{parent}/{name}")
                };
                wukong_worker_package::validate_path(&relative)?;
                let path = entry.path();
                let metadata = reject_link(&path)?;
                if metadata.is_dir() {
                    ensure!(
                        expected_directories.contains(&relative),
                        "offline media template payload has an unexpected directory: {relative}"
                    );
                    pending.push((path, relative));
                    continue;
                }
                ensure!(
                    metadata.is_file(),
                    "offline media template payload members must be regular files: {relative}"
                );
                let expected = manifest.files.get(&relative).with_context(|| {
                    format!("offline media template payload has an unexpected member: {relative}")
                })?;
                ensure!(
                    metadata.len() == expected.size,
                    "offline media template payload member size mismatch: {relative}"
                );
                total = total
                    .checked_add(metadata.len())
                    .context("offline media template payload size overflow")?;
                ensure!(
                    total <= MAX_TEMPLATE_BYTES,
                    "offline media template payload exceeds 8 GiB"
                );
                let digest = sha256_file(&path)?;
                ensure!(
                    digest == expected.sha256,
                    "offline media template payload member hash mismatch: {relative}"
                );
                observed_payload.insert(relative, digest);
            }
        }
        ensure!(
            observed_payload.len() == manifest.files.len(),
            "offline media template payload is incomplete"
        );
        let native_copy = observed_payload
            .get(NATIVE_BUILD)
            .context("offline media template payload omits native-build.json")?;
        ensure!(
            *native_copy == sha256_bytes(&native_bytes),
            "offline media template native build copies differ"
        );
        let toolchain_copy = observed_payload
            .get(TOOLCHAIN_LOCK)
            .context("offline media template payload omits the toolchain lock")?;
        ensure!(
            *toolchain_copy == record.toolchain_lock_sha256,
            "offline media template toolchain lock changed"
        );
        let worker_copy = observed_payload
            .get(WORKER_MEMBER)
            .context("offline media template payload omits the worker executable")?;
        ensure!(
            *worker_copy == worker_binary.sha256,
            "offline media template worker executable changed"
        );

        Ok(Self {
            root: root.to_path_buf(),
            manifest,
            version,
            template_sha256,
            binaries: record.binaries,
        })
    }
}

fn expected_payload_directories(manifest: &Manifest) -> BTreeSet<String> {
    let mut directories = BTreeSet::new();
    for path in manifest.files.keys() {
        let mut current = path.as_str();
        while let Some((parent, _)) = current.rsplit_once('/') {
            directories.insert(parent.to_owned());
            current = parent;
        }
    }
    directories
}

pub(crate) fn lowercase_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn canonical_semver(value: &str, label: &str) -> Result<String> {
    let parsed = semver::Version::parse(value).with_context(|| format!("invalid {label}"))?;
    ensure!(
        parsed.to_string() == value,
        "{label} is not canonical: {value}"
    );
    Ok(value.to_owned())
}

fn extract_archive(archive_path: &Path, workspace: &Workspace) -> Result<PathBuf> {
    let mut archive = zip::ZipArchive::new(fs::File::open(archive_path)?)
        .context("open offline media template archive")?;
    let destination = workspace.path("template");
    fs::create_dir(&destination)?;
    let maximum_entries = MAX_MEDIA_ENTRIES
        .saturating_mul(2)
        .saturating_add(TEMPLATE_ROOT_ENTRIES.len());
    ensure!(
        archive.len() <= maximum_entries,
        "offline media template archive exceeds {maximum_entries} entries"
    );
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry
            .name()
            .context("offline media template archive member name is not UTF-8")?
            .into_owned();
        let is_directory = name.ends_with('/');
        let relative = name.trim_end_matches('/');
        ensure!(
            !relative.is_empty(),
            "offline media template archive contains an empty member name"
        );
        wukong_worker_package::validate_path(relative)?;
        if let Some(mode) = entry.unix_mode() {
            ensure!(
                mode & 0o170000 != 0o120000,
                "offline media template archive contains a link: {relative}"
            );
        }
        let target = destination.join(relative_path(relative));
        if is_directory || entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        let size = entry.size();
        total = total
            .checked_add(size)
            .context("offline media template archive size overflow")?;
        ensure!(
            total <= MAX_TEMPLATE_BYTES,
            "offline media template archive exceeds 8 GiB"
        );
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .with_context(|| format!("extract offline media template member {relative}"))?;
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut written = 0u64;
        loop {
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            written = written
                .checked_add(u64::try_from(read).unwrap_or(u64::MAX))
                .context("offline media template archive size overflow")?;
            ensure!(
                written <= size,
                "offline media template archive member grew while extracting: {relative}"
            );
            output.write_all(&buffer[..read])?;
        }
        ensure!(
            written == size,
            "offline media template archive member changed while extracting: {relative}"
        );
    }
    locate_template_root(&destination)
}

fn locate_template_root(extracted: &Path) -> Result<PathBuf> {
    if extracted.join(TEMPLATE_RECORD).is_file() {
        return Ok(extracted.to_path_buf());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(extracted)? {
        let entry = entry?;
        entries.push(entry.path());
    }
    if entries.len() == 1 {
        let candidate = &entries[0];
        if candidate.is_dir() && candidate.join(TEMPLATE_RECORD).is_file() {
            return Ok(candidate.clone());
        }
    }
    bail!("offline media template archive does not contain a media template")
}

#[cfg(test)]
mod tests {
    use super::super::RUNTIME_PROTOCOL;
    use super::*;
    use crate::application::worker_media::fixture::{self, SyntheticTemplate};
    use crate::application::worker_media::write_directory_zip;

    fn new_workspace(temporary: &Path) -> Workspace {
        Workspace::create(Some(temporary)).expect("workspace")
    }

    fn synthetic(temporary: &Path, label: &str) -> SyntheticTemplate {
        let root = temporary.join(label);
        fs::create_dir(&root).expect("fixture directory");
        fixture::synthetic_template(&root).expect("synthetic template")
    }

    fn expect_invalid(label: &str, template: &Path, temporary: &Path) {
        let workspace = new_workspace(temporary);
        match Template::open(template, &workspace) {
            Ok(_) => panic!("{label}: template unexpectedly validated"),
            Err(error) => assert!(
                !error.to_string().is_empty(),
                "{label}: error message must explain the refusal"
            ),
        }
    }

    #[test]
    fn synthetic_template_validates_and_exposes_its_identity() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let template = synthetic(temporary.path(), "template");
        let workspace = new_workspace(temporary.path());
        let validated = Template::open(&template.root, &workspace)?;
        assert_eq!(validated.version, APP_VERSION);
        assert_eq!(
            validated.template_sha256,
            sha256_file(&template.root.join(TEMPLATE_RECORD))?
        );
        assert_eq!(validated.manifest.source_revision, fixture::SOURCE_REVISION);
        assert_eq!(
            validated.manifest.runtime_protocol, RUNTIME_PROTOCOL,
            "the fixture must exercise the released runtime protocol"
        );
        assert_eq!(
            validated.worker_sha256()?,
            template.worker_sha256,
            "worker binary identity must match the payload member"
        );
        assert_eq!(
            validated.longest_payload_member(),
            validated
                .manifest
                .files
                .keys()
                .map(String::len)
                .max()
                .unwrap_or(0)
        );
        assert!(validated.payload_path(WORKER_MEMBER).is_file());
        Ok(())
    }

    #[test]
    fn release_archive_layout_is_accepted() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let template = synthetic(temporary.path(), "template");
        let archive = temporary.path().join("template.zip");
        write_directory_zip(
            &template.root,
            &archive,
            Some(&format!("wukong-windows-worker-media-{APP_VERSION}")),
        )?;
        let workspace = new_workspace(temporary.path());
        let from_archive = Template::open(&archive, &workspace)?;
        let workspace_directory = new_workspace(temporary.path());
        let from_directory = Template::open(&template.root, &workspace_directory)?;
        assert_eq!(from_archive.template_sha256, from_directory.template_sha256);
        assert_eq!(
            from_archive.manifest.source_revision,
            from_directory.manifest.source_revision
        );
        Ok(())
    }

    #[test]
    fn tampered_templates_are_rejected() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let base = temporary.path();

        let template = synthetic(base, "record-schema");
        fixture::rewrite_json_value(&template.root.join(TEMPLATE_RECORD), &{
            let mut value = fixture::read_json(&template.root.join(TEMPLATE_RECORD))?;
            value["schema"] = serde_json::json!(2);
            value
        })?;
        expect_invalid("record schema", &template.root, base);

        let template = synthetic(base, "record-version");
        fixture::rewrite_json_value(&template.root.join(TEMPLATE_RECORD), &{
            let mut value = fixture::read_json(&template.root.join(TEMPLATE_RECORD))?;
            value["version"] = serde_json::json!("0.0.1");
            value
        })?;
        expect_invalid("record version", &template.root, base);

        let template = synthetic(base, "draft-hash");
        fixture::rewrite_json_value(&template.root.join(TEMPLATE_RECORD), &{
            let mut value = fixture::read_json(&template.root.join(TEMPLATE_RECORD))?;
            value["payload_manifest_draft_sha256"] = serde_json::json!("0".repeat(64));
            value
        })?;
        expect_invalid("manifest draft hash", &template.root, base);

        let template = synthetic(base, "toolchain-hash");
        fixture::rewrite_json_value(&template.root.join(TEMPLATE_RECORD), &{
            let mut value = fixture::read_json(&template.root.join(TEMPLATE_RECORD))?;
            value["toolchain_lock_sha256"] = serde_json::json!("0".repeat(64));
            value
        })?;
        expect_invalid("toolchain lock hash", &template.root, base);

        let template = synthetic(base, "binary-bytes");
        fixture::rewrite_payload_member(
            &template.root,
            "runtime/native/wukong-worker.exe",
            b"MZ tampered worker",
        )?;
        expect_invalid("runtime binary bytes", &template.root, base);

        let template = synthetic(base, "binary-record");
        fixture::rewrite_json_value(&template.root.join(TEMPLATE_RECORD), &{
            let mut value = fixture::read_json(&template.root.join(TEMPLATE_RECORD))?;
            value["binaries"]["wukong-worker.exe"]["sha256"] = serde_json::json!("0".repeat(64));
            value
        })?;
        expect_invalid("binary record", &template.root, base);

        let template = synthetic(base, "native-source");
        fixture::rewrite_json_value(&template.root.join(NATIVE_BUILD), &{
            let mut value = fixture::read_json(&template.root.join(NATIVE_BUILD))?;
            value["source_revision"] = serde_json::json!("f".repeat(40));
            value
        })?;
        expect_invalid("native build source", &template.root, base);

        let template = synthetic(base, "manifest-version");
        fixture::rewrite_manifest(&template.root, |manifest| {
            manifest.version = "0.0.1".into();
        })?;
        expect_invalid("manifest version", &template.root, base);

        let template = synthetic(base, "manifest-worker");
        fixture::rewrite_manifest(&template.root, |manifest| {
            let entry = manifest
                .files
                .get_mut(WORKER_MEMBER)
                .expect("fixture worker member");
            entry.size += 1;
        })?;
        expect_invalid("manifest worker identity", &template.root, base);

        let template = synthetic(base, "payload-tampered");
        fs::write(
            template.root.join("payload/runtime/python/python.exe"),
            b"MZ tampered python",
        )?;
        expect_invalid("payload bytes", &template.root, base);

        let template = synthetic(base, "payload-extra");
        fs::write(
            template.root.join("payload/runtime/python/unexpected.dll"),
            b"unexpected",
        )?;
        expect_invalid("payload extra member", &template.root, base);

        let template = synthetic(base, "payload-missing");
        fs::remove_file(template.root.join("payload/runtime/python/python.exe"))?;
        expect_invalid("payload missing member", &template.root, base);

        let template = synthetic(base, "root-extra");
        fs::write(template.root.join("unexpected.txt"), b"unexpected")?;
        expect_invalid("extra template root entry", &template.root, base);

        let template = synthetic(base, "native-copy");
        fixture::rewrite_payload_member(
            &template.root,
            "native-build.json",
            br#"{"schema":1,"platform":"windows-amd64","source_revision":"0123456789abcdef0123456789abcdef01234567","files":{}}"#,
        )?;
        expect_invalid("native build copies", &template.root, base);
        Ok(())
    }

    #[test]
    fn archive_traversal_members_are_rejected_before_extraction() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let archive_path = temporary.path().join("unsafe.zip");
        {
            use zip::{ZipWriter, write::SimpleFileOptions};
            let mut archive = ZipWriter::new(fs::File::create(&archive_path)?);
            archive.start_file("../escape.txt", SimpleFileOptions::default())?;
            archive.write_all(b"escape")?;
            archive.finish()?;
        }
        let workspace = new_workspace(temporary.path());
        let error = Template::open(&archive_path, &workspace).unwrap_err();
        assert!(
            format!("{error:#}").contains("package path")
                || format!("{error:#}").contains("invalid")
                || format!("{error:#}").contains("archive member"),
            "unexpected error: {error:#}"
        );
        assert!(!temporary.path().join("escape.txt").exists());
        Ok(())
    }
}
