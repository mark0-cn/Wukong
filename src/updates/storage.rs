//! Relocatable bulk cache; recovery journals and process locks stay at a stable address.
use super::{maintenance, manifest::Manifest, recovery};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct Location {
    directory: PathBuf,
}
pub fn cache_root() -> Result<PathBuf> {
    let control = recovery::root()?;
    let settings = control.join("cache-location.json");
    if settings.exists() {
        let location: Location = crate::workbench::read_json(&settings)?;
        ensure!(location.directory.is_absolute(), "缓存目录必须是绝对路径");
        Ok(location.directory)
    } else {
        Ok(control)
    }
}
pub fn cache_lock(exclusive: bool) -> Result<fs::File> {
    let root = recovery::root()?;
    fs::create_dir_all(&root)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("cache.lock"))?;
    if exclusive {
        file.try_lock().context("更新下载或恢复正在使用缓存")?;
    } else {
        file.try_lock_shared().context("缓存位置正在切换")?;
    }
    Ok(file)
}

pub fn set_cache(destination: &Path) -> Result<PathBuf> {
    let _cache = cache_lock(true)?;
    let _maintenance = maintenance::lock(&recovery::root()?, true)?;
    ensure!(
        recovery::pending()?.is_none(),
        "先完成待处理更新或恢复，再修改缓存目录"
    );
    ensure!(destination.is_absolute(), "请选择绝对路径");
    fs::create_dir_all(destination)?;
    let destination = fs::canonicalize(destination)?;
    let source = cache_root()?;
    fs::create_dir_all(&source)?;
    let source = fs::canonicalize(source)?;
    if source == destination {
        return Ok(destination);
    }
    ensure!(
        !source.starts_with(&destination) && !destination.starts_with(&source),
        "新旧缓存目录不能相互包含"
    );
    if let Some(program) = recovery::executable()?.parent() {
        ensure!(
            !destination.starts_with(fs::canonicalize(program)?),
            "缓存目录不能位于程序安装目录内"
        );
    }
    ensure!(
        fs::read_dir(&destination)?.next().is_none(),
        "请选择一个空目录；不会覆盖已有文件"
    );
    let staging = tempfile::tempdir_in(&destination)?;
    let releases = source.join("releases");
    if releases.exists() {
        copy_releases(&releases, &staging.path().join("releases"))?;
    }
    let staged = staging.path().join("releases");
    if staged.exists() {
        fs::rename(staged, destination.join("releases"))?;
    }
    crate::workbench::write_json(
        &recovery::root()?.join("cache-location.json"),
        &Location {
            directory: destination.clone(),
        },
    )?;
    // Old files are preserved. Only the small pointer switches after a complete copy.
    Ok(destination)
}

fn copy_releases(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        ensure!(
            entry.file_type()?.is_dir() && !entry.file_type()?.is_symlink(),
            "缓存包含非版本目录"
        );
        let version = entry.file_name().to_string_lossy().to_string();
        semver::Version::parse(&version)?;
        let path = entry.path();
        let bytes = fs::read(path.join("latest.json"))?;
        let signature = fs::read_to_string(path.join("latest.json.sig"))?;
        let manifest = Manifest::parse(&bytes, &signature)?;
        ensure!(manifest.version == version, "缓存版本身份不一致");
        let output = destination.join(&version);
        fs::create_dir_all(&output)?;
        for file in ["latest.json", "latest.json.sig"] {
            fs::copy(path.join(file), output.join(file))?;
        }
        let artifact = manifest.artifact()?;
        let filename = artifact.url.rsplit('/').next().unwrap();
        let payload = path.join(filename);
        if payload.exists() {
            ensure!(!payload.is_symlink(), "缓存文件不能是符号链接");
            super::manifest::verify_file(&payload, artifact)?;
            ensure!(
                fs2::available_space(&output)? > artifact.size + 64 * 1024 * 1024,
                "新缓存目录空间不足"
            );
            fs::copy(payload, output.join(filename))?;
            super::manifest::verify_file(&output.join(filename), artifact)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_release_cannot_be_migrated() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let input = dir.path().join("source/0.2.0");
        fs::create_dir_all(&input)?;
        fs::write(input.join("latest.json"), b"{}")?;
        fs::write(input.join("latest.json.sig"), b"bad")?;
        let output = dir.path().join("destination");
        assert!(copy_releases(&dir.path().join("source"), &output).is_err());
        assert!(!output.exists());
        assert!(input.join("latest.json").exists());
        Ok(())
    }
}
