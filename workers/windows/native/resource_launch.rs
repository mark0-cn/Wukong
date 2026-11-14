//! Windows admission for the existing schema-1 LaunchSpec. serde owns decoding;
//! native input receipts, not worker filesystem paths, supply every member.
use crate::{
    inputs::Inputs,
    resource_input::{AdmittedInput, Purpose, Reference},
    resource_journal::Owner,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Read};

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    StdinPipe,
    ArgvFile,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub manifest: Reference,
    pub files: Vec<Reference>,
    pub cwd: String,
    pub input_mode: InputMode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Document {
    schema_version: u32,
    base: String,
    target: String,
    files: BTreeMap<String, String>,
    #[serde(default)]
    args: Vec<String>,
    cwd: String,
    input_mode: InputMode,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
    #[serde(default)]
    loader: Option<String>,
    #[serde(default)]
    library_path: Option<String>,
    #[serde(default)]
    service: Option<Service>,
    #[serde(default, rename = "challenge_id")]
    _challenge_id: Option<String>,
    #[serde(default, rename = "runtime_confirmed")]
    _runtime_confirmed: Option<bool>,
}
#[derive(Deserialize)]
struct Service {
    mode: String,
}
fn default_timeout() -> u64 {
    30
}

pub(crate) struct Resolved {
    pub program: Reference,
    pub files: Vec<Reference>,
    pub cwd: String,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub timeout_secs: u64,
    pub input_mode: InputMode,
}
pub(crate) fn logical(path: &str) -> Result<()> {
    ensure!(
        path.split('/').count() <= crate::resource_cleanup::MAX_DEPTH,
        "native launch exceeds cleanup directory depth"
    );
    wukong_worker_package::validate_path(
        path.strip_prefix("files/")
            .context("launch paths must use admitted files/...")?,
    )
}
impl Document {
    pub(crate) fn read(source: AdmittedInput) -> Result<Self> {
        ensure!(
            source.reference().size <= 65536,
            "LaunchSpec exceeds 64 KiB"
        );
        let mut bytes = Vec::new();
        source.take(65537).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 65536, "LaunchSpec exceeds 64 KiB");
        Ok(serde_json::from_slice(&bytes)?)
    }
    pub(crate) fn resolve(
        &self,
        mut reference: impl FnMut(&str, Purpose) -> Result<Reference>,
    ) -> Result<Resolved> {
        ensure!(
            self.schema_version == 1 && !self.files.is_empty() && self.files.len() <= 64,
            "unsupported native launch schema or file count"
        );
        ensure!(self.args.len() <= 64, "LaunchSpec exceeds 64 arguments");
        ensure!(
            self.loader.is_none() && self.library_path.is_none(),
            "Linux loader launches are unavailable in a Windows environment"
        );
        ensure!(
            self.service
                .as_ref()
                .is_none_or(|service| service.mode == "offline"),
            "native process LaunchSpec requires offline networking"
        );
        logical(&self.base)?;
        wukong_worker_package::validate_path(&self.target)?;
        ensure!(
            self.files.contains_key(&self.target),
            "launch target identity is missing"
        );
        let cwd = if self.cwd == "." {
            self.base.clone()
        } else {
            wukong_worker_package::validate_path(&self.cwd)?;
            format!("{}/{}", self.base, self.cwd)
        };
        logical(&cwd)?;
        ensure!(
            self.input_mode != InputMode::ArgvFile || self.args.iter().any(|arg| arg == "@@"),
            "argv_file launch requires @@"
        );
        let mut program = None;
        let mut files = Vec::new();
        for (name, hash) in &self.files {
            wukong_worker_package::validate_path(name)?;
            ensure!(
                !name
                    .split('/')
                    .any(|part| part.eq_ignore_ascii_case(".wukong-root-guard")),
                "launch member uses a reserved resource guard name"
            );
            let path = format!("{}/{name}", self.base);
            logical(&path)?;
            let file = reference(
                &path,
                if name == &self.target {
                    Purpose::Execute
                } else {
                    Purpose::Inspect
                },
            )?;
            ensure!(
                file.path == path && file.sha256 == *hash,
                "launch member differs from admitted content"
            );
            if name == &self.target {
                ensure!(file.executable, "launch target is not executable");
                program = Some(file);
            } else {
                files.push(file);
            }
        }
        let program = program.context("launch target is absent")?;
        crate::resource_image::validate_paths(&program, &files)?;
        validate_cwd(&cwd, std::iter::once(&program).chain(&files))?;
        Ok(Resolved {
            program,
            files,
            cwd,
            arguments: self.args.clone(),
            environment: self.env.clone(),
            timeout_secs: self.timeout_secs,
            input_mode: self.input_mode,
        })
    }
}
pub(crate) fn validate_cwd<'a>(
    cwd: &str,
    files: impl Iterator<Item = &'a Reference>,
) -> Result<()> {
    logical(cwd)?;
    let cwd = cwd.to_ascii_lowercase();
    for file in files {
        let name = file.path.to_ascii_lowercase();
        ensure!(
            name != cwd
                && !cwd.starts_with(&(name.clone() + "/"))
                && name != format!("{cwd}/.wukong-root-guard"),
            "launch cwd overlaps a file"
        );
    }
    Ok(())
}

impl Binding {
    pub(crate) fn validate(&self, program: &Reference, input: Option<&Reference>) -> Result<()> {
        logical(&self.manifest.path)?;
        ensure!(
            self.manifest.size <= 65536 && self.files.len() <= 63,
            "native launch binding exceeds bounds"
        );
        ensure!(
            self.input_mode != InputMode::ArgvFile || input.is_some(),
            "argv_file requires an admitted input"
        );
        let files = self.image_files(program, input)?;
        crate::resource_image::validate_paths(program, &files)?;
        validate_cwd(&self.cwd, std::iter::once(program).chain(&files))
    }
    pub(crate) fn image_files(
        &self,
        program: &Reference,
        input: Option<&Reference>,
    ) -> Result<Vec<Reference>> {
        let mut files = self.files.clone();
        if self.input_mode == InputMode::ArgvFile {
            let input = input.context("argv_file input is missing")?;
            if input.path == program.path {
                ensure!(input == program, "launch input aliases a changed program");
            } else if let Some(file) = files.iter().find(|file| file.path == input.path) {
                ensure!(file == input, "launch input aliases a changed member");
            } else {
                files.push(input.clone());
            }
        }
        Ok(files)
    }
    pub(crate) fn verify(
        &self,
        program: &Reference,
        input: Option<&Reference>,
        document: Document,
        arguments: &[String],
        environment: &BTreeMap<String, String>,
        timeout: u64,
    ) -> Result<()> {
        let resolved = document.resolve(|path, _| {
            std::iter::once(program)
                .chain(&self.files)
                .find(|file| file.path == path)
                .cloned()
                .context("launch member is missing from the protected request")
        })?;
        ensure!(
            resolved.program == *program
                && resolved.files == self.files
                && resolved.cwd == self.cwd
                && resolved.input_mode == self.input_mode
                && resolved.arguments == arguments
                && resolved.environment == *environment
                && resolved.timeout_secs == timeout,
            "protected native launch differs from its original manifest"
        );
        self.validate(program, input)
    }
}
pub(crate) fn prepare(inputs: &Inputs, owner: &Owner, path: &str) -> Result<(Reference, Resolved)> {
    let manifest = inputs.reference(owner, path, Purpose::Inspect)?;
    let document = Document::read(inputs.acquire(owner, &manifest, Purpose::Inspect)?)?;
    let resolved = document.resolve(|path, purpose| inputs.reference(owner, path, purpose))?;
    Ok((manifest, resolved))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document() -> serde_json::Value {
        serde_json::json!({"schema_version":1,"base":"files/case","target":"bin/target.exe","files":{"bin/target.exe":"a".repeat(64),"data/input.bin":"b".repeat(64)},"args":["--input","@@"],"cwd":"work","input_mode":"argv_file","env":{},"timeout_secs":5,"service":{"mode":"offline","port":1337},"loader":null,"library_path":null,"runtime_confirmed":false})
    }
    fn reference(path: &str) -> Reference {
        Reference {
            path: path.into(),
            id: "c".repeat(64),
            sha256: if path.ends_with("target.exe") {
                "a".repeat(64)
            } else {
                "b".repeat(64)
            },
            size: 4,
            executable: path.ends_with("target.exe"),
        }
    }
    #[test]
    fn existing_launch_schema_preserves_member_hashes_cwd_and_input_mode() -> Result<()> {
        let doc: Document = serde_json::from_value(document())?;
        let resolved = doc.resolve(|path, _| Ok(reference(path)))?;
        assert_eq!(resolved.program.path, "files/case/bin/target.exe");
        assert_eq!(resolved.cwd, "files/case/work");
        assert_eq!(resolved.input_mode, InputMode::ArgvFile);
        let binding = Binding {
            manifest: reference("files/case/launch.json"),
            files: resolved.files,
            cwd: resolved.cwd,
            input_mode: resolved.input_mode,
        };
        let input = reference("files/input.bin");
        binding.verify(
            &resolved.program,
            Some(&input),
            doc,
            &resolved.arguments,
            &resolved.environment,
            5,
        )?;
        assert_eq!(
            binding.image_files(&resolved.program, Some(&input))?.len(),
            2
        );
        let mut changed = resolved.arguments;
        changed.push("changed".into());
        assert!(
            binding
                .verify(
                    &resolved.program,
                    Some(&input),
                    serde_json::from_value(document())?,
                    &changed,
                    &resolved.environment,
                    5
                )
                .is_err()
        );
        Ok(())
    }
    #[test]
    fn native_launch_rejects_changed_identity_platform_overrides_and_path_aliases() -> Result<()> {
        for (key, value) in [
            ("schema_version", serde_json::json!(2)),
            ("base", serde_json::json!("C:/Windows")),
            ("target", serde_json::json!("../target.exe")),
            ("cwd", serde_json::json!("bin/target.exe/subdir")),
            ("loader", serde_json::json!("loader")),
            ("library_path", serde_json::json!("libraries")),
            ("input_mode", serde_json::json!("pty")),
            ("service", serde_json::json!({"mode":"local_service"})),
            ("files", serde_json::json!({"bin/target.exe":"changed"})),
            ("args", serde_json::json!(["no-placeholder"])),
        ] {
            let mut raw = document();
            raw[key] = value;
            let result = serde_json::from_value::<Document>(raw);
            assert!(
                result.is_err()
                    || result
                        .unwrap()
                        .resolve(|path, _| Ok(reference(path)))
                        .is_err(),
                "{key}"
            );
        }
        let mut raw = document();
        raw["files"]["BIN/TARGET.exe"] = serde_json::json!("b".repeat(64));
        assert!(
            serde_json::from_value::<Document>(raw)?
                .resolve(|path, _| Ok(reference(path)))
                .is_err()
        );
        Ok(())
    }
}
