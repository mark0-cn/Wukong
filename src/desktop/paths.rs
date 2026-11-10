//! Installed applications keep writable state outside their executable directory.
use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn home() -> Result<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    // NSIS currentUser installs the program in LOCALAPPDATA/Wukong.
    // Keep state and recovery files outside that directory.
    Ok(base
        .context("user application directory is unavailable")?
        .join(if cfg!(windows) {
            "WukongData"
        } else {
            "Wukong"
        }))
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn data_is_outside_nsis_default_install_directory() {
        let installation =
            std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("Wukong");
        assert!(!super::home().unwrap().starts_with(installation));
    }
}

pub fn default_config() -> Result<PathBuf> {
    let home = home()?;
    std::fs::create_dir_all(&home)?;
    let path = home.join("wukong.toml");
    if !path.exists() {
        // create_new prevents concurrent first launches from overwriting configuration.
        let mut config = crate::config::AppConfig::default();
        config.resolve_paths(&home);
        use std::io::Write;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(toml::to_string_pretty(&config)?.as_bytes())?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}
