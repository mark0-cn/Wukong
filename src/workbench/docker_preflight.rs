//! Docker host checks. Never edit the user's login configuration or global PATH.
use super::docker::Docker;
use crate::i18n::Message;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
#[cfg(windows)]
use std::path::Path;
use std::{env, ffi::OsStr, fs, io::Read, path::PathBuf};

impl Docker {
    pub fn preflight(&self, image: &str) -> Result<()> {
        let os = self
            .run(&["info", "--format", "{{.OSType}}"])
            .context(Message::new("system-docker-unavailable"))?;
        ensure!(os == "linux", Message::new("system-docker-linux-required"));
        if self.inspect("image", image)?.is_none() {
            self.check_credentials(image)?;
        }
        Ok(())
    }

    pub(super) fn check_credentials(&self, image: &str) -> Result<()> {
        let root = env::var_os("DOCKER_CONFIG").map(PathBuf::from).or_else(|| {
            env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|p| PathBuf::from(p).join(".docker"))
        });
        let Some(root) = root else {
            return Ok(());
        };
        let path = root.join("config.json");
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => anyhow::bail!(Message::new("system-docker-config-unreadable")),
        };
        // Do not echo configuration contents (auths may contain credentials).
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Message::new("system-docker-config-unreadable"))?;
        ensure!(
            bytes.len() <= 1024 * 1024,
            Message::new("system-docker-config-unreadable")
        );
        let config: Value = serde_json::from_slice(&bytes)
            .map_err(|_| Message::new("system-docker-config-unreadable"))?;
        let Some(helper) = credential_helper(&config, image) else {
            return Ok(());
        };
        let name = format!("docker-credential-{helper}");
        let command = self.command();
        let path = command
            .get_envs()
            .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("PATH"))
            .and_then(|(_, value)| value.map(ToOwned::to_owned))
            .or_else(|| env::var_os("PATH"));
        ensure!(
            find_program(&name, path.as_deref()).is_some(),
            Message::new("system-docker-credential-missing").arg("helper", name)
        );
        Ok(())
    }
}

fn credential_helper<'a>(config: &'a Value, image: &str) -> Option<&'a str> {
    let first = image.split('/').next().unwrap_or_default();
    let registry = if image.contains('/')
        && (first.contains('.') || first.contains(':') || first == "localhost")
    {
        first
    } else {
        "docker.io"
    };
    let registry = match registry {
        "docker.io" | "index.docker.io" => "https://index.docker.io/v1/",
        other => other,
    };
    config
        .get("credHelpers")
        .and_then(|v| v.get(registry))
        .and_then(Value::as_str)
        .or_else(|| config.get("credsStore").and_then(Value::as_str))
        .filter(|s| !s.is_empty())
}

fn find_program(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    // A helper suffix is a program name, never an arbitrary path from config.
    if name.contains(['/', '\\']) {
        return None;
    }
    let filename = if cfg!(windows) && !name.to_ascii_lowercase().ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    env::split_paths(path?)
        .filter(|p| p.is_absolute())
        .map(|p| p.join(&filename))
        .find(|p| p.is_file())
}

#[cfg(windows)]
pub(super) fn configure_command(executable: &str) -> std::process::Command {
    let inherited = env::var_os("PATH").unwrap_or_default();
    let mut path: Vec<PathBuf> = env::split_paths(&inherited).collect();
    let mut resolved = if Path::new(executable).is_absolute() {
        Some(PathBuf::from(executable))
    } else {
        find_program(executable, Some(&inherited))
    };
    // Only supplement the standard CLI with known Desktop installations. Custom CLI
    // paths keep their identity. All changes are local to this child process.
    if resolved.is_none() && matches!(executable, "docker" | "docker.exe") {
        for (variable, suffix) in [
            ("ProgramFiles", "Docker/Docker/resources/bin"),
            ("LOCALAPPDATA", "Docker/resources/bin"),
        ] {
            if let Some(base) = env::var_os(variable) {
                let candidate = PathBuf::from(base).join(suffix).join("docker.exe");
                if candidate.is_absolute() && candidate.is_file() {
                    resolved = Some(candidate);
                    break;
                }
            }
        }
    }
    if let Some(parent) = resolved
        .as_deref()
        .filter(|p| p.is_file())
        .and_then(Path::parent)
        && !path.iter().any(|p| p == parent)
    {
        path.push(parent.to_owned());
    }
    let mut command =
        std::process::Command::new(resolved.as_deref().unwrap_or_else(|| Path::new(executable)));
    if let Ok(path) = env::join_paths(path) {
        command.env("PATH", path);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn helpers_follow_target_registry_and_hub_aliases() {
        let config = json!({"credsStore":"desktop", "credHelpers":{
            "https://index.docker.io/v1/":"hub", "private.example":"private"
        }});
        for image in [
            "hlmark0/wukong@sha256:abc",
            "ubuntu",
            "docker.io/hlmark0/wukong:tag",
            "index.docker.io/a/b",
        ] {
            assert_eq!(credential_helper(&config, image), Some("hub"));
        }
        assert_eq!(
            credential_helper(&config, "private.example/a"),
            Some("private")
        );
        assert_eq!(
            credential_helper(&config, "localhost:5000/a"),
            Some("desktop")
        );
        assert_eq!(credential_helper(&json!({}), "ubuntu"), None);
        assert_eq!(
            credential_helper(
                &json!({"credsStore":"desktop", "credHelpers":{"https://index.docker.io/v1/":""}}),
                "ubuntu"
            ),
            None
        );
    }

    #[test]
    fn helper_lookup_requires_an_existing_program_in_absolute_search_paths() {
        let temp = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) {
            "docker-credential-test.exe"
        } else {
            "docker-credential-test"
        };
        fs::write(temp.path().join(name), "fixture").unwrap();
        let path = env::join_paths([temp.path()]).unwrap();
        assert!(find_program("docker-credential-test", Some(&path)).is_some());
        assert!(find_program("docker-credential-missing", Some(&path)).is_none());
        assert!(find_program("../docker-credential-test", Some(&path)).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn explicit_docker_path_adds_its_sibling_helpers_without_changing_global_path() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("docker.exe");
        fs::write(&executable, "fixture").unwrap();
        fs::write(temp.path().join("docker-credential-desktop.exe"), "fixture").unwrap();
        let original = env::var_os("PATH");
        let command = configure_command(executable.to_str().unwrap());
        let path = command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .unwrap()
            .1;
        assert!(find_program("docker-credential-desktop", path).is_some());
        assert_eq!(env::var_os("PATH"), original);
        assert_eq!(command.get_program(), executable.as_os_str());
    }
}
