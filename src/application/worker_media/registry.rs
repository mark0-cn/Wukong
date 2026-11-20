//! Generation registry for finalized offline worker media.
//!
//! Each `wukong worker media generate` run publishes one immutable generation
//! directory `<registry>/<installation_id>/` (SSH private identity and finalized
//! media) plus a schema-1 record `<registry>/<installation_id>.json`. The record
//! is what the desktop application and later provisioning steps consume; private
//! keys stay in the generation directory and never enter the media or the ZIP.
use super::finalize::{
    SshKeygen, TemplateSshKeygen, choose_ssh_port, finalize_media_with_keygen, host_port_available,
};
use super::template::{APP_VERSION, canonical_semver, lowercase_hex};
use super::{
    FinalizeMediaRequest, MEDIA_RECORD, overlaps, read_bounded, reject_link, sha256_file,
    validate_absolute_path, write_directory_zip,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

/// Management network prefix length recorded for the desktop VM network.
pub const MANAGED_NETWORK_PREFIX_LEN: u8 = 24;
/// Schema of the generation registry record.
pub const GENERATION_SCHEMA: u32 = 1;
const DEFAULT_ENVIRONMENT_ID: &str = "windows";
const DEFAULT_MANAGEMENT_PEER: Ipv4Addr = Ipv4Addr::new(192, 168, 199, 1);
const MAX_RECORD_BYTES: usize = 64 * 1024;

/// One finalized media generation. All paths are absolute and controller-local;
/// the record never contains private key bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationRecord {
    pub schema: u32,
    pub installation_id: String,
    pub environment_id: String,
    pub app_version: String,
    pub template_sha256: String,
    pub payload_manifest_sha256: String,
    pub controller_public_key: String,
    pub ssh_identity_file: PathBuf,
    pub management_peer: Ipv4Addr,
    pub network_prefix_len: u8,
    pub ssh_port: u16,
    pub media_path: PathBuf,
    pub zip_path: PathBuf,
    pub created_at: String,
}

impl GenerationRecord {
    /// Validate the schema invariants without touching the filesystem.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == GENERATION_SCHEMA,
            "unsupported offline worker media generation record"
        );
        ensure!(
            lowercase_hex(&self.installation_id, 32),
            "generation record installation identity is invalid"
        );
        crate::adapters::environments::validate_id(&self.environment_id)?;
        canonical_semver(&self.app_version, "generation record app version")?;
        ensure!(
            lowercase_hex(&self.template_sha256, 64)
                && lowercase_hex(&self.payload_manifest_sha256, 64)
                && lowercase_hex(&self.controller_public_key, 64),
            "generation record contains invalid hashes"
        );
        for (label, path) in [
            ("SSH identity", &self.ssh_identity_file),
            ("media", &self.media_path),
            ("archive", &self.zip_path),
        ] {
            validate_absolute_path(path, label)?;
        }
        ensure!(
            !self.management_peer.is_unspecified()
                && !self.management_peer.is_loopback()
                && !self.management_peer.is_multicast()
                && !self.management_peer.is_broadcast(),
            "generation record management peer is invalid"
        );
        ensure!(
            (1..=32).contains(&self.network_prefix_len),
            "generation record network prefix is invalid"
        );
        ensure!(self.ssh_port > 0, "generation record SSH port is invalid");
        chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .context("generation record creation timestamp is invalid")?;
        ensure!(
            self.ssh_identity_file
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "management_ed25519"),
            "generation record SSH identity file name is unsupported"
        );
        ensure!(
            self.media_path.file_name().and_then(|name| name.to_str()) == Some("media"),
            "generation record media path must name the generation media directory"
        );
        Ok(())
    }
}

/// Request for one generation.
#[derive(Clone, Debug)]
pub struct GenerateRequest {
    /// Released template ZIP archive or unpacked template directory.
    pub template: PathBuf,
    /// Requested controller-credential-free media ZIP (must not exist).
    pub output_zip: PathBuf,
    /// Directory that receives the generation record, identity and media.
    pub registry: PathBuf,
    /// Absolute controller signing key directory (outside template and registry).
    pub keys: PathBuf,
    /// Enrollment environment id; defaults to `windows`.
    pub environment_id: Option<String>,
    /// Worker management peer (controller address as the guest sees it).
    pub management_peer: Option<Ipv4Addr>,
    /// Fixed SSH port; otherwise a random free-ish port in 22220..=22299.
    pub ssh_port: Option<u16>,
    /// Media/controller version override; defaults to the template version.
    pub controller_version: Option<String>,
    /// Pins the installation id (tests and scripted regeneration); otherwise a
    /// fresh uuid4 identity is generated.
    pub installation_id: Option<String>,
}

/// Outcome of one generation, including the published registry record.
#[derive(Debug, Serialize)]
pub struct GenerateOutcome {
    pub generated: bool,
    pub record_path: PathBuf,
    pub media_record_sha256: String,
    pub zip_sha256: String,
    pub zip_size: u64,
    pub zip_entries: usize,
    pub record: GenerationRecord,
}

/// Generate one finalized media directory, deterministic ZIP and registry
/// record from a released template.
///
/// # Errors
/// Returns an error when any path is relative, overlapping or already present,
/// the template or controller key directory is invalid, sealing/signing fails,
/// the SSH identity cannot be generated, media verification fails, or the ZIP or
/// record cannot be published. A failed run removes its own generation directory
/// and archive.
pub fn generate(request: &GenerateRequest) -> Result<GenerateOutcome> {
    generate_with(request, &TemplateSshKeygen)
}

pub(crate) fn generate_with(
    request: &GenerateRequest,
    keygen: &dyn SshKeygen,
) -> Result<GenerateOutcome> {
    validate_absolute_path(&request.template, "template")?;
    validate_absolute_path(&request.output_zip, "archive")?;
    validate_absolute_path(&request.registry, "registry")?;
    validate_absolute_path(&request.keys, "controller key directory")?;
    ensure!(
        request.template.exists(),
        "offline media template is missing: {}",
        request.template.display()
    );
    ensure!(
        !request.output_zip.exists(),
        "offline media archive already exists: {}",
        request.output_zip.display()
    );
    let template_metadata = reject_link(&request.template)?;
    if template_metadata.is_dir() {
        for (label, path) in [
            ("registry", &request.registry),
            ("controller key directory", &request.keys),
            ("archive", &request.output_zip),
        ] {
            ensure!(
                !overlaps(path, &request.template),
                "offline media {label} must stay outside the template"
            );
        }
    }
    ensure!(
        !overlaps(&request.keys, &request.registry),
        "controller key directory must stay outside the offline media registry"
    );
    let installation_id = match &request.installation_id {
        Some(identity) => {
            ensure!(
                lowercase_hex(identity, 32),
                "offline media installation identity must be 32 lowercase hex characters"
            );
            identity.clone()
        }
        None => uuid::Uuid::new_v4().simple().to_string(),
    };
    let environment_id = request
        .environment_id
        .clone()
        .unwrap_or_else(|| DEFAULT_ENVIRONMENT_ID.to_owned());
    crate::adapters::environments::validate_id(&environment_id)?;
    let management_peer = request.management_peer.unwrap_or(DEFAULT_MANAGEMENT_PEER);
    ensure!(
        !management_peer.is_unspecified()
            && !management_peer.is_loopback()
            && !management_peer.is_multicast()
            && !management_peer.is_broadcast(),
        "invalid offline media management peer address"
    );
    let ssh_port = match request.ssh_port {
        Some(port) => {
            ensure!(port > 0, "offline media SSH port must be positive");
            port
        }
        None => choose_ssh_port(uuid::Uuid::new_v4().as_u128(), host_port_available)?,
    };
    let generation_dir = generation_directory(&request.registry, &installation_id);
    let record_path = record_path(&request.registry, &installation_id);
    ensure!(
        !generation_dir.exists(),
        "offline worker media generation already exists: {}",
        generation_dir.display()
    );
    ensure!(
        !record_path.exists(),
        "offline worker media generation record already exists: {}",
        record_path.display()
    );
    ensure!(
        !overlaps(&request.output_zip, &generation_dir),
        "offline media archive must stay outside the generation directory"
    );
    ensure!(
        request.output_zip != record_path,
        "offline media archive must differ from the generation record"
    );
    fs::create_dir_all(&request.registry)?;
    ensure!(
        reject_link(&request.registry)?.is_dir(),
        "offline worker media registry must be a directory"
    );
    fs::create_dir(&generation_dir)?;
    let mut guard = GenerationGuard::new(generation_dir.clone());
    let ssh_dir = generation_dir.join("ssh");
    fs::create_dir(&ssh_dir)?;
    let ssh_identity_file = ssh_dir.join("management_ed25519");
    let finalize_request = FinalizeMediaRequest {
        template: request.template.clone(),
        keys: request.keys.clone(),
        output: generation_dir.join("media"),
        installation_id: installation_id.clone(),
        environment_id: environment_id.clone(),
        generation: 1,
        management_peer,
        ssh_port,
        ssh_public_key: None,
        generate_ssh_key: Some(ssh_identity_file.clone()),
        controller_version: request.controller_version.clone(),
    };
    let finalized = finalize_media_with_keygen(&finalize_request, keygen)?;
    let media_record_sha256 = sha256_file(&finalized.media_path.join(MEDIA_RECORD))?;
    let archive = write_directory_zip(&finalized.media_path, &request.output_zip, None)?;
    guard.zip_path = Some(request.output_zip.clone());
    let zip_sha256 = archive.sha256;
    let record = GenerationRecord {
        schema: GENERATION_SCHEMA,
        installation_id,
        environment_id,
        app_version: APP_VERSION.to_owned(),
        template_sha256: finalized.template_sha256,
        payload_manifest_sha256: finalized.record.manifest_sha256,
        controller_public_key: finalized.record.controller_public_key,
        ssh_identity_file,
        management_peer,
        network_prefix_len: MANAGED_NETWORK_PREFIX_LEN,
        ssh_port,
        media_path: finalized.media_path,
        zip_path: request.output_zip.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    record.validate()?;
    let record_path = write_record(&request.registry, &record)?;
    guard.disarm();
    Ok(GenerateOutcome {
        generated: true,
        record_path,
        media_record_sha256,
        zip_sha256,
        zip_size: archive.size,
        zip_entries: archive.entries,
        record,
    })
}

struct GenerationGuard {
    directory: PathBuf,
    zip_path: Option<PathBuf>,
    armed: bool,
}

impl GenerationGuard {
    fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            zip_path: None,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for GenerationGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(zip) = &self.zip_path {
            let _ = fs::remove_file(zip);
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}

pub(crate) fn generation_directory(registry: &Path, installation_id: &str) -> PathBuf {
    registry.join(installation_id)
}

pub(crate) fn record_path(registry: &Path, installation_id: &str) -> PathBuf {
    registry.join(format!("{installation_id}.json"))
}

/// Publish one generation record atomically. Never overwrites a record.
pub fn write_record(registry: &Path, record: &GenerationRecord) -> Result<PathBuf> {
    validate_absolute_path(registry, "registry")?;
    record.validate()?;
    fs::create_dir_all(registry)?;
    ensure!(
        reject_link(registry)?.is_dir(),
        "offline worker media registry must be a directory"
    );
    let path = record_path(registry, &record.installation_id);
    ensure!(
        !path.exists(),
        "offline worker media generation record already exists: {}",
        path.display()
    );
    let temporary = registry.join(format!(".generation-{}.json", random_hex(16)));
    struct Cleanup<'a>(&'a Path);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0);
        }
    }
    let cleanup = Cleanup(&temporary);
    let mut bytes = serde_json::to_vec_pretty(record)?;
    bytes.push(b'\n');
    super::write_new_file(&temporary, &bytes)?;
    fs::rename(&temporary, &path).context("publish the offline media generation record")?;
    drop(cleanup);
    Ok(path)
}

/// Read and validate one generation record written by [`write_record`].
pub fn read_record(path: &Path) -> Result<GenerationRecord> {
    validate_absolute_path(path, "generation record")?;
    let bytes = read_bounded(path, MAX_RECORD_BYTES, "generation record")?;
    let record: GenerationRecord =
        serde_json::from_slice(&bytes).context("parse offline media generation record")?;
    record.validate()?;
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("generation record has no file name")?;
    ensure!(
        stem == record.installation_id,
        "generation record file name differs from its installation identity"
    );
    Ok(record)
}

/// Read every generation record in one registry directory, sorted by identity.
pub fn read_generation_records(registry: &Path) -> Result<Vec<GenerationRecord>> {
    validate_absolute_path(registry, "registry")?;
    ensure!(
        registry.is_dir(),
        "offline worker media registry is missing: {}",
        registry.display()
    );
    let mut paths = Vec::new();
    for entry in fs::read_dir(registry)? {
        let entry = entry?;
        let metadata = reject_link(&entry.path())?;
        if metadata.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    paths.iter().map(|path| read_record(path)).collect()
}

fn random_hex(characters: usize) -> String {
    let value = uuid::Uuid::new_v4().simple().to_string();
    value[..characters.min(value.len())].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::worker_media::MEDIA_ALLOWLIST;
    use crate::application::worker_media::fixture::{self, FixedKeygen};
    use std::collections::BTreeSet;

    const INSTALLATION_ID: &str = "0123456789abcdef0123456789abcdef";

    fn synthetic_template(temporary: &Path) -> PathBuf {
        let root = temporary.join("template");
        fs::create_dir(&root).expect("fixture template directory");
        fixture::synthetic_template(&root)
            .expect("synthetic template")
            .root
    }

    fn request(temporary: &Path, installation_id: Option<&str>) -> GenerateRequest {
        GenerateRequest {
            template: synthetic_template(temporary),
            output_zip: temporary.join("media.zip"),
            registry: temporary.join("registry"),
            keys: temporary.join("controller"),
            environment_id: Some("windows-vm".into()),
            management_peer: Some("192.168.199.1".parse().expect("management peer")),
            ssh_port: Some(22_268),
            controller_version: None,
            installation_id: installation_id.map(str::to_owned),
        }
    }

    fn record_fixture(temporary: &Path) -> GenerationRecord {
        GenerationRecord {
            schema: GENERATION_SCHEMA,
            installation_id: INSTALLATION_ID.into(),
            environment_id: "windows-vm".into(),
            app_version: APP_VERSION.into(),
            template_sha256: "a".repeat(64),
            payload_manifest_sha256: "b".repeat(64),
            controller_public_key: "c".repeat(64),
            ssh_identity_file: temporary.join("generation/ssh/management_ed25519"),
            management_peer: "192.168.199.1".parse().expect("management peer"),
            network_prefix_len: MANAGED_NETWORK_PREFIX_LEN,
            ssh_port: 22_268,
            media_path: temporary.join("generation/media"),
            zip_path: temporary.join("media.zip"),
            created_at: "2026-01-01T00:00:00+00:00".into(),
        }
    }

    #[test]
    fn generation_records_round_trip_and_never_overwrite() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let registry = temporary.path().join("windows-worker-media");
        let record = record_fixture(temporary.path());
        let path = write_record(&registry, &record)?;
        assert_eq!(path, record_path(&registry, INSTALLATION_ID));
        assert_eq!(read_record(&path)?, record);
        assert_eq!(read_generation_records(&registry)?, vec![record.clone()]);
        assert!(
            write_record(&registry, &record).is_err(),
            "records are immutable"
        );
        let mut invalid = record.clone();
        invalid.ssh_port = 0;
        assert!(invalid.validate().is_err());
        invalid = record.clone();
        invalid.payload_manifest_sha256 = "not-a-hash".into();
        assert!(invalid.validate().is_err());
        invalid = record;
        invalid.media_path = PathBuf::from("relative/media");
        assert!(invalid.validate().is_err());
        Ok(())
    }

    #[test]
    fn generate_publishes_media_zip_identity_and_record() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let request = request(temporary.path(), Some(INSTALLATION_ID));
        let outcome = generate_with(&request, &FixedKeygen { fill: 11 })?;
        assert!(outcome.generated);
        assert_eq!(
            read_record(&outcome.record_path)?,
            outcome.record,
            "the published record must round-trip"
        );
        let record = &outcome.record;
        assert_eq!(record.installation_id, INSTALLATION_ID);
        assert_eq!(record.environment_id, "windows-vm");
        assert_eq!(record.management_peer, "192.168.199.1".parse::<Ipv4Addr>()?);
        assert_eq!(record.ssh_port, 22_268);
        assert_eq!(record.network_prefix_len, MANAGED_NETWORK_PREFIX_LEN);
        assert_eq!(
            record.ssh_identity_file,
            request
                .registry
                .join(INSTALLATION_ID)
                .join("ssh/management_ed25519")
        );
        assert!(record.ssh_identity_file.is_file());
        assert!(record.media_path.is_dir());
        assert_eq!(
            sha256_file(&record.media_path.join(MEDIA_RECORD))?,
            outcome.media_record_sha256
        );
        assert_eq!(sha256_file(&request.output_zip)?, outcome.zip_sha256);
        assert!(outcome.zip_size > 0);
        assert!(outcome.zip_entries >= 5);
        let media = crate::application::windows_lab::provision::verify_media(&record.media_path)?;
        assert_eq!(media.manifest_sha256, record.payload_manifest_sha256);
        assert_eq!(media.controller_public_key, record.controller_public_key);
        assert!(
            record.media_path.join("media.json").is_file()
                && record.media_path.join("WukongWorkerSetup.exe").is_file()
                && record.media_path.join("enrollment").is_dir()
                && record.media_path.join("payload").is_dir()
        );
        assert!(
            !record
                .media_path
                .join("payload")
                .join("management_ed25519")
                .exists(),
            "private identity must never enter the media"
        );
        let mut archive = zip::ZipArchive::new(fs::File::open(&request.output_zip)?)?;
        let mut names = BTreeSet::new();
        for index in 0..archive.len() {
            let entry = archive.by_index(index)?;
            names.insert(entry.name().context("zip member name")?.into_owned());
        }
        let expected: BTreeSet<String> = MEDIA_ALLOWLIST
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        let roots: BTreeSet<String> = names
            .iter()
            .map(|name| name.split('/').next().unwrap_or(name).to_owned())
            .collect();
        assert_eq!(
            roots, expected,
            "archive root must contain only the public media entries"
        );
        assert!(
            names
                .iter()
                .all(|name| !name.contains("management_ed25519") && !name.contains("signing-key"))
        );
        assert!(names.contains("payload/manifest.json"));
        assert!(names.contains("enrollment/enrollment.json"));
        Ok(())
    }

    #[test]
    fn generation_is_reproducible_for_the_same_inputs() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let first = request(temporary.path(), Some(INSTALLATION_ID));
        let first = GenerateRequest {
            registry: temporary.path().join("registry-one"),
            output_zip: temporary.path().join("one.zip"),
            ..first
        };
        let outcome = generate_with(&first, &FixedKeygen { fill: 11 })?;
        // A second generation shares the controller identity and generated SSH
        // key material, so every published byte must match except paths and the
        // creation timestamp inside the generation record.
        let second = GenerateRequest {
            registry: temporary.path().join("registry-two"),
            output_zip: temporary.path().join("two.zip"),
            ..first
        };
        let repeated = generate_with(&second, &FixedKeygen { fill: 11 })?;
        assert_eq!(outcome.zip_sha256, repeated.zip_sha256);
        assert_eq!(
            outcome.record.payload_manifest_sha256,
            repeated.record.payload_manifest_sha256
        );
        assert_eq!(
            outcome.record.controller_public_key,
            repeated.record.controller_public_key
        );
        assert_eq!(fs::read(&second.output_zip)?, fs::read(&first.output_zip)?);
        Ok(())
    }

    #[test]
    fn generate_fails_closed_on_existing_outputs_and_overlaps() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let request = request(temporary.path(), Some(INSTALLATION_ID));
        fs::write(&request.output_zip, b"existing archive")?;
        assert!(generate_with(&request, &FixedKeygen { fill: 11 }).is_err());
        fs::remove_file(&request.output_zip)?;
        fs::create_dir_all(&request.registry)?;
        fs::write(
            record_path(&request.registry, INSTALLATION_ID),
            b"existing record",
        )?;
        assert!(generate_with(&request, &FixedKeygen { fill: 11 }).is_err());
        fs::remove_dir_all(&request.registry)?;
        let mut keys_inside = request;
        keys_inside.keys = keys_inside.template.join("controller");
        assert!(generate_with(&keys_inside, &FixedKeygen { fill: 11 }).is_err());
        Ok(())
    }
}
