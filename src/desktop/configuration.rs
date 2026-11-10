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
                !edit.path.is_empty() && edit.path.len() <= 3,
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
                item[key] = Item::Value(
                    parsed["value"]
                        .as_value()
                        .context("invalid field value")?
                        .clone(),
                );
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
        let store = Store::open(&resolved.state_dir)?;
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
        let (config, _) = document.save(None)?;
        assert_eq!(
            config.state_dir,
            std::fs::canonicalize(dir.path())?.join("state")
        );
        assert!(document.source.starts_with("# keep"));
        std::fs::write(&path, "# external\n")?;
        assert!(document.save(None).is_err());
        assert_eq!(std::fs::read_to_string(path)?, "# external\n");
        Ok(())
    }
}
