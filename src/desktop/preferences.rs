//! Language preferences are independent of workspace epochs, model config and active runs.
use crate::i18n::Locale;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{io::Write, path::Path};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum LanguageChoice {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en-US")]
    EnUs,
}
impl LanguageChoice {
    pub fn resolve(self, system: Locale) -> Locale {
        match self {
            Self::Auto => system,
            Self::ZhCn => Locale::ZhCn,
            Self::EnUs => Locale::EnUs,
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub ui_language: LanguageChoice,
    pub output_language: LanguageChoice,
}
#[derive(Serialize)]
pub struct Snapshot {
    #[serde(flatten)]
    pub preferences: Preferences,
    pub system_locale: Locale,
}
pub fn read_at(path: &Path) -> Result<Preferences> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Preferences::default()),
        Err(error) => Err(error.into()),
    }
}
pub fn write_at(path: &Path, preferences: &Preferences) -> Result<()> {
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&serde_json::to_vec_pretty(preferences)?)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}
pub fn read() -> Result<Snapshot> {
    Ok(Snapshot {
        preferences: read_at(&super::paths::home()?.join("preferences.json"))?,
        system_locale: Locale::system(),
    })
}
pub fn write(preferences: &Preferences) -> Result<Snapshot> {
    write_at(&super::paths::home()?.join("preferences.json"), preferences)?;
    Ok(Snapshot {
        preferences: preferences.clone(),
        system_locale: Locale::system(),
    })
}
pub fn locale() -> Locale {
    read()
        .map(|s| s.preferences.ui_language.resolve(s.system_locale))
        .unwrap_or_else(|_| Locale::system())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_replace_atomically_without_touching_workspace() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("preferences.json");
        assert_eq!(read_at(&path).unwrap(), Preferences::default());
        let config = root.path().join("wukong.toml");
        std::fs::write(&config, "untouched").unwrap();
        for choice in [LanguageChoice::ZhCn, LanguageChoice::EnUs] {
            let value = Preferences {
                ui_language: choice,
                output_language: LanguageChoice::Auto,
            };
            write_at(&path, &value).unwrap();
            assert_eq!(read_at(&path).unwrap(), value);
        }
        assert_eq!(std::fs::read_to_string(config).unwrap(), "untouched");
        assert!(serde_json::from_str::<Preferences>(r#"{"ui_language":"unknown"}"#).is_err());
    }
}
