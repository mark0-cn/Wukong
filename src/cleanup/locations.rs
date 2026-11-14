//! Track configuration locations, not secrets or ownership of arbitrary parent folders.
use super::*;
use crate::{
    config::AppConfig,
    workbench::{read_json, write_json},
};
use std::collections::BTreeSet;
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum LocationKind {
    State,
    Worker,
    Ghidra,
    Workbench,
    Cache,
    Config,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    pub kind: LocationKind,
    pub path: PathBuf,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub schema_version: u32,
    pub application: String,
    #[serde(default)]
    pub locations: BTreeSet<Location>,
}
impl Default for Registry {
    fn default() -> Self {
        Self {
            schema_version: 1,
            application: "io.wukong.desktop".into(),
            locations: BTreeSet::new(),
        }
    }
}
impl Registry {
    pub fn read(home: &Path) -> Result<Self> {
        let path = home.join("managed-locations.json");
        if !path.exists() {
            return Ok(Self::default());
        }
        files::safe(&path)?;
        ensure!(
            std::fs::metadata(&path)?.len() <= 1024 * 1024,
            "managed-location registry is too large"
        );
        let mut value: Self = read_json(&path)?;
        ensure!(
            value.schema_version == 1 && value.application == "io.wukong.desktop",
            "invalid managed-location registry identity"
        );
        ensure!(value.locations.len() <= 1024, "too many managed locations");
        value.locations = value
            .locations
            .into_iter()
            .map(|entry| {
                Ok(Location {
                    kind: entry.kind,
                    path: files::absolute(&entry.path)?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(value)
    }
    pub fn include_config(&mut self, config: &AppConfig, path: Option<&Path>) -> Result<()> {
        for (kind, path) in [
            (LocationKind::State, &config.state_dir),
            (LocationKind::Ghidra, &config.analysis.ghidra_cache_dir),
            (LocationKind::Workbench, &config.workbench.root),
        ] {
            self.locations.insert(Location {
                kind,
                path: files::absolute(path)?,
            });
        }
        for environment in config.environments.values() {
            if environment.runtime.backend == crate::runtime::RuntimeBackend::TrustedLocal {
                self.locations.insert(Location {
                    kind: LocationKind::Worker,
                    path: files::absolute(&environment.runtime.state_dir)?,
                });
            }
        }
        if let Some(path) = path {
            self.locations.insert(Location {
                kind: LocationKind::Config,
                path: files::absolute(path)?,
            });
        }
        Ok(())
    }
}
pub fn register_configuration(config: &AppConfig, path: Option<&Path>) -> Result<()> {
    let home = crate::desktop::paths::home()?;
    let _lock = crate::updates::maintenance::lock(&home.join("updates"), false)?;
    register_configuration_at_locked(&home, config, path)
}
pub(crate) fn register_configuration_at_locked(
    home: &Path,
    config: &AppConfig,
    path: Option<&Path>,
) -> Result<()> {
    let mut registry = Registry::read(home)?;
    registry.include_config(config, path)?;
    // Serialize writers independently of the shared analysis/maintenance lock.
    save(home, registry)
}
fn save(home: &Path, additions: Registry) -> Result<()> {
    std::fs::create_dir_all(home)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.join("locations.lock"))?;
    file.lock()?;
    let mut registry = Registry::read(home)?;
    registry.locations.extend(additions.locations);
    write_json(&home.join("managed-locations.json"), &registry)
}
pub(crate) fn register_host_pins_at_locked(home: &Path, paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut additions = Registry::default();
    for path in paths {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("invalid public host pin name")?;
        let digest = name
            .strip_prefix("ssh-host-")
            .and_then(|name| name.strip_suffix(".known_hosts"))
            .context("unrecognized public host pin name")?;
        ensure!(
            path.parent() == Some(home)
                && digest.len() == 64
                && digest
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "host pin is outside its fixed controller settings location"
        );
        files::safe(path)?;
        additions.locations.insert(Location {
            kind: LocationKind::Config,
            path: files::absolute(path)?,
        });
    }
    save(home, additions)
}
pub fn register_cache(home: &Path, path: &Path) -> Result<()> {
    let mut registry = Registry::default();
    registry.locations.insert(Location {
        kind: LocationKind::Cache,
        path: files::absolute(path)?,
    });
    save(home, registry)
}
pub(super) fn inventory(home: &Path) -> Result<Registry> {
    let mut registry = Registry::read(home)?;
    let mut defaults = AppConfig::default();
    defaults.resolve_paths(home);
    let config_path = home.join("wukong.toml");
    registry.include_config(&defaults, Some(&config_path))?;
    if config_path.is_file() {
        files::safe(&config_path)?;
        ensure!(
            std::fs::metadata(&config_path)?.len() <= 1024 * 1024,
            "configuration is too large"
        );
        let raw = std::fs::read_to_string(&config_path)?;
        // Ignore invalid model configuration: cleanup needs paths, never credentials.
        let value: toml::Value = toml::from_str(&raw)
            .map_err(|_| anyhow::anyhow!("cannot inspect paths in invalid configuration"))?;
        for (kind, keys) in [
            (LocationKind::State, vec!["state_dir"]),
            (LocationKind::Ghidra, vec!["analysis", "ghidra_cache_dir"]),
            (LocationKind::Workbench, vec!["workbench", "root"]),
        ] {
            let item = keys
                .into_iter()
                .try_fold(&value, |value, key| value.get(key));
            if let Some(path) = item.and_then(toml::Value::as_str) {
                let path = PathBuf::from(path);
                registry.locations.insert(Location {
                    kind,
                    path: files::absolute(&if path.is_absolute() {
                        path
                    } else {
                        home.join(path)
                    })?,
                });
            }
        }
        if let Some(environments) = value.get("environments").and_then(toml::Value::as_table) {
            for environment in environments.values() {
                let Some(runtime) = environment.get("runtime") else {
                    continue;
                };
                if runtime.get("backend").and_then(toml::Value::as_str) != Some("trusted_local") {
                    continue;
                }
                let path = runtime
                    .get("state_dir")
                    .and_then(toml::Value::as_str)
                    .map(PathBuf::from)
                    .unwrap_or_else(|| crate::runtime::RuntimeConfig::default().state_dir);
                registry.locations.insert(Location {
                    kind: LocationKind::Worker,
                    path: files::absolute(&if path.is_absolute() {
                        path
                    } else {
                        home.join(path)
                    })?,
                });
            }
        }
    }
    let cache_pointer = home.join("updates/cache-location.json");
    if cache_pointer.is_file() {
        files::safe(&cache_pointer)?;
        let value: serde_json::Value = read_json(&cache_pointer)?;
        let path = PathBuf::from(
            value["directory"]
                .as_str()
                .context("invalid cache location")?,
        );
        ensure!(path.is_absolute(), "cache location must be absolute");
        registry.locations.insert(Location {
            kind: LocationKind::Cache,
            path: files::absolute(&path)?,
        });
    }
    registry.locations.insert(Location {
        kind: LocationKind::Cache,
        path: home.join("updates"),
    });
    Ok(registry)
}

/// A marker is written only when Wukong creates a new directory. Existing
/// arbitrary folders are never adopted just because a config names them.
pub fn create_owned_directory(path: &Path) -> Result<()> {
    let new = !path.try_exists()?;
    std::fs::create_dir_all(path)?;
    if new && files::safe(path).is_ok() {
        write_json(
            &path.join(".wukong-owned.json"),
            &serde_json::json!({"schema_version":1,"application":"io.wukong.desktop"}),
        )?;
    }
    Ok(())
}
pub(super) fn owns_directory(path: &Path) -> bool {
    let marker = path.join(".wukong-owned.json");
    files::safe(&marker).is_ok()
        && read_json::<serde_json::Value>(&marker).is_ok_and(|value| {
            value["schema_version"] == 1 && value["application"] == "io.wukong.desktop"
        })
}
