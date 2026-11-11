//! Configuration documents preserve comments, relative paths and external edits.
use crate::i18n::Message as UiMessage;
use crate::{config::AppConfig, store::Store};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item};
#[derive(Clone, Serialize, Deserialize)]
pub struct ConfigDocument {
    pub path: PathBuf,
    pub source: String,
    pub original: Option<String>,
    pub form: AppConfig,
}
#[derive(Clone, Deserialize)]
pub struct ConfigEdit {
    pub path: Vec<String>,
    pub value: Value,
}
impl ConfigDocument {
    pub fn open(path: PathBuf) -> Result<Self> {
        let path = std::path::absolute(path)?;
        let original = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let source = original
            .clone()
            .unwrap_or(toml::to_string_pretty(&AppConfig::default())?);
        let form =
            toml::from_str(&source).context(UiMessage::new("system-invalid-toml-configuration"))?;
        Ok(Self {
            path,
            source,
            original,
            form,
        })
    }
    pub fn patch(&mut self, edits: &[ConfigEdit]) -> Result<()> {
        let mut document = self.source.parse::<DocumentMut>()?;
        for edit in edits {
            ensure!(
                !edit.path.is_empty() && edit.path.len() <= 5,
                "invalid configuration field path"
            );
            let mut item = document.as_item_mut();
            for key in &edit.path[..edit.path.len() - 1] {
                if item.is_none() {
                    *item = Item::Table(toml_edit::Table::new());
                }
                ensure!(
                    item.is_table_like(),
                    UiMessage::new("system-the-configuration-path-is-not-a-table")
                );
                item = &mut item[key];
            }
            if item.is_none() {
                *item = Item::Table(toml_edit::Table::new());
            }
            ensure!(
                item.is_table_like(),
                UiMessage::new("system-the-configuration-path-is-not-a-table")
            );
            let key = edit.path.last().unwrap();
            if edit.value.is_null() {
                if let Some(table) = item.as_table_like_mut() {
                    table.remove(key);
                }
            } else {
                let text =
                    toml::to_string(&std::collections::BTreeMap::from([("value", &edit.value)]))?;
                let parsed = text.parse::<DocumentMut>()?;
                item[key] = parsed.get("value").context("invalid field value")?.clone();
            }
        }
        let source = document.to_string();
        let form = toml::from_str(&source)
            .context(UiMessage::new("system-invalid-configuration-field-type"))?;
        self.source = source;
        self.form = form;
        Ok(())
    }
    pub fn save(&mut self, state_override: Option<&Path>) -> Result<(AppConfig, Store)> {
        self.save_in(&super::paths::home()?, state_override)
    }
    pub fn import_windows_connection(
        &mut self,
        home: &Path,
        input: &crate::environments::registration::WindowsConnection,
        edits: &[ConfigEdit],
    ) -> Result<()> {
        let mut pending = self.clone();
        pending.patch(edits)?;
        let (id, environment) =
            crate::environments::registration::prepare(home, input, &pending.form.environments)?;
        let mut additions = vec![ConfigEdit {
            path: vec!["environments".into(), id.clone()],
            value: environment,
        }];
        if input.make_default || pending.form.default_environment.is_none() {
            additions.push(ConfigEdit {
                path: vec!["default_environment".into()],
                value: id.into(),
            });
        }
        pending.patch(&additions)?;
        *self = pending;
        Ok(())
    }
    fn save_in(
        &mut self,
        home: &Path,
        state_override: Option<&Path>,
    ) -> Result<(AppConfig, Store)> {
        let _maintenance = crate::updates::maintenance::lock(&home.join("updates"), true)?;
        let form: AppConfig = toml::from_str(&self.source)
            .context(UiMessage::new("system-invalid-toml-configuration"))?;
        form.validate()?;
        let parent = self.path.parent().context(UiMessage::new(
            "system-the-configuration-directory-does-not-exist",
        ))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(self.source.as_bytes())?;
        temporary.as_file().sync_all()?;
        let mut resolved = AppConfig::load(Some(temporary.path()))?;
        if let Some(path) = state_override {
            resolved.state_dir = path.to_owned();
        }
        let current = match std::fs::read_to_string(&self.path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        ensure!(
            current == self.original,
            UiMessage::new(
                "system-another-program-changed-this-configuration-reload-before-editing"
            )
        );
        let pins =
            crate::environments::registration::prepare_pin_files(home, &resolved.environments)?;
        crate::cleanup::register_host_pins_at_locked(home, &pins)?;
        let store = Store::open(&resolved.state_dir)?;
        // Keep both the previous and new locations discoverable after a switch.
        if let Ok(previous) = AppConfig::load(Some(&self.path)) {
            crate::cleanup::register_configuration_at_locked(home, &previous, Some(&self.path))?;
        }
        crate::cleanup::register_configuration_at_locked(home, &resolved, Some(&self.path))?;
        temporary
            .persist(&self.path)
            .map_err(|error| error.error)
            .context(UiMessage::new(
                "system-save-failed-the-original-configuration-was-not-replaced",
            ))?;
        self.original = Some(self.source.clone());
        self.form = form;
        Ok((resolved, store))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn importing_a_connection_preserves_drafts_and_tracks_only_the_public_pin_on_save() -> Result<()>
    {
        use crate::environments::registration::WindowsConnection;
        use base64::Engine;
        let root = tempfile::tempdir()?;
        let home = root.path().join("profile");
        let path = root.path().join("config.toml");
        std::fs::write(
            &path,
            "# preserve controller comments\nstate_dir='state'\n[model]\nmodel='before'\n",
        )?;
        let identity = root.path().join("existing-key");
        std::fs::write(&identity, "CONTROLLED-KEY-FIXTURE-NOT-A-CREDENTIAL")?;
        let mut blob = vec![0, 0, 0, 11];
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&[0, 0, 0, 32]);
        blob.extend_from_slice(&[7; 32]);
        let key = wukong_worker_package::SshPublicKey {
            algorithm: "ssh-ed25519".into(),
            key: base64::engine::general_purpose::STANDARD.encode(blob),
        };
        let fingerprint = key.fingerprint()?;
        let mut input = WindowsConnection { name:"Windows VM".into(), host:"192.0.2.1".into(), identity_file:identity.clone(),
            confirmed_fingerprint:String::new(), make_default:false,
            connection_json:serde_json::json!({"schema":1,"installed":true,"installation_id":"a".repeat(32),"environment_id":"windows",
                "generation":1,"manifest_sha256":"b".repeat(64),"ssh_user":format!("wkgm{}","a".repeat(16)),"ssh_port":22,
                "host_key":key,"host_fingerprint":fingerprint,"production_isolation_verified":false}).to_string() };
        let mut document = ConfigDocument::open(path)?;
        let original = document.source.clone();
        let edits = [ConfigEdit {
            path: vec!["model".into(), "model".into()],
            value: "after".into(),
        }];
        assert!(
            document
                .import_windows_connection(&home, &input, &edits)
                .is_err()
        );
        assert_eq!(document.source, original);
        input.confirmed_fingerprint = fingerprint;
        document.import_windows_connection(&home, &input, &edits)?;
        assert_eq!(
            document.form.default_environment.as_deref(),
            Some("windows")
        );
        assert_eq!(document.form.model.model, "after");
        let pin = document.form.environments["windows"]
            .runtime
            .managed_ssh
            .as_ref()
            .unwrap()
            .known_hosts_file
            .clone();
        assert!(
            !pin.exists() && !home.exists(),
            "preview must not create controller files"
        );
        let (_, _store) = document.save_in(&home, None)?;
        assert_eq!(
            std::fs::read_to_string(&pin)?,
            format!("wukong-worker-{} ssh-ed25519 {}\n", "a".repeat(32), key.key)
        );
        assert!(
            document
                .source
                .starts_with("# preserve controller comments")
        );
        let registry = crate::cleanup::Registry::read(&home)?;
        assert!(
            registry
                .locations
                .iter()
                .any(|location| location.path.canonicalize().ok() == pin.canonicalize().ok())
        );
        assert!(
            !registry
                .locations
                .iter()
                .any(|location| location.path.canonicalize().ok() == identity.canonicalize().ok())
        );
        assert_eq!(
            std::fs::read_to_string(identity)?,
            "CONTROLLED-KEY-FIXTURE-NOT-A-CREDENTIAL"
        );
        Ok(())
    }
    #[test]
    fn structured_native_environment_edits_preserve_other_configuration() -> Result<()> {
        use base64::Engine;
        let root = tempfile::tempdir()?;
        let path = root.path().join("config.toml");
        std::fs::write(
            &path,
            "# keep the controller settings\nstate_dir='state'\n[model]\nmodel='unchanged'\n",
        )?;
        let mut blob = vec![0, 0, 0, 11];
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&[0, 0, 0, 32]);
        blob.extend_from_slice(&[7; 32]);
        let environment = serde_json::json!({"name":"Windows VM","platform":"windows",
            "installation_id":"a".repeat(32),"runtime":{"backend":"ssh","ssh_host":"192.0.2.1",
            "managed_ssh":{"installation_id":"a".repeat(32),"user":format!("wkgm{}","a".repeat(16)),
            "port":22,"identity_file":root.path().join("private-key"),"known_hosts_file":root.path().join("host-pin"),
            "host_key":{"algorithm":"ssh-ed25519","key":base64::engine::general_purpose::STANDARD.encode(blob)}}}});
        let mut document = ConfigDocument::open(path)?;
        document.patch(&[
            ConfigEdit {
                path: vec!["environments".into(), "windows".into()],
                value: environment,
            },
            ConfigEdit {
                path: vec!["default_environment".into()],
                value: "windows".into(),
            },
        ])?;
        let (config, _) = document.save_in(&root.path().join("profile"), None)?;
        assert_eq!(config.model.model, "unchanged");
        assert!(
            document
                .source
                .starts_with("# keep the controller settings")
        );
        assert_eq!(
            config.environments["windows"]
                .runtime
                .managed_ssh
                .as_ref()
                .unwrap()
                .user,
            format!("wkgm{}", "a".repeat(16))
        );
        assert!(
            !root.path().join("private-key").exists(),
            "configuration registration must remain lazy"
        );
        let saved = document.source.clone();
        assert!(
            document
                .patch(&[
                    ConfigEdit {
                        path: vec!["model".into(), "model".into()],
                        value: "must-not-be-applied".into(),
                    },
                    ConfigEdit {
                        path: vec!["environments".into(), "invalid".into()],
                        value: serde_json::json!({"name":"Invalid", "platform":17}),
                    },
                ])
                .is_err()
        );
        assert_eq!(document.source, saved);
        assert_eq!(document.form.model.model, "unchanged");
        assert_eq!(std::fs::read_to_string(&document.path)?, saved);
        Ok(())
    }
    #[test]
    fn named_environment_fields_preserve_the_configuration_document() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("config.toml");
        std::fs::write(&path, "# controller settings\nstate_dir='state'\n")?;
        let mut document = ConfigDocument::open(path)?;
        let edit = |path: &[&str], value: &str| ConfigEdit {
            path: path.iter().map(|value| (*value).into()).collect(),
            value: Value::String(value.into()),
        };
        document.patch(&[
            edit(&["default_environment"], "windows"),
            edit(&["environments", "windows", "name"], "Windows VM"),
            edit(&["environments", "windows", "platform"], "windows"),
            edit(&["environments", "windows", "runtime", "backend"], "ssh"),
            edit(
                &["environments", "windows", "runtime", "ssh_host"],
                "windows-fixture",
            ),
        ])?;
        let (config, _) = document.save_in(&root.path().join("profile"), None)?;
        assert_eq!(config.default_environment.as_deref(), Some("windows"));
        assert!(document.source.starts_with("# controller settings"));
        assert!(!document.source.contains("\n[runtime]"));
        Ok(())
    }
    #[test]
    fn editing_preserves_comments_relative_paths_and_external_changes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# keep\nstate_dir = 'state'\n[model]\nmodel = 'before'\n",
        )?;
        let mut document = ConfigDocument::open(path.clone())?;
        document.patch(&[ConfigEdit {
            path: vec!["model".into(), "model".into()],
            value: Value::String("after".into()),
        }])?;
        let home = dir.path().join("profile");
        let (config, _) = document.save_in(&home, None)?;
        assert_eq!(
            config.state_dir,
            std::fs::canonicalize(dir.path())?.join("state")
        );
        assert!(document.source.starts_with("# keep"));
        std::fs::write(&path, "# external\n")?;
        assert!(document.save_in(&home, None).is_err());
        assert_eq!(std::fs::read_to_string(path)?, "# external\n");
        Ok(())
    }
}
