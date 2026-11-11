//! The launcher owns the temporary profile until the entire GUI process exits.
use super::*;
use std::process::{Command, Stdio};
pub fn choose(output: PathBuf, initial_data: bool, diagnostics: bool) -> Result<()> {
    let profile = tempfile::Builder::new()
        .prefix("wukong-review-")
        .tempdir()?;
    let mut command = Command::new(crate::updates::recovery::executable()?);
    if diagnostics {
        command.arg("--json");
    }
    command
        .args(["cleanup", "review-window", "--output"])
        .arg(output)
        .arg("--profile")
        .arg(profile.path());
    if initial_data {
        command.arg("--initial-data");
    }
    command.stdin(Stdio::null());
    if !diagnostics {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(target_os = "linux")]
    command.env("APPIMAGE_EXTRACT_AND_RUN", "1");
    let status = command.status()?;
    release(profile)?;
    std::process::exit(status.code().unwrap_or(1));
}
pub fn validate_profile(path: &Path) -> Result<PathBuf> {
    let root = std::fs::canonicalize(path)?;
    let parent = std::fs::canonicalize(std::env::temp_dir())?;
    ensure!(
        root.parent() == Some(parent.as_path())
            && root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("wukong-review-")),
        "unexpected review profile location"
    );
    Ok(root)
}
fn release(profile: tempfile::TempDir) -> Result<()> {
    let root = validate_profile(profile.path())?;
    if profile.close().is_ok() {
        return Ok(());
    }
    // The GUI host is gone; WebView2 can briefly retain background file handles.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if !root.try_exists()? {
            return Ok(());
        }
        ensure!(
            validate_profile(&root)? == root,
            "review profile location changed"
        );
        match std::fs::remove_dir_all(&root) {
            Ok(()) => return Ok(()),
            Err(error) if std::time::Instant::now() >= deadline => {
                return Err(error).context("temporary review profile could not be removed");
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}
