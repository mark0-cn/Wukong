use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Target {
    pub binary: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChallengeRuntime {
    pub kind: String,
    pub libc: Option<String>,
    pub loader: Option<String>,
    pub dockerfile: Option<String>,
    pub image: Option<String>,
    pub build_args: BTreeMap<String, String>,
    pub container_binary: Option<String>,
    pub rootfs: Option<String>,
    pub privileged: bool,
}
impl Default for ChallengeRuntime {
    fn default() -> Self {
        Self {
            kind: "native".into(),
            libc: None,
            loader: None,
            dockerfile: None,
            image: None,
            build_args: BTreeMap::new(),
            container_binary: None,
            rootfs: None,
            privileged: false,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Launch {
    pub argv: Vec<String>,
    pub cwd: String,
    pub input: String,
    pub timeout_seconds: u64,
    pub env: BTreeMap<String, String>,
}
impl Default for Launch {
    fn default() -> Self {
        Self {
            argv: vec![],
            cwd: "original".into(),
            input: "stdin_pipe".into(),
            timeout_seconds: 10,
            env: BTreeMap::new(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Network {
    pub mode: String,
    pub port: u16,
    pub host: Option<String>,
}
impl Default for Network {
    fn default() -> Self {
        Self {
            mode: "offline".into(),
            port: 1337,
            host: None,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Challenge {
    pub schema_version: u32,
    pub id: String,
    pub architecture: String,
    pub target: Target,
    pub runtime: ChallengeRuntime,
    pub launch: Launch,
    pub network: Network,
}
impl Default for Challenge {
    fn default() -> Self {
        Self {
            schema_version: 1,
            id: String::new(),
            architecture: "unknown".into(),
            target: Target::default(),
            runtime: ChallengeRuntime::default(),
            launch: Launch::default(),
            network: Network::default(),
        }
    }
}

pub fn relative_path(root: &Path, value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    ensure!(
        !value.is_empty()
            && !value.contains('\\')
            && !value.contains(':')
            && !path.is_absolute()
            && path
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
        "invalid relative path: {value}"
    );
    let joined = root.join(path);
    if joined.exists() {
        ensure!(
            fs::canonicalize(&joined)?.starts_with(fs::canonicalize(root)?),
            "path escapes challenge: {value}"
        );
    }
    Ok(joined)
}
impl Challenge {
    pub fn validate(&self, root: &Path) -> Result<()> {
        valid_id(&self.id)?;
        ensure!(
            self.id.len() <= 32,
            "challenge id must be at most 32 characters"
        );
        ensure!(self.schema_version == 1, "unsupported challenge schema");
        ensure!(
            ["native", "loader", "docker"].contains(&self.runtime.kind.as_str()),
            "runtime.kind must be native, loader or docker"
        );
        ensure!(
            ["stdin_pipe", "argv_file", "pty"].contains(&self.launch.input.as_str()),
            "unsupported input mode"
        );
        ensure!(
            (1..=3600).contains(&self.launch.timeout_seconds),
            "invalid timeout"
        );
        for path in [
            &self.target.binary,
            &self.runtime.libc,
            &self.runtime.loader,
            &self.runtime.dockerfile,
        ]
        .into_iter()
        .flatten()
        {
            ensure!(
                relative_path(root, path)?.is_file(),
                "required file missing: {path}"
            );
        }
        ensure!(
            relative_path(root, &self.launch.cwd)?.is_dir(),
            "working directory missing"
        );
        ensure!(
            ["offline", "local_service", "remote"].contains(&self.network.mode.as_str()),
            "unknown network mode"
        );
        if self.network.mode != "offline" {
            ensure!(self.network.port > 0, "service port must be nonzero");
        }
        if self.network.mode == "remote" {
            let host = self
                .network
                .host
                .as_deref()
                .context("remote mode requires host")?;
            ensure!(
                !host.is_empty()
                    && host.len() <= 253
                    && !host.chars().any(|c| c.is_control() || c.is_whitespace()),
                "invalid remote host"
            );
        }
        for (key, value) in &self.launch.env {
            ensure!(
                !key.is_empty()
                    && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    && !value.contains('\0'),
                "invalid target environment variable"
            );
        }
        Ok(())
    }
}

pub fn inventory(root: &Path) -> Result<Value> {
    fn visit(root: &Path, dir: &Path, items: &mut BTreeMap<String, Value>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            ensure!(items.len() < 20000, "too many imported files");
            let key = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_symlink() {
                let link = fs::read_link(&path)?;
                ensure!(
                    !link.is_absolute() && fs::canonicalize(&path)?.starts_with(root),
                    "external symlink in challenge: {key}"
                );
                items.insert(key, json!({"symlink":link}));
            } else if metadata.is_dir() {
                visit(root, &path, items)?;
            } else if metadata.is_file() {
                items.insert(
                    key,
                    json!({"sha256":crate::binary::sha256_file(&path)?,"size":metadata.len()}),
                );
            } else {
                anyhow::bail!("unsupported file type: {key}");
            }
        }
        Ok(())
    }
    let root = fs::canonicalize(root)?;
    let mut items = BTreeMap::new();
    visit(&root, &root, &mut items)?;
    Ok(serde_json::to_value(items)?)
}
pub fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let output = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_symlink() {
            let link = fs::read_link(&path)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(link, output)?;
            #[cfg(windows)]
            if path.is_dir() {
                std::os::windows::fs::symlink_dir(link, output)?;
            } else {
                std::os::windows::fs::symlink_file(link, output)?;
            }
        } else if metadata.is_dir() {
            copy_tree(&path, &output)?;
        } else if metadata.is_file() {
            fs::copy(path, output)?;
        } else {
            anyhow::bail!("unsupported imported file type");
        }
    }
    Ok(())
}

impl Workbench {
    pub fn verify_original(&self, id: &str) -> Result<Value> {
        let root = self.challenge_dir(id)?;
        let lock: Value = read_json(&root.join("environment.lock.json"))?;
        let actual = inventory(&root.join("original"))?;
        ensure!(
            actual == lock["original_files"],
            "original attachments changed; import a new challenge to establish a new identity"
        );
        Ok(json!({"id":id,"original_verified":true}))
    }
    pub fn challenge_dir(&self, id: &str) -> Result<PathBuf> {
        valid_id(id)?;
        Ok(self.root.join("challenges").join(id))
    }
    pub fn challenge(&self, id: &str) -> Result<Challenge> {
        let root = self.challenge_dir(id)?;
        let result: Challenge = toml::from_str(&fs::read_to_string(root.join("challenge.toml"))?)?;
        ensure!(result.id == id, "challenge directory identity mismatch");
        result.validate(&root)?;
        Ok(result)
    }
    pub fn challenges(&self) -> Result<Value> {
        let mut items = vec![];
        let parent = self.root.join("challenges");
        if parent.exists() {
            for entry in fs::read_dir(parent)? {
                let entry = entry?;
                if entry.path().join("challenge.toml").is_file() {
                    let id = entry.file_name().to_string_lossy().into_owned();
                    match self.challenge(&id) {
                        Ok(challenge) => items.push(json!({"challenge":challenge})),
                        Err(error) => items.push(json!({"id":id,"error":error.to_string()})),
                    }
                }
            }
        }
        Ok(json!({"challenges":items}))
    }
    pub fn import_challenge(&self, source: &Path, id: &str, binary: Option<&str>) -> Result<Value> {
        valid_id(id)?;
        let source = fs::canonicalize(source)?;
        let destination = self.challenge_dir(id)?;
        ensure!(
            !destination.exists(),
            "challenge already exists; use a new id"
        );
        ensure!(
            !source.starts_with(&self.root) && !self.root.starts_with(&source),
            "import source and workbench must not contain one another"
        );
        let parent = destination.parent().unwrap();
        fs::create_dir_all(parent)?;
        let temporary = tempfile::tempdir_in(parent)?;
        let root = temporary.path();
        let original = root.join("original");
        let mut challenge = if source.is_dir() && source.join("challenge.toml").is_file() {
            toml::from_str::<Challenge>(&fs::read_to_string(source.join("challenge.toml"))?)?
        } else {
            Challenge::default()
        };
        if source.is_dir() {
            inventory(&source)?;
            copy_tree(&source, &original)?;
        } else {
            fs::create_dir_all(&original)?;
            fs::copy(
                &source,
                original.join(source.file_name().context("input filename missing")?),
            )?;
        }
        challenge.id = id.into();
        for path in [
            &mut challenge.target.binary,
            &mut challenge.runtime.libc,
            &mut challenge.runtime.loader,
            &mut challenge.runtime.dockerfile,
        ]
        .into_iter()
        .flatten()
        {
            *path = format!("original/{path}");
        }
        if let Some(binary) = binary {
            challenge.target.binary = Some(format!("original/{binary}"));
        }
        let files = inventory(&original)?;
        if challenge.target.binary.is_none() {
            let mut candidates = vec![];
            for (path, _) in files.as_object().unwrap() {
                if path.contains("libc") || path.split('/').last().unwrap_or("").starts_with("ld-")
                {
                    continue;
                }
                if let Ok(profile) = crate::binary::inspect(original.join(path)) {
                    if profile.format == "Elf" {
                        candidates.push(path.clone());
                    }
                }
            }
            ensure!(
                candidates.len() <= 1,
                "multiple ELF candidates; pass --binary with the path relative to the source directory"
            );
            challenge.target.binary = candidates.pop().map(|p| format!("original/{p}"));
        }
        if let Some(binary) = &challenge.target.binary {
            challenge.architecture =
                crate::binary::inspect(relative_path(root, binary)?)?.architecture;
        }
        if challenge.runtime.dockerfile.is_none() && original.join("Dockerfile").is_file() {
            challenge.runtime.dockerfile = Some("original/Dockerfile".into());
        }
        if challenge.runtime.dockerfile.is_some() {
            challenge.runtime.kind = "docker".into();
            challenge.network.mode = "local_service".into();
        }
        if challenge.runtime.libc.is_none() && original.join("libc.so.6").is_file() {
            challenge.runtime.libc = Some("original/libc.so.6".into());
        }
        if challenge.runtime.loader.is_none() {
            let candidates: Vec<_> = files
                .as_object()
                .unwrap()
                .keys()
                .filter(|p| p.split('/').next_back().unwrap_or("").starts_with("ld-"))
                .collect();
            if candidates.len() == 1 {
                challenge.runtime.loader = Some(format!("original/{}", candidates[0]));
            }
        }
        if challenge.runtime.kind == "native"
            && challenge.runtime.libc.is_some()
            && challenge.runtime.loader.is_some()
        {
            challenge.runtime.kind = "loader".into();
        }
        if challenge.launch.cwd != "original" {
            challenge.launch.cwd = format!("original/{}", challenge.launch.cwd);
        }
        for folder in ["prepared", "scripts", "results"] {
            fs::create_dir_all(root.join(folder))?;
        }
        challenge.validate(root)?;
        fs::write(
            root.join("challenge.toml"),
            toml::to_string_pretty(&challenge)?,
        )?;
        write_json(
            &root.join("environment.lock.json"),
            &json!({"schema_version":1,"original_files":files,"status":"imported","runtime_confirmed":false}),
        )?;
        let kept = temporary.keep();
        fs::rename(kept, &destination)?;
        Ok(json!({"challenge":challenge,"directory":destination,"status":"imported"}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_keeps_original_identity_and_detects_edits() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("attachments");
        fs::create_dir(&source)?;
        fs::write(source.join("README.txt"), "fixture")?;
        let mut config = AppConfig::default();
        config.workbench.root = temp.path().join("workbench");
        let workbench = Workbench::open(&config)?;
        workbench.import_challenge(&source, "first", None)?;
        assert!(workbench.verify_original("first").is_ok());
        assert!(workbench.import_challenge(&source, "first", None).is_err());
        fs::write(
            workbench
                .challenge_dir("first")?
                .join("original/README.txt"),
            "changed",
        )?;
        assert!(workbench.verify_original("first").is_err());
        Ok(())
    }
    #[test]
    fn challenge_paths_cannot_escape() -> Result<()> {
        let root = tempfile::tempdir()?;
        for path in ["../secret", "/etc/passwd", "C:/secret", "a\\..\\b", ""] {
            assert!(relative_path(root.path(), path).is_err(), "{path}");
        }
        assert!(relative_path(root.path(), "original/program").is_ok());
        assert!(valid_id("../../other").is_err());
        Ok(())
    }
}
