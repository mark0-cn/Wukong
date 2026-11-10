//! Recovery works without a webview, model credentials, or network access.
use super::{
    environment, maintenance,
    manifest::{self, Manifest},
};
use crate::workbench::{read_json, write_json};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct Journal {
    pub from: String,
    pub to: String,
    pub config: PathBuf,
    pub destination: PathBuf,
    pub previous_image: String,
}

pub fn root() -> Result<PathBuf> {
    Ok(crate::desktop::paths::home()?.join("updates"))
}
pub fn cache(version: &str) -> Result<PathBuf> {
    semver::Version::parse(version)?;
    ensure!(!version.contains(['/', '\\']), "invalid version path");
    Ok(root()?.join("releases").join(version))
}
pub fn cached(version: &str) -> Result<Manifest> {
    let path = cache(version)?;
    let manifest = Manifest::parse(
        &std::fs::read(path.join("latest.json"))?,
        &std::fs::read_to_string(path.join("latest.json.sig"))?,
    )?;
    ensure!(manifest.version == version, "cached version mismatch");
    Ok(manifest)
}
pub fn payload(manifest: &Manifest) -> Result<PathBuf> {
    let name = manifest
        .artifact()?
        .url
        .rsplit('/')
        .next()
        .context("missing filename")?;
    Ok(cache(&manifest.version)?.join(name))
}
pub fn executable() -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(path) = std::env::var_os("APPIMAGE") {
        return Ok(PathBuf::from(path));
    }
    std::env::current_exe().map_err(Into::into)
}
pub fn stage(journal: &Journal) -> Result<PathBuf> {
    ensure!(
        journal.config.is_absolute() && journal.destination.is_absolute(),
        "recovery paths must be absolute"
    );
    let previous = cached(&journal.from)?;
    manifest::verify_file(&payload(&previous)?, previous.artifact()?)?;
    let source = executable()?;
    let helper = root()?.join(if cfg!(windows) {
        "Wukong-Recovery.exe"
    } else {
        "Wukong-Recovery.AppImage"
    });
    std::fs::copy(source, &helper)?;
    write_json(&root()?.join("pending.json"), journal)?;
    Ok(helper)
}
pub fn pending() -> Result<Option<Journal>> {
    let path = root()?.join("pending.json");
    if path.exists() {
        Ok(Some(read_json(&path)?))
    } else {
        Ok(None)
    }
}
pub fn confirm() -> Result<bool> {
    let Some(journal) = pending()? else {
        return Ok(false);
    };
    ensure!(
        journal.to == env!("CARGO_PKG_VERSION"),
        "update incomplete; use the recovery entry before starting analysis"
    );
    let _lock = maintenance::lock(&root()?, true)?;
    let manifest = cached(&journal.to)?;
    environment::activate(&journal.config, &manifest.image)?;
    write_json(&root()?.join("previous.json"), &journal)?;
    std::fs::remove_file(root()?.join("pending.json"))?;
    Ok(true)
}
pub fn recover() -> Result<()> {
    let _lock = maintenance::lock(&root()?, true)?;
    let journal = pending()?
        .or_else(|| read_json::<Journal>(&root().ok()?.join("previous.json")).ok())
        .context("no recoverable update")?;
    let previous = cached(&journal.from)?;
    let installer = payload(&previous)?;
    manifest::verify_file(&installer, previous.artifact()?)?;
    ensure!(
        journal.destination.is_absolute() && journal.config.is_absolute(),
        "invalid recovery paths"
    );
    // Restore the environment first. If it fails, keep the journal and installer.
    environment::activate(&journal.config, &journal.previous_image)?;
    #[cfg(windows)]
    {
        let directory = journal
            .destination
            .parent()
            .context("missing install directory")?;
        let result = std::process::Command::new(&installer)
            .arg("/S")
            .arg(format!("/D={}", directory.display()))
            .status()?;
        ensure!(
            result.success(),
            "previous installer failed; recovery record retained"
        );
    }
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let mut temporary = tempfile::NamedTempFile::new_in(
            journal
                .destination
                .parent()
                .context("missing install directory")?,
        )?;
        temporary.write_all(&std::fs::read(&installer)?)?;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&journal.destination)
            .map_err(|e| e.error)?;
    }
    for name in ["pending.json", "previous.json"] {
        let path = root()?.join(name);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub fn integrate_linux() -> Result<PathBuf> {
    ensure!(
        cfg!(target_os = "linux"),
        "desktop integration is Linux-only"
    );
    let source = std::env::var_os("APPIMAGE")
        .context("launch the AppImage to install desktop integration")?;
    let directory = crate::desktop::paths::home()?.join("application");
    std::fs::create_dir_all(&directory)?;
    let destination = directory.join("Wukong.AppImage");
    if Path::new(&source) != destination {
        std::fs::copy(source, &destination)?;
    }
    let icon = directory.join("wukong.svg");
    std::fs::write(&icon, include_bytes!("../../icons/icon.svg"))?;
    let apps = crate::desktop::paths::home()?
        .parent()
        .context("missing data home")?
        .join("applications");
    std::fs::create_dir_all(&apps)?;
    let quote = |path: &Path| -> Result<String> {
        let text = path.to_str().context("desktop path must be UTF-8")?;
        ensure!(!text.contains(['\n', '\r']), "invalid desktop path");
        Ok(format!(
            "\"{}\"",
            text.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('`', "\\`")
                .replace('$', "\\$")
                .replace('%', "%%")
        ))
    };
    std::fs::write(
        apps.join("io.wukong.desktop.desktop"),
        format!(
            "[Desktop Entry]\nType=Application\nName=Wukong\nExec={}\nIcon={}\nTerminal=false\nCategories=Development;\n",
            quote(&destination)?,
            icon.display()
        ),
    )?;
    Ok(destination)
}
