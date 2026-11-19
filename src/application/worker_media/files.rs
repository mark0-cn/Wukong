//! Windows media workspace, bounded file copies and archive publication.
use super::{MAX_MEDIA_ENTRIES, PAYLOAD_DIR};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    path::{Component, Path, PathBuf},
    thread,
    time::Duration,
};
use zip::{CompressionMethod, DateTime, System, ZipWriter, write::SimpleFileOptions};

const COPY_BUFFER: usize = 1024 * 1024;
const WINDOWS_PATH_LIMIT: usize = 259;
const STAGED_PATH_BUDGET: usize = 220;
const PAYLOAD_LONGEST_RELATIVE: usize = 132;
const WORKSPACE_PREFIX: &str = ".worker-media-finalize-";
const WORKSPACE_RANDOM: usize = 8;
const WORKSPACE_STAGED_RELATIVE: &str = "media/payload";
const WORKSPACE_ATTEMPTS: u32 = 8;

fn u64_of(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn random_hex(characters: usize) -> String {
    let value = uuid::Uuid::new_v4().simple().to_string();
    value[..characters.min(value.len())].to_owned()
}

/// Windows path length in UTF-16 code units, the unit `MAX_PATH` counts.
fn path_units(path: &Path) -> usize {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().count()
}

pub(crate) fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("open {} for hashing", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; COPY_BUFFER];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

/// Reject symlinks and Windows reparse points before reading a controller input.
pub(crate) fn reject_link(path: &Path) -> Result<fs::Metadata> {
    use std::os::windows::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect offline media path {}", path.display()))?;
    ensure!(
        !metadata.file_type().is_symlink() && metadata.file_attributes() & 0x400 == 0,
        "offline media links and reparse points are forbidden: {}",
        path.display()
    );
    Ok(metadata)
}

pub(crate) fn read_bounded(path: &Path, maximum: usize, label: &str) -> Result<Vec<u8>> {
    let metadata = reject_link(path)?;
    ensure!(
        metadata.is_file(),
        "{label} must be a regular file: {}",
        path.display()
    );
    ensure!(
        metadata.len() <= u64_of(maximum),
        "{label} exceeds its size limit: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(u64_of(maximum).saturating_add(1))
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= maximum,
        "{label} exceeds its size limit: {}",
        path.display()
    );
    Ok(bytes)
}

pub(crate) fn validate_absolute_path(path: &Path, label: &str) -> Result<()> {
    ensure!(
        path.is_absolute(),
        "offline media {label} path must be absolute: {}",
        path.display()
    );
    ensure!(
        !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir)),
        "offline media {label} path must not contain relative aliases: {}",
        path.display()
    );
    Ok(())
}

pub(crate) fn overlaps(first: &Path, second: &Path) -> bool {
    first == second || first.starts_with(second) || second.starts_with(first)
}

pub(crate) fn relative_path(relative: &str) -> PathBuf {
    relative.split('/').collect()
}

pub(crate) fn public_key_path(private: &Path) -> PathBuf {
    let mut path = private.as_os_str().to_os_string();
    path.push(".pub");
    PathBuf::from(path)
}

pub(crate) fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create offline media file {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn write_pretty_json<T: serde::Serialize + ?Sized>(
    path: &Path,
    value: &T,
) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_new_file(path, &bytes)
}

pub(crate) fn copy_verified_file(
    source: &Path,
    destination: &Path,
    expected_sha256: Option<&str>,
) -> Result<String> {
    let metadata = reject_link(source)?;
    ensure!(
        metadata.is_file(),
        "offline media source is not a regular file: {}",
        source.display()
    );
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut input = fs::File::open(source)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("create offline media copy {}", destination.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; COPY_BUFFER];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        output.write_all(&buffer[..read])?;
    }
    let sha256 = hex::encode(digest.finalize());
    if let Some(expected) = expected_sha256 {
        ensure!(
            sha256 == expected,
            "offline media source changed while copying: {}",
            source.display()
        );
    }
    Ok(sha256)
}

#[derive(Clone, Debug)]
pub(crate) struct TreeFile {
    pub(crate) relative: String,
    pub(crate) path: PathBuf,
    pub(crate) size: u64,
}

/// Collect every regular file below `root` in portable, sorted member order.
pub(crate) fn collect_files(root: &Path, maximum_entries: usize) -> Result<Vec<TreeFile>> {
    let mut pending = vec![(root.to_path_buf(), String::new())];
    let mut files = Vec::new();
    let mut entries = 0usize;
    while let Some((directory, parent)) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            entries += 1;
            ensure!(
                entries <= maximum_entries,
                "offline media tree exceeds {maximum_entries} entries: {}",
                root.display()
            );
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("offline media member name is not UTF-8"))?;
            let relative = if parent.is_empty() {
                name
            } else {
                format!("{parent}/{name}")
            };
            wukong_worker_package::validate_path(&relative)?;
            let path = entry.path();
            let metadata = reject_link(&path)?;
            if metadata.is_dir() {
                pending.push((path, relative));
            } else {
                ensure!(
                    metadata.is_file(),
                    "offline media members must be regular files: {}",
                    path.display()
                );
                files.push(TreeFile {
                    relative,
                    path,
                    size: metadata.len(),
                });
            }
        }
    }
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(files)
}

pub(crate) fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let files = collect_files(source, MAX_MEDIA_ENTRIES)?;
    for file in files {
        copy_verified_file(
            &file.path,
            &destination.join(relative_path(&file.relative)),
            None,
        )?;
    }
    Ok(())
}

fn workspace_margin() -> usize {
    // os separator + workspace name + staged media payload path + the deepest
    // shipped payload member (`PAYLOAD_LONGEST_RELATIVE` is the conservative
    // measured value the Python finalize uses for the same budget).
    1 + WORKSPACE_PREFIX.len()
        + WORKSPACE_RANDOM
        + 1
        + WORKSPACE_STAGED_RELATIVE.len()
        + 1
        + PAYLOAD_LONGEST_RELATIVE
}

fn workspace_base_fits(base: &Path) -> bool {
    path_units(base) + workspace_margin() <= STAGED_PATH_BUDGET
}

fn workspace_candidates(output_parent: Option<&Path>) -> Vec<(String, PathBuf)> {
    let mut candidates = Vec::new();
    for variable in ["TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(variable) {
            candidates.push((variable.to_owned(), PathBuf::from(value)));
        }
    }
    candidates.push((
        "system temporary directory".to_owned(),
        std::env::temp_dir(),
    ));
    if let Some(parent) = output_parent {
        candidates.push(("output parent".to_owned(), parent.to_path_buf()));
    }
    candidates
}

/// Short-lived protected staging directory for one finalize operation.
pub(crate) struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub(crate) fn create(output_parent: Option<&Path>) -> Result<Self> {
        let mut rejected = Vec::new();
        for (label, candidate) in workspace_candidates(output_parent) {
            if !candidate.is_absolute() {
                rejected.push(format!("{label} {} is not absolute", candidate.display()));
                continue;
            }
            if !candidate.is_dir() {
                rejected.push(format!(
                    "{label} {} is not an existing directory",
                    candidate.display()
                ));
                continue;
            }
            if !workspace_base_fits(&candidate) {
                rejected.push(format!(
                    "{label} {} needs {} characters",
                    candidate.display(),
                    path_units(&candidate) + workspace_margin()
                ));
                continue;
            }
            for _ in 0..WORKSPACE_ATTEMPTS {
                let root = candidate.join(format!(
                    "{WORKSPACE_PREFIX}{}",
                    random_hex(WORKSPACE_RANDOM)
                ));
                match fs::create_dir(&root) {
                    Ok(()) => return Ok(Self { root }),
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                    Err(error) => {
                        rejected.push(format!(
                            "{label} {} is not writable: {error}",
                            candidate.display()
                        ));
                        break;
                    }
                }
            }
        }
        bail!(
            "offline media staging needs a short absolute writable workspace directory: {}. A workspace base may be at most {} characters and must already exist; move the controller state directory or the output to a shorter absolute path, for example C:\\state or D:\\state, or point TEMP/TMP at a short absolute directory",
            rejected.join("; "),
            STAGED_PATH_BUDGET.saturating_sub(workspace_margin())
        )
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        for attempt in 0..5u64 {
            if fs::remove_dir_all(&self.root).is_ok() || !self.root.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(100 * (attempt + 1)));
        }
    }
}

pub(crate) fn ensure_output_path_fits(
    output: &Path,
    longest_member: usize,
    label: &str,
) -> Result<()> {
    let payload_relative = 1 + PAYLOAD_DIR.len() + 1 + longest_member;
    let required = path_units(output) + payload_relative;
    if required <= WINDOWS_PATH_LIMIT {
        return Ok(());
    }
    let room = WINDOWS_PATH_LIMIT.saturating_sub(payload_relative);
    bail!(
        "offline media {label} path is too long for Windows MAX_PATH: {required} characters are needed for the deepest payload member (limit {WINDOWS_PATH_LIMIT}); the destination {} cannot hold the payload. The {label} directory may be at most {room} characters; move the controller state directory (or finalized media output) to a shorter absolute path",
        output.display()
    )
}

pub(crate) fn ensure_staged_path_fits(workspace_root: &Path, longest_member: usize) -> Result<()> {
    let required =
        path_units(workspace_root) + 1 + WORKSPACE_STAGED_RELATIVE.len() + 1 + longest_member;
    ensure!(
        required <= WINDOWS_PATH_LIMIT,
        "offline media template payload needs {required} staged path characters (limit {WINDOWS_PATH_LIMIT}); point TEMP/TMP at a shorter absolute directory"
    );
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ZipReport {
    pub(crate) sha256: String,
    pub(crate) size: u64,
    pub(crate) entries: usize,
}

/// Write a deterministic ZIP archive of `source`; files are sorted, timestamped
/// 1980-01-01 and compressed with deflate. The archive never contains links,
/// directory entries or controller credentials.
pub(crate) fn write_directory_zip(
    source: &Path,
    output: &Path,
    prefix: Option<&str>,
) -> Result<ZipReport> {
    ensure!(
        source.is_dir(),
        "offline media archive source is missing: {}",
        source.display()
    );
    ensure!(
        !output.exists(),
        "offline media archive already exists: {}",
        output.display()
    );
    let parent = output
        .parent()
        .context("offline media archive has no parent directory")?;
    fs::create_dir_all(parent)?;
    let files = collect_files(source, MAX_MEDIA_ENTRIES)?;
    ensure!(!files.is_empty(), "offline media archive source is empty");
    let temporary = parent.join(format!(".worker-media-archive-{}.zip", random_hex(16)));
    struct Cleanup<'a>(&'a Path);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0);
        }
    }
    let cleanup = Cleanup(&temporary);
    {
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(DateTime::default())
            .unix_permissions(0o644)
            .system(System::Unix);
        let mut buffer = vec![0u8; COPY_BUFFER];
        for entry in &files {
            let name = match prefix {
                Some(prefix) => format!("{prefix}/{}", entry.relative),
                None => entry.relative.clone(),
            };
            archive.start_file(name, options)?;
            let mut input = fs::File::open(&entry.path)?;
            loop {
                let read = input.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                archive.write_all(&buffer[..read])?;
            }
        }
        let file = archive.finish()?;
        file.sync_all()?;
    }
    fs::rename(&temporary, output).context("publish the offline media archive")?;
    drop(cleanup);
    Ok(ZipReport {
        sha256: sha256_file(output)?,
        size: fs::metadata(output)?.len(),
        entries: files.len(),
    })
}

/// Publish a finished media tree by rename, retrying transient Windows locks and
/// falling back to a copy when the workspace lives on another volume.
pub(crate) fn publish_directory(source: &Path, target: &Path) -> Result<()> {
    ensure!(
        !target.exists(),
        "offline media output already exists: {}",
        target.display()
    );
    let parent = target
        .parent()
        .context("offline media output has no parent directory")?;
    fs::create_dir_all(parent)?;
    let mut last_error = None;
    for attempt in 0..10u64 {
        match fs::rename(source, target) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if target.exists() {
                    bail!("offline media output could not be published: {error}");
                }
                if matches!(error.raw_os_error(), Some(17 | 18)) {
                    match copy_tree(source, target) {
                        Ok(()) => {
                            let _ = fs::remove_dir_all(source);
                            return Ok(());
                        }
                        Err(copy_error) => {
                            let _ = fs::remove_dir_all(target);
                            last_error = Some(copy_error);
                        }
                    }
                } else {
                    last_error = Some(error.into());
                }
                thread::sleep(Duration::from_millis(500 * (attempt + 1)));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        anyhow::anyhow!(
            "offline media output could not be published: {}",
            target.display()
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::ZipArchive;

    fn short_base() -> PathBuf {
        std::env::temp_dir()
    }

    #[test]
    fn workspace_is_created_short_lived_and_removes_itself() -> Result<()> {
        let base = short_base();
        assert!(workspace_base_fits(&base));
        let root = {
            let workspace = Workspace::create(Some(&base))?;
            let root = workspace.root().to_path_buf();
            assert!(root.is_dir());
            assert!(root.starts_with(&base));
            assert!(
                root.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(WORKSPACE_PREFIX))
            );
            root
        };
        assert!(!root.exists(), "workspace must be discarded on drop");
        assert!(
            !workspace_base_fits(&base.join("a".repeat(80))),
            "a deep workspace base must be rejected before staging"
        );
        Ok(())
    }

    #[test]
    fn media_output_and_staging_paths_respect_max_path() -> Result<()> {
        let short = short_base().join("media");
        ensure_output_path_fits(&short, 131, "output")?;
        let deep = short_base().join("segment".repeat(40));
        let error = ensure_output_path_fits(&deep, 131, "output").unwrap_err();
        assert!(
            error.to_string().contains("MAX_PATH"),
            "unexpected error: {error}"
        );
        let workspace = short_base().join("workspace");
        ensure_staged_path_fits(&workspace, 131)?;
        let error = ensure_staged_path_fits(&deep, 131).unwrap_err();
        assert!(
            error.to_string().contains("staged path"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    fn write_tree(root: &Path, entries: &[(&str, &[u8])]) -> Result<()> {
        for (relative, bytes) in entries {
            let path = root.join(relative_path(relative));
            fs::create_dir_all(path.parent().expect("fixture parent"))?;
            fs::write(path, bytes)?;
        }
        Ok(())
    }

    #[test]
    fn zip_archives_are_deterministic_and_sorted() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("media");
        fs::create_dir_all(source.join("enrollment"))?;
        fs::create_dir_all(source.join("payload/nested"))?;
        write_tree(
            &source,
            &[
                ("setup.json", b"setup"),
                ("media.json", b"media"),
                ("enrollment/enrollment.json", b"enrollment"),
                ("payload/nested/worker.exe", b"worker"),
                ("payload/a.txt", b"a"),
            ],
        )?;
        let first = temporary.path().join("first.zip");
        let second = temporary.path().join("second.zip");
        let report = write_directory_zip(&source, &first, None)?;
        let repeated = write_directory_zip(&source, &second, None)?;
        assert_eq!(report.sha256, repeated.sha256);
        assert_eq!(report.size, repeated.size);
        assert_eq!(report.entries, 5);
        assert_eq!(fs::read(&first)?, fs::read(&second)?);
        let mut archive = ZipArchive::new(fs::File::open(&first)?)?;
        let names: Vec<String> = (0..archive.len())
            .map(|index| {
                archive
                    .by_index(index)
                    .expect("zip entry")
                    .name()
                    .expect("name")
                    .into_owned()
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "enrollment/enrollment.json",
                "media.json",
                "payload/a.txt",
                "payload/nested/worker.exe",
                "setup.json",
            ]
        );
        for index in 0..archive.len() {
            let entry = archive.by_index(index)?;
            assert_eq!(entry.last_modified(), Some(DateTime::default()));
            assert_eq!(
                entry.unix_mode(),
                Some(0o100644),
                "regular-file permission bits must be stable"
            );
            assert_eq!(entry.compression(), CompressionMethod::Deflated);
            assert!(!entry.is_dir());
        }
        assert!(
            write_directory_zip(&source, &first, None).is_err(),
            "writing must never overwrite an existing archive"
        );
        Ok(())
    }

    #[test]
    fn zip_prefix_matches_the_release_archive_layout() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("template");
        write_tree(&source, &[("payload/worker.exe", b"worker")])?;
        let archive_path = temporary.path().join("template.zip");
        write_directory_zip(
            &source,
            &archive_path,
            Some("wukong-windows-worker-media-1.0.0"),
        )?;
        let mut archive = ZipArchive::new(fs::File::open(&archive_path)?)?;
        let names: Vec<String> = (0..archive.len())
            .map(|index| {
                archive
                    .by_index(index)
                    .expect("zip entry")
                    .name()
                    .expect("name")
                    .into_owned()
            })
            .collect();
        assert_eq!(
            names,
            vec!["wukong-windows-worker-media-1.0.0/payload/worker.exe"]
        );
        Ok(())
    }

    #[test]
    fn publish_publishes_once_and_refuses_an_existing_target() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("staged/media");
        write_tree(&source, &[("media.json", b"media")])?;
        let target = temporary.path().join("published");
        publish_directory(&source, &target)?;
        assert!(!source.exists());
        assert_eq!(fs::read(target.join("media.json"))?, b"media");
        assert!(publish_directory(&source, &target).is_err());
        Ok(())
    }

    #[test]
    fn links_and_unsafe_member_names_are_rejected() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().join("tree");
        fs::create_dir(&root)?;
        fs::write(root.join("caf\u{e9}.txt"), b"non-ASCII member")?;
        assert!(
            collect_files(&root, MAX_MEDIA_ENTRIES).is_err(),
            "portable member names must stay ASCII"
        );
        Ok(())
    }
}
