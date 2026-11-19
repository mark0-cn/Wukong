//! Sealing, enrollment, media assembly and verification for offline worker media.
//!
//! This is the Rust replacement for `scripts/worker/media.py finalize` plus
//! `scripts/worker/bundle.py prepare`: it seals the staged payload with the
//! controller signing identity, signs the enrollment, assembles the public
//! `WukongWorkerSetup.exe` media, re-verifies every hash and publishes the media
//! directory only after verification succeeded.
use super::template::{Template, canonical_semver, lowercase_hex};
use super::{
    ENROLLMENT_DIR, ENROLLMENT_RECORD, ENROLLMENT_SIGNATURE, FinalizeMediaRequest, FinalizedMedia,
    MANIFEST_RECORD, MAX_MEDIA_BYTES, MAX_MEDIA_ENTRIES, MEDIA_ALLOWLIST, MEDIA_RECORD,
    MEDIA_SCHEMA, MediaRecord, PAYLOAD_DIR, RUNTIME_PROTOCOL, SETUP_EXECUTABLE, SETUP_RECORD,
    Workspace, copy_tree, copy_verified_file, ensure_output_path_fits, ensure_staged_path_fits,
    overlaps, public_key_path, publish_directory, read_bounded, relative_path, sha256_bytes,
    sha256_file, validate_absolute_path, write_pretty_json,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, net::Ipv4Addr, path::Path, process::Command};
use wukong_worker_package::{
    Compatibility, Enrollment, ExecutionBoundary, SignatureRecord, Signer, SshPublicKey,
    VerifiedEnrollment, load_controller_signer, seal_package, verify_package, write_enrollment,
};

#[derive(Serialize)]
struct SetupRecordWrite<'a> {
    schema: u32,
    controller_public_key: &'a str,
    controller_version: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupRecordFile {
    schema: u32,
    controller_public_key: String,
    controller_version: String,
}

/// Generates the controller-managed SSH client identity from the template.
pub(crate) trait SshKeygen {
    /// Run the packaged key generator and return the canonical public key.
    fn generate(&self, keygen: &Path, private: &Path) -> Result<SshPublicKey>;
}

pub(crate) struct TemplateSshKeygen;

impl SshKeygen for TemplateSshKeygen {
    fn generate(&self, keygen: &Path, private: &Path) -> Result<SshPublicKey> {
        use std::os::windows::process::CommandExt;
        ensure!(
            keygen.is_file(),
            "offline media template omits the packaged ssh-keygen executable"
        );
        let output = Command::new(keygen)
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(private)
            .creation_flags(0x08000000)
            .output()
            .with_context(|| {
                format!(
                    "run the packaged ssh-keygen executable {}",
                    keygen.display()
                )
            })?;
        if !output.status.success() {
            bail!(
                "packaged ssh-keygen.exe failed{}",
                process_diagnostic(&output)
            );
        }
        ensure!(
            fs::symlink_metadata(private)?.is_file(),
            "packaged ssh-keygen.exe produced no private identity"
        );
        let bytes = read_bounded(&public_key_path(private), 4096, "generated SSH public key")?;
        let text = String::from_utf8(bytes).context("generated SSH public key is not UTF-8")?;
        let mut fields = text.split_whitespace();
        let algorithm = fields
            .next()
            .context("generated SSH public key is empty")?
            .to_owned();
        let key = fields
            .next()
            .context("generated SSH public key is incomplete")?
            .to_owned();
        let key = SshPublicKey { algorithm, key };
        ensure!(
            key.algorithm == "ssh-ed25519",
            "only canonical Ed25519 SSH public keys are supported"
        );
        key.fingerprint()
            .context("invalid generated SSH public key")?;
        Ok(key)
    }
}

fn process_diagnostic(output: &std::process::Output) -> String {
    let text = format!(
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .replace(['\r', '\n'], " ");
    let text = text.trim();
    if text.is_empty() {
        String::new()
    } else {
        format!(": {}", text.chars().take(1024).collect::<String>())
    }
}

/// Finalize one template into a new media directory.
///
/// # Errors
/// Returns an error when any path is relative or overlapping, the template
/// fails validation, the controller key directory is unsafe, sealing or signing
/// fails, the SSH identity cannot be generated, the assembled media fails its
/// hash/contract verification, or the output already exists.
pub fn finalize_media(request: &FinalizeMediaRequest) -> Result<FinalizedMedia> {
    finalize_media_with_keygen(request, &TemplateSshKeygen)
}

pub(crate) fn finalize_media_with_keygen(
    request: &FinalizeMediaRequest,
    keygen: &dyn SshKeygen,
) -> Result<FinalizedMedia> {
    validate_request(request)?;
    let output_parent = request.output.parent().map(Path::to_path_buf);
    let workspace = Workspace::create(output_parent.as_deref())?;
    let template = Template::open(&request.template, &workspace)?;
    ensure_output_path_fits(&request.output, template.longest_payload_member(), "output")?;
    ensure_staged_path_fits(workspace.root(), template.longest_payload_member())?;
    ensure!(
        !request.output.exists(),
        "offline media output already exists: {}",
        request.output.display()
    );
    ensure!(
        !overlaps(&request.keys, template.root()),
        "controller key directory must stay outside the offline media template"
    );
    ensure!(
        !overlaps(&request.keys, &request.output),
        "controller key directory must stay outside the offline media output"
    );
    ensure!(
        !overlaps(&request.output, template.root()),
        "offline media output must stay outside the offline media template"
    );
    if let Some(private) = &request.generate_ssh_key {
        ensure!(
            !overlaps(private, template.root())
                && !overlaps(private, &request.output)
                && !overlaps(private, &request.keys),
            "generated SSH private key must stay outside the template, output and controller key directory"
        );
    }
    let media_version =
        resolve_media_version(request.controller_version.as_deref(), &template.version)?;
    let signer =
        load_controller_signer(&request.keys).context("load the controller signing identity")?;
    let ssh_public_key = match (&request.ssh_public_key, &request.generate_ssh_key) {
        (Some(key), None) => key.clone(),
        (None, Some(private)) => {
            let keygen_path = template.payload_path(&template.manifest.entrypoints.ssh_keygen);
            keygen.generate(&keygen_path, private)?
        }
        (Some(_), Some(_)) => bail!(
            "offline media finalize accepts either an SSH public key or a generated SSH identity, not both"
        ),
        (None, None) => {
            bail!("offline media finalize needs an SSH public key or a generated SSH identity path")
        }
    };
    ensure!(
        ssh_public_key.algorithm == "ssh-ed25519",
        "only canonical Ed25519 SSH public keys are supported"
    );
    ssh_public_key
        .fingerprint()
        .context("invalid offline media SSH public key")?;

    let payload = workspace.path(PAYLOAD_DIR);
    fs::create_dir(&payload)?;
    copy_payload(&template, &payload)?;
    let compatibility = Compatibility {
        runtime_protocol: RUNTIME_PROTOCOL,
        controller_version: &media_version,
    };
    let sealed = seal_package(&payload, &template.manifest, &signer, &compatibility)
        .context("seal the offline worker payload")?;
    let enrollment = Enrollment {
        schema: 1,
        generation: request.generation,
        installation_id: request.installation_id.clone(),
        environment_id: request.environment_id.clone(),
        controller_public_key: signer.public_key().to_hex(),
        payload_manifest_sha256: sealed.sha256().to_owned(),
        management_peer: std::net::IpAddr::V4(request.management_peer),
        ssh_port: request.ssh_port,
        ssh_public_key: ssh_public_key.clone(),
        execution_boundary: ExecutionBoundary::DisposableVm,
    };
    let enrollment_dir = workspace.path(ENROLLMENT_DIR);
    write_enrollment(&enrollment_dir, &enrollment, &signer, &sealed)
        .context("sign the offline media enrollment")?;

    let media = workspace.path("media");
    let record = assemble_media(
        &media,
        &MediaAssembly {
            payload: &payload,
            enrollment_dir: &enrollment_dir,
            template: &template,
            sealed: &sealed,
            signer: &signer,
            media_version: &media_version,
            installation_id: &request.installation_id,
            environment_id: &request.environment_id,
            generation: request.generation,
        },
    )?;
    let private_names = private_key_names(&request.keys, request.generate_ssh_key.as_deref())?;
    verify_finalized_media(
        &media,
        &record,
        &signer,
        &template,
        &compatibility,
        &private_names,
    )?;
    publish_directory(&media, &request.output)?;
    Ok(FinalizedMedia {
        record,
        media_path: request.output.clone(),
        template_sha256: template.template_sha256.clone(),
        ssh_public_key,
        ssh_identity_file: request.generate_ssh_key.clone(),
    })
}

fn validate_request(request: &FinalizeMediaRequest) -> Result<()> {
    validate_absolute_path(&request.template, "template")?;
    validate_absolute_path(&request.keys, "controller key directory")?;
    validate_absolute_path(&request.output, "output")?;
    ensure!(
        request.template.exists(),
        "offline media template is missing: {}",
        request.template.display()
    );
    ensure!(
        !request.output.exists(),
        "offline media output already exists: {}",
        request.output.display()
    );
    ensure!(
        lowercase_hex(&request.installation_id, 32),
        "offline media installation identity must be 32 lowercase hex characters"
    );
    crate::adapters::environments::validate_id(&request.environment_id)?;
    ensure!(
        request.generation >= 1,
        "offline media enrollment generation must be positive"
    );
    ensure!(
        request.ssh_port > 0,
        "offline media SSH port must be positive"
    );
    ensure!(
        !request.management_peer.is_unspecified()
            && !request.management_peer.is_loopback()
            && !request.management_peer.is_multicast()
            && !request.management_peer.is_broadcast(),
        "invalid offline media management peer address"
    );
    match (&request.ssh_public_key, &request.generate_ssh_key) {
        (Some(key), None) => {
            ensure!(
                key.algorithm == "ssh-ed25519",
                "only canonical Ed25519 SSH public keys are supported"
            );
            key.fingerprint()
                .context("invalid offline media SSH public key")?;
        }
        (None, Some(private)) => {
            validate_absolute_path(private, "generated SSH private key")?;
            ensure!(
                !private.exists() && !public_key_path(private).exists(),
                "generated SSH private key output already exists"
            );
            ensure!(
                private.parent().is_some_and(Path::is_dir),
                "generated SSH private key parent directory is missing"
            );
        }
        _ => bail!("provide exactly one of an SSH public key or a generated SSH identity path"),
    }
    Ok(())
}

fn resolve_media_version(override_version: Option<&str>, template_version: &str) -> Result<String> {
    let template = semver::Version::parse(template_version)
        .context("invalid offline media template version")?;
    let version = match override_version {
        Some(value) => canonical_semver(value, "controller version")?,
        None => template_version.to_owned(),
    };
    let parsed = semver::Version::parse(&version).context("invalid controller version")?;
    ensure!(
        parsed >= template,
        "controller version {version} is older than the offline media template {template_version}"
    );
    Ok(version)
}

fn copy_payload(template: &Template, destination: &Path) -> Result<()> {
    for (relative, expected) in &template.manifest.files {
        let source = template.payload_path(relative);
        let target = destination.join(relative_path(relative));
        let sha256 = copy_verified_file(&source, &target, Some(&expected.sha256))?;
        ensure!(
            sha256 == expected.sha256,
            "offline media payload member changed while copying: {relative}"
        );
        ensure!(
            fs::metadata(&target)?.len() == expected.size,
            "offline media payload member size changed while copying: {relative}"
        );
    }
    Ok(())
}

struct MediaAssembly<'a> {
    payload: &'a Path,
    enrollment_dir: &'a Path,
    template: &'a Template,
    sealed: &'a wukong_worker_package::VerifiedManifest,
    signer: &'a Signer,
    media_version: &'a str,
    installation_id: &'a str,
    environment_id: &'a str,
    generation: u64,
}

fn assemble_media(media: &Path, assembly: &MediaAssembly<'_>) -> Result<MediaRecord> {
    fs::create_dir(media)?;
    copy_tree(assembly.payload, &media.join(PAYLOAD_DIR))?;
    let media_enrollment = media.join(ENROLLMENT_DIR);
    fs::create_dir(&media_enrollment)?;
    for name in [ENROLLMENT_RECORD, ENROLLMENT_SIGNATURE] {
        copy_verified_file(
            &assembly.enrollment_dir.join(name),
            &media_enrollment.join(name),
            None,
        )?;
    }
    let worker_relative = assembly.template.manifest.entrypoints.worker.clone();
    let worker_entry = assembly
        .template
        .manifest
        .files
        .get(&worker_relative)
        .context("offline media payload omits the worker executable")?;
    let setup_sha256 = copy_verified_file(
        &media
            .join(PAYLOAD_DIR)
            .join(relative_path(&worker_relative)),
        &media.join(SETUP_EXECUTABLE),
        Some(&worker_entry.sha256),
    )?;
    ensure!(
        setup_sha256 == assembly.template.worker_sha256()?,
        "offline media setup executable differs from the template worker"
    );
    let public_key = assembly.signer.public_key().to_hex();
    let setup = SetupRecordWrite {
        schema: 1,
        controller_public_key: &public_key,
        controller_version: assembly.media_version,
    };
    write_pretty_json(&media.join(SETUP_RECORD), &setup)?;
    let enrollment_bytes = read_bounded(
        &media_enrollment.join(ENROLLMENT_RECORD),
        64 * 1024,
        "offline media enrollment record",
    )?;
    let record = MediaRecord {
        schema: MEDIA_SCHEMA,
        version: assembly.media_version.to_owned(),
        controller_public_key: public_key,
        manifest_sha256: assembly.sealed.sha256().to_owned(),
        setup_sha256,
        enrollment_sha256: sha256_bytes(&enrollment_bytes),
        installation_id: assembly.installation_id.to_owned(),
        environment_id: assembly.environment_id.to_owned(),
        generation: assembly.generation,
        source_revision: assembly.template.manifest.source_revision.clone(),
    };
    write_pretty_json(&media.join(MEDIA_RECORD), &record)?;
    Ok(record)
}

fn private_key_names(keys: &Path, generated: Option<&Path>) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::from(["signing-key.bin".to_owned()]);
    if keys.is_dir() {
        for entry in fs::read_dir(keys)? {
            if let Some(name) = entry?.file_name().to_str() {
                names.insert(name.to_owned());
            }
        }
    }
    if let Some(private) = generated {
        let name = private
            .file_name()
            .and_then(|name| name.to_str())
            .context("generated SSH private key has no file name")?;
        names.insert(name.to_owned());
    }
    Ok(names)
}

fn verify_finalized_media(
    media: &Path,
    record: &MediaRecord,
    signer: &Signer,
    template: &Template,
    compatibility: &Compatibility<'_>,
    private_names: &BTreeSet<String>,
) -> Result<()> {
    ensure!(
        media.is_dir(),
        "offline media finalize produced no media directory"
    );
    let expected: BTreeSet<String> = MEDIA_ALLOWLIST
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let mut observed = BTreeSet::new();
    for entry in fs::read_dir(media)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("offline media entry name is not UTF-8"))?;
        ensure!(
            expected.contains(name.as_str()),
            "offline media layout differs from the supported set: {name}"
        );
        observed.insert(name);
    }
    ensure!(
        observed == expected,
        "offline media layout differs from the supported set"
    );
    let mut enrollment_entries = BTreeSet::new();
    for entry in fs::read_dir(media.join(ENROLLMENT_DIR))? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("offline media enrollment entry is not UTF-8"))?;
        enrollment_entries.insert(name);
    }
    ensure!(
        enrollment_entries
            == BTreeSet::from([
                ENROLLMENT_RECORD.to_owned(),
                ENROLLMENT_SIGNATURE.to_owned()
            ]),
        "offline media enrollment records are incomplete"
    );
    let files = super::collect_files(media, MAX_MEDIA_ENTRIES)?;
    let mut total = 0u64;
    for file in &files {
        total = total
            .checked_add(file.size)
            .context("offline media size overflow")?;
        ensure!(
            total <= MAX_MEDIA_BYTES,
            "offline media exceeds the 4 GiB provisioning budget"
        );
        let name = Path::new(&file.relative)
            .file_name()
            .and_then(|name| name.to_str())
            .context("offline media member has no file name")?;
        ensure!(
            !private_names.contains(name),
            "private key material must stay outside offline media: {name}"
        );
    }
    let record_bytes = read_bounded(
        &media.join(MEDIA_RECORD),
        1024 * 1024,
        "offline media receipt",
    )?;
    let parsed: MediaRecord =
        serde_json::from_slice(&record_bytes).context("parse offline media receipt")?;
    ensure!(
        &parsed == record,
        "offline media receipt differs from the finalized media"
    );
    ensure!(
        sha256_file(&media.join(SETUP_EXECUTABLE))? == record.setup_sha256,
        "offline media setup executable changed"
    );
    ensure!(
        sha256_file(&media.join(ENROLLMENT_DIR).join(ENROLLMENT_RECORD))?
            == record.enrollment_sha256,
        "offline media enrollment record changed"
    );
    ensure!(
        sha256_file(&media.join(PAYLOAD_DIR).join(MANIFEST_RECORD))? == record.manifest_sha256,
        "offline media payload manifest changed"
    );
    ensure!(
        record.setup_sha256 == template.worker_sha256()?,
        "offline media setup executable differs from the staged worker"
    );
    let setup: SetupRecordFile = serde_json::from_slice(&read_bounded(
        &media.join(SETUP_RECORD),
        1024 * 1024,
        "offline media setup record",
    )?)
    .context("parse offline media setup record")?;
    ensure!(
        setup.schema == 1
            && setup.controller_public_key == record.controller_public_key
            && setup.controller_version == record.version,
        "offline media setup record differs from the receipt"
    );
    let public_key = signer.public_key();
    let enrollment_bytes = read_bounded(
        &media.join(ENROLLMENT_DIR).join(ENROLLMENT_RECORD),
        64 * 1024,
        "offline media enrollment record",
    )?;
    let signature: SignatureRecord = serde_json::from_slice(&read_bounded(
        &media.join(ENROLLMENT_DIR).join(ENROLLMENT_SIGNATURE),
        16 * 1024,
        "offline media enrollment signature",
    )?)
    .context("parse offline media enrollment signature")?;
    let verified = VerifiedEnrollment::verify(&enrollment_bytes, &signature, &public_key)
        .context("offline media enrollment signature is invalid")?;
    let enrollment = verified.enrollment();
    ensure!(
        enrollment.schema == 1
            && enrollment.installation_id == record.installation_id
            && enrollment.environment_id == record.environment_id
            && enrollment.generation == record.generation
            && enrollment.controller_public_key == record.controller_public_key
            && enrollment.payload_manifest_sha256 == record.manifest_sha256
            && enrollment.execution_boundary == ExecutionBoundary::DisposableVm,
        "offline media enrollment differs from the receipt"
    );
    let payload = verify_package(&media.join(PAYLOAD_DIR), &public_key, compatibility)
        .context("offline media payload verification failed")?;
    verified
        .bind_payload(&payload)
        .context("offline media enrollment and payload differ")?;
    ensure!(
        payload.manifest().source_revision == record.source_revision,
        "offline media payload source revision differs from the receipt"
    );
    let worker_relative = template.manifest.entrypoints.worker.clone();
    ensure!(
        sha256_file(
            &media
                .join(PAYLOAD_DIR)
                .join(relative_path(&worker_relative))
        )? == record.setup_sha256,
        "offline media setup executable differs from the sealed worker"
    );
    Ok(())
}

/// First port of the controller-managed SSH range.
pub(crate) const SSH_PORT_FIRST: u16 = 22_220;
/// Last port of the controller-managed SSH range.
pub(crate) const SSH_PORT_LAST: u16 = 22_299;

/// Pick a random port from the controller-managed range, preferring one the host
/// does not currently bind (`free-ish`); an explicit operator port bypasses this.
pub(crate) fn choose_ssh_port(random: u128, available: impl Fn(u16) -> bool) -> Result<u16> {
    let count = u32::from(SSH_PORT_LAST - SSH_PORT_FIRST) + 1;
    let start = u16::try_from(random % u128::from(count)).unwrap_or(0);
    for offset in 0..count {
        let position = u16::try_from((u32::from(start) + offset) % count).unwrap_or(0);
        let port = SSH_PORT_FIRST + position;
        if available(port) {
            return Ok(port);
        }
    }
    bail!("no free SSH port in {SSH_PORT_FIRST}..={SSH_PORT_LAST}; pass an explicit SSH port")
}

pub(crate) fn host_port_available(port: u16) -> bool {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::worker_media::fixture::{self, FixedKeygen};
    use crate::application::worker_media::template::APP_VERSION;
    use std::path::PathBuf;

    fn synthetic_template(temporary: &Path) -> PathBuf {
        let root = temporary.join("template");
        fs::create_dir(&root).expect("fixture template directory");
        fixture::synthetic_template(&root)
            .expect("synthetic template")
            .root
    }

    fn request(template: &Path, keys: &Path, output: &Path) -> FinalizeMediaRequest {
        FinalizeMediaRequest {
            template: template.to_path_buf(),
            keys: keys.to_path_buf(),
            output: output.to_path_buf(),
            installation_id: "958ef753fc7646598935ce8f79c53940".into(),
            environment_id: "windows-vm".into(),
            generation: 3,
            management_peer: "192.168.199.1".parse().expect("management peer"),
            ssh_port: 22_268,
            ssh_public_key: Some(fixture::ssh_public_key(7)),
            generate_ssh_key: None,
            controller_version: None,
        }
    }

    #[test]
    fn ssh_port_selection_stays_in_the_controller_managed_range() {
        assert_eq!(
            choose_ssh_port(0, |_| true).expect("first port"),
            SSH_PORT_FIRST
        );
        let port = choose_ssh_port(u128::MAX, |_| true).expect("random port");
        assert!((SSH_PORT_FIRST..=SSH_PORT_LAST).contains(&port));
        assert_eq!(
            choose_ssh_port(0, |port| port == 22_240).expect("preferred port"),
            22_240
        );
        assert!(choose_ssh_port(7, |_| false).is_err());
    }

    #[test]
    fn media_version_defaults_to_the_template_and_never_goes_backwards() {
        assert_eq!(
            resolve_media_version(None, APP_VERSION).expect("default"),
            APP_VERSION
        );
        assert_eq!(
            resolve_media_version(Some("2.0.0"), APP_VERSION).expect("newer controller"),
            "2.0.0"
        );
        assert!(resolve_media_version(Some("0.0.1"), APP_VERSION).is_err());
        assert!(resolve_media_version(Some("2.0"), APP_VERSION).is_err());
    }

    #[test]
    fn finalize_validates_the_ssh_identity_mode() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let template = synthetic_template(temporary.path());
        let keys = temporary.path().join("keys");
        let output = temporary.path().join("media");
        let mut both = request(&template, &keys, &output);
        both.generate_ssh_key = Some(temporary.path().join("identity/management_ed25519"));
        assert!(validate_request(&both).is_err());
        let mut neither = request(&template, &keys, &output);
        neither.ssh_public_key = None;
        assert!(validate_request(&neither).is_err());
        let mut existing = request(&template, &keys, &output);
        existing.ssh_public_key = None;
        fs::create_dir_all(temporary.path().join("identity")).expect("identity directory");
        fs::write(
            temporary.path().join("identity/management_ed25519"),
            b"existing",
        )
        .expect("existing identity");
        existing.generate_ssh_key = Some(temporary.path().join("identity/management_ed25519"));
        assert!(validate_request(&existing).is_err());
        assert!(validate_request(&request(&template, &keys, &output)).is_ok());
    }

    #[test]
    fn finalize_supplies_the_offline_provisioning_media_contract() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let template = synthetic_template(temporary.path());
        let keys = temporary.path().join("controller");
        let output = temporary.path().join("media");
        let finalized = finalize_media(&request(&template, &keys, &output))?;
        assert_eq!(finalized.record.version, APP_VERSION);
        assert_eq!(
            finalized.record.controller_public_key,
            wukong_worker_package::load_controller_signer(&keys)?
                .public_key()
                .to_hex()
        );
        assert_eq!(
            finalized.record.installation_id,
            "958ef753fc7646598935ce8f79c53940"
        );
        assert_eq!(finalized.record.environment_id, "windows-vm");
        assert_eq!(finalized.record.generation, 3);
        assert!(finalized.record.manifest_sha256.len() == 64);
        assert!(output.join("media.json").is_file());
        let identity = crate::application::windows_lab::provision::verify_media(&output)
            .context("provisioning must accept the finalized media")?;
        assert_eq!(identity.manifest_sha256, finalized.record.manifest_sha256);
        assert_eq!(identity.installation_id, finalized.record.installation_id);
        assert_eq!(identity.ssh_port, 22_268);
        assert!(finalized.ssh_identity_file.is_none());
        Ok(())
    }

    #[test]
    fn finalize_generates_a_private_identity_outside_the_media() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let template = synthetic_template(temporary.path());
        let keys = temporary.path().join("controller");
        let output = temporary.path().join("media");
        let identity = temporary.path().join("ssh/management_ed25519");
        fs::create_dir_all(identity.parent().expect("identity parent"))?;
        let mut request = request(&template, &keys, &output);
        request.ssh_public_key = None;
        request.generate_ssh_key = Some(identity.clone());
        let finalized = finalize_media_with_keygen(&request, &FixedKeygen { fill: 9 })?;
        assert!(identity.is_file());
        assert!(public_key_path(&identity).is_file());
        assert_eq!(
            finalized.ssh_public_key.key,
            fixture::ssh_public_key(9).key,
            "media enrollment must carry the generated public key"
        );
        assert!(output.join("media.json").is_file());
        assert!(
            !output
                .join("enrollment")
                .join("management_ed25519")
                .exists(),
            "the private identity must never enter the media"
        );
        crate::application::windows_lab::provision::verify_media(&output)?;
        Ok(())
    }

    #[test]
    fn finalize_refuses_existing_output_key_overlap_and_bad_specs() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let template = synthetic_template(temporary.path());
        let keys = temporary.path().join("controller");
        let output = temporary.path().join("media");
        fs::create_dir(&output)?;
        let error = finalize_media(&request(&template, &keys, &output)).unwrap_err();
        assert!(format!("{error:#}").contains("already exists"));
        fs::remove_dir(&output)?;
        let keys_inside = template.join("controller-keys");
        let error = finalize_media(&request(&template, &keys_inside, &output)).unwrap_err();
        assert!(format!("{error:#}").contains("outside"));
        let mut bad = request(&template, &keys, &output);
        bad.installation_id = "not-an-identity".into();
        assert!(validate_request(&bad).is_err());
        bad = request(&template, &keys, &output);
        bad.environment_id = "bad environment".into();
        assert!(validate_request(&bad).is_err());
        bad = request(&template, &keys, &output);
        bad.management_peer = "127.0.0.1".parse()?;
        assert!(validate_request(&bad).is_err());
        Ok(())
    }
}
