//! Offline Windows Worker media contracts and generation.
mod types;
pub use types::{FinalizeMediaRequest, FinalizedMedia, MediaRecord};

#[cfg(windows)]
mod files;
#[cfg(windows)]
mod finalize;
#[cfg(windows)]
mod registry;
#[cfg(windows)]
mod template;

#[cfg(windows)]
pub(crate) use files::*;
#[cfg(windows)]
pub use finalize::finalize_media;
#[cfg(windows)]
pub use registry::{
    GenerateOutcome, GenerateRequest, GenerationRecord, GenerationRecordScan,
    MANAGED_NETWORK_PREFIX_LEN, UnreadableGenerationRecord, generate, read_generation_records,
    scan_generation_records,
};
#[cfg(windows)]
pub use template::APP_VERSION;

#[cfg(windows)]
pub(crate) const MEDIA_SCHEMA: u32 = 1;
/// Worker package runtime protocol the released template must declare.
#[cfg(windows)]
pub(crate) const RUNTIME_PROTOCOL: u32 = 3;
#[cfg(windows)]
pub(crate) const MEDIA_RECORD: &str = "media.json";
#[cfg(windows)]
pub(crate) const SETUP_RECORD: &str = "setup.json";
#[cfg(windows)]
pub(crate) const SETUP_EXECUTABLE: &str = "WukongWorkerSetup.exe";
#[cfg(windows)]
pub(crate) const ENROLLMENT_DIR: &str = "enrollment";
#[cfg(windows)]
pub(crate) const ENROLLMENT_RECORD: &str = "enrollment.json";
#[cfg(windows)]
pub(crate) const ENROLLMENT_SIGNATURE: &str = "enrollment.sig";
#[cfg(windows)]
pub(crate) const PAYLOAD_DIR: &str = "payload";
#[cfg(windows)]
pub(crate) const MANIFEST_RECORD: &str = "manifest.json";
#[cfg(windows)]
pub(crate) const MEDIA_ALLOWLIST: [&str; 5] = [
    SETUP_EXECUTABLE,
    SETUP_RECORD,
    PAYLOAD_DIR,
    ENROLLMENT_DIR,
    MEDIA_RECORD,
];
#[cfg(windows)]
pub(crate) const MAX_MEDIA_ENTRIES: usize = 20_000;
#[cfg(windows)]
pub(crate) const MAX_MEDIA_BYTES: u64 = 4 * 1024 * 1024 * 1024;
#[cfg(windows)]
pub(crate) const MAX_TEMPLATE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[cfg(all(windows, test))]
pub(crate) mod fixture {
    use super::{public_key_path, sha256_bytes};
    use crate::application::worker_media::finalize::SshKeygen;
    use anyhow::{Context, Result};
    use base64::Engine;
    use serde_json::json;
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
    };
    use wukong_worker_package::{
        Component, EntryPoints, FileEntry, Manifest, PayloadKind, SshPublicKey,
    };

    pub(crate) const SOURCE_REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

    pub(crate) struct SyntheticTemplate {
        pub(crate) root: PathBuf,
        pub(crate) worker_sha256: String,
    }

    fn file_entry(sha256: &str, size: usize, kind: PayloadKind) -> FileEntry {
        FileEntry {
            sha256: sha256.to_owned(),
            size: u64::try_from(size).expect("synthetic payload size"),
            kind,
        }
    }

    pub(crate) fn ssh_public_key(fill: u8) -> SshPublicKey {
        let mut blob = vec![0, 0, 0, 11];
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&[0, 0, 0, 32]);
        blob.extend_from_slice(&[fill; 32]);
        SshPublicKey {
            algorithm: "ssh-ed25519".into(),
            key: base64::engine::general_purpose::STANDARD.encode(blob),
        }
    }

    /// Tests never run the packaged Windows `ssh-keygen.exe`; this writes one
    /// canonical public key and an opaque private file at the requested paths.
    pub(crate) struct FixedKeygen {
        pub(crate) fill: u8,
    }

    impl SshKeygen for FixedKeygen {
        fn generate(&self, _keygen: &Path, private: &Path) -> Result<SshPublicKey> {
            let key = ssh_public_key(self.fill);
            fs::write(private, b"synthetic-controller-private-identity")
                .context("write synthetic SSH private identity")?;
            fs::write(
                public_key_path(private),
                format!("{} {}\n", key.algorithm, key.key),
            )
            .context("write synthetic SSH public identity")?;
            Ok(key)
        }
    }

    fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes).with_context(|| format!("write fixture {}", path.display()))
    }

    fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(value)?;
        bytes.push(b'\n');
        write_bytes(path, &bytes)
    }

    pub(crate) fn synthetic_template(root: &Path) -> Result<SyntheticTemplate> {
        let worker = b"MZ synthetic wukong worker".to_vec();
        let package = b"MZ synthetic wukong package".to_vec();
        let toolchain = br#"{"synthetic":1}"#.to_vec();
        let native_build = serde_json::to_vec(&json!({
            "schema": 1,
            "platform": "windows-amd64",
            "source_revision": SOURCE_REVISION,
            "static_crt_checked": true,
            "files": {
                "wukong-worker.exe": {
                    "sha256": sha256_bytes(&worker),
                    "size": worker.len(),
                },
                "wukong-package.exe": {
                    "sha256": sha256_bytes(&package),
                    "size": package.len(),
                },
            },
        }))?;
        let payload: Vec<(&str, Vec<u8>, PayloadKind)> = vec![
            (
                "runtime/native/wukong-worker.exe",
                worker.clone(),
                PayloadKind::Runtime,
            ),
            (
                "runtime/native/dbgeng.dll",
                b"MZ synthetic dbgeng".to_vec(),
                PayloadKind::Runtime,
            ),
            (
                "runtime/python/python.exe",
                b"MZ synthetic python".to_vec(),
                PayloadKind::Runtime,
            ),
            (
                "runtime/helpers/selftest.py",
                b"print('selftest')\n".to_vec(),
                PayloadKind::PythonSource,
            ),
            (
                "runtime/windbg-mcp/windbg-mcp.exe",
                b"MZ synthetic windbg mcp".to_vec(),
                PayloadKind::Runtime,
            ),
            (
                "runtime/openssh/sshd.exe",
                b"MZ synthetic sshd".to_vec(),
                PayloadKind::Runtime,
            ),
            (
                "runtime/openssh/ssh-keygen.exe",
                b"MZ synthetic ssh-keygen".to_vec(),
                PayloadKind::Runtime,
            ),
            ("toolchain.lock.json", toolchain.clone(), PayloadKind::Data),
            ("native-build.json", native_build.clone(), PayloadKind::Data),
            (
                "originals/python.zip",
                b"synthetic python installer".to_vec(),
                PayloadKind::OriginalInstaller,
            ),
            (
                "originals/debugging-tools.msi",
                b"synthetic debugger installer".to_vec(),
                PayloadKind::OriginalInstaller,
            ),
            (
                "originals/openssh.msi",
                b"synthetic openssh installer".to_vec(),
                PayloadKind::OriginalInstaller,
            ),
            (
                "originals/windbg-mcp.zip",
                b"synthetic windbg-mcp installer".to_vec(),
                PayloadKind::OriginalInstaller,
            ),
            (
                "licenses/python.txt",
                b"synthetic python license".to_vec(),
                PayloadKind::License,
            ),
            (
                "licenses/debugging-tools.txt",
                b"synthetic debugger license".to_vec(),
                PayloadKind::License,
            ),
            (
                "licenses/openssh.txt",
                b"synthetic openssh license".to_vec(),
                PayloadKind::License,
            ),
            (
                "licenses/windbg-mcp.txt",
                b"synthetic windbg-mcp license".to_vec(),
                PayloadKind::License,
            ),
        ];
        let mut files = BTreeMap::new();
        for (relative, bytes, kind) in &payload {
            files.insert(
                (*relative).to_owned(),
                file_entry(&sha256_bytes(bytes), bytes.len(), *kind),
            );
            write_bytes(&root.join("payload").join(relative), bytes)?;
        }
        let component = |version: &str, installer: &str, license: &str| Component {
            version: version.into(),
            channel: "stable".into(),
            original_payloads: vec![installer.into()],
            licenses: vec![license.into()],
        };
        let mut components = BTreeMap::new();
        components.insert(
            "python".to_owned(),
            component("3.12.7", "originals/python.zip", "licenses/python.txt"),
        );
        components.insert(
            "debugging_tools".to_owned(),
            component(
                "10.0.26100",
                "originals/debugging-tools.msi",
                "licenses/debugging-tools.txt",
            ),
        );
        components.insert(
            "openssh".to_owned(),
            component("9.5", "originals/openssh.msi", "licenses/openssh.txt"),
        );
        components.insert(
            "windbg_mcp".to_owned(),
            component(
                "0.1.0",
                "originals/windbg-mcp.zip",
                "licenses/windbg-mcp.txt",
            ),
        );
        let manifest = Manifest {
            schema: 2,
            platform: "windows-amd64".into(),
            version: super::APP_VERSION.into(),
            source_revision: SOURCE_REVISION.into(),
            toolchain_lock_sha256: sha256_bytes(&toolchain),
            toolchain_lock_path: "toolchain.lock.json".into(),
            runtime_protocol: super::RUNTIME_PROTOCOL,
            minimum_controller_version: super::APP_VERSION.into(),
            entrypoints: EntryPoints {
                worker: "runtime/native/wukong-worker.exe".into(),
                python: "runtime/python/python.exe".into(),
                selftest: "runtime/helpers/selftest.py".into(),
                windbg_mcp: "runtime/windbg-mcp/windbg-mcp.exe".into(),
                dbgeng_library: "runtime/native/dbgeng.dll".into(),
                sshd: "runtime/openssh/sshd.exe".into(),
                ssh_keygen: "runtime/openssh/ssh-keygen.exe".into(),
            },
            components,
            license_notices: vec!["licenses/python.txt".into()],
            files,
        };
        manifest.validate().context("synthetic manifest fixture")?;
        let draft = serde_json::to_vec(&manifest)?;
        write_bytes(&root.join("manifest-draft.json"), &draft)?;
        write_bytes(&root.join("native-build.json"), &native_build)?;
        write_bytes(&root.join("binaries/wukong-worker.exe"), &worker)?;
        write_bytes(&root.join("binaries/wukong-package.exe"), &package)?;
        write_json(
            &root.join("media-template.json"),
            &json!({
                "schema": 1,
                "version": super::APP_VERSION,
                "source_revision": SOURCE_REVISION,
                "payload_manifest_draft_sha256": sha256_bytes(&draft),
                "binaries": {
                    "wukong-worker.exe": {
                        "sha256": sha256_bytes(&worker),
                        "size": worker.len(),
                    },
                    "wukong-package.exe": {
                        "sha256": sha256_bytes(&package),
                        "size": package.len(),
                    },
                },
                "toolchain_lock_sha256": sha256_bytes(&toolchain),
            }),
        )?;
        Ok(SyntheticTemplate {
            root: root.to_path_buf(),
            worker_sha256: sha256_bytes(&worker),
        })
    }

    pub(crate) fn read_json(path: &Path) -> Result<serde_json::Value> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    pub(crate) fn rewrite_json_value(path: &Path, value: &serde_json::Value) -> Result<()> {
        write_json(path, value)
    }

    pub(crate) fn rewrite_manifest(root: &Path, mutate: impl FnOnce(&mut Manifest)) -> Result<()> {
        let draft_path = root.join("manifest-draft.json");
        let mut manifest: Manifest = serde_json::from_slice(&fs::read(&draft_path)?)?;
        mutate(&mut manifest);
        let draft = serde_json::to_vec(&manifest)?;
        fs::write(&draft_path, &draft)?;
        let record_path = root.join("media-template.json");
        let mut record = read_json(&record_path)?;
        record["payload_manifest_draft_sha256"] = json!(sha256_bytes(&draft));
        write_json(&record_path, &record)
    }

    /// Replace one payload member and keep the manifest draft, template record
    /// and binary hashes consistent so a later cross-check is the failure under
    /// test rather than a stale hash.
    pub(crate) fn rewrite_payload_member(root: &Path, relative: &str, bytes: &[u8]) -> Result<()> {
        write_bytes(&root.join("payload").join(relative), bytes)?;
        let size = u64::try_from(bytes.len()).expect("synthetic payload size");
        let sha256 = sha256_bytes(bytes);
        rewrite_manifest(root, move |manifest| {
            let entry = manifest
                .files
                .get_mut(relative)
                .expect("fixture payload member is indexed");
            entry.sha256 = sha256.clone();
            entry.size = size;
        })
    }
}
