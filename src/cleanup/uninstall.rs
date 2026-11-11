use super::*;
pub fn run() -> Result<()> {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe()?;
        let uninstall = exe
            .parent()
            .context("application directory missing")?
            .join("uninstall.exe");
        ensure!(
            uninstall.is_file(),
            "installed uninstaller is unavailable; portable copies can be removed manually"
        );
        std::process::Command::new(uninstall).spawn()?;
    }
    #[cfg(not(windows))]
    {
        let temp = tempfile::tempdir()?;
        let review = temp.path().join("review.json");
        let exe = crate::updates::recovery::executable()?;
        let status = std::process::Command::new(&exe)
            .args(["cleanup", "choose", "--output"])
            .arg(&review)
            .env("APPIMAGE_EXTRACT_AND_RUN", "1")
            .status()?;
        if status.code() == Some(64) {
            return Ok(());
        }
        ensure!(
            status.code().is_some_and(|code| (32..40).contains(&code)),
            "uninstall review failed: {status}"
        );
        let cleanup = Cleanup::current()?;
        let plan = read_plan(&review)?;
        if plan.selection.any() {
            let report = cleanup.execute(&plan)?;
            crate::workbench::write_json(&temp.path().join("report.json"), &report)?;
            if !report.complete {
                let saved = temp.keep();
                anyhow::bail!(
                    "cleanup incomplete: {}; report: {}",
                    report.errors.join("; "),
                    saved.join("report.json").display()
                );
            }
        }
        if !remove_integration(&cleanup.home, &exe)? {
            println!("Portable application retained: {}", exe.display());
        }
    }
    Ok(())
}
#[cfg(not(windows))]
fn remove_integration(home: &Path, exe: &Path) -> Result<bool> {
    let installed = home.join("application/Wukong.AppImage");
    if files::absolute(exe)? == files::absolute(&installed)? {
        files::safe(&installed)?;
        let base = home.parent().context("application home missing")?;
        let desktop = base.join("applications/io.wukong.desktop.desktop");
        if desktop.is_file() {
            files::safe(&desktop)?;
            let content = std::fs::read_to_string(&desktop)?;
            let expected = format!("Exec={}", crate::desktop::paths::desktop_exec(&installed)?);
            ensure!(
                content.lines().any(|line| line == expected),
                "desktop launcher ownership changed"
            );
            std::fs::remove_file(desktop)?;
        }
        std::fs::remove_file(installed)?;
        let icon = home.join("application/wukong.svg");
        if icon.is_file() {
            files::safe(&icon)?;
            std::fs::remove_file(icon)?;
        }
        let _ = std::fs::remove_dir(home.join("application"));
        let _ = std::fs::remove_dir(home);
        return Ok(true);
    }
    Ok(false)
}
#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    #[test]
    fn linux_removal_requires_managed_executable_and_exact_launcher_identity() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("Wukong");
        let installed = home.join("application/Wukong.AppImage");
        std::fs::create_dir_all(installed.parent().unwrap())?;
        std::fs::write(&installed, b"trusted fixture")?;
        let portable = temp.path().join("download.AppImage");
        std::fs::write(&portable, b"original")?;
        assert!(!remove_integration(&home, &portable)?);
        assert!(portable.exists() && installed.exists());
        let launcher = temp.path().join("applications/io.wukong.desktop.desktop");
        std::fs::create_dir_all(launcher.parent().unwrap())?;
        std::fs::write(
            &launcher,
            format!("Exec=/bin/echo {}\n", installed.display()),
        )?;
        assert!(remove_integration(&home, &installed).is_err());
        assert!(installed.exists() && launcher.exists());
        std::fs::write(
            &launcher,
            format!(
                "[Desktop Entry]\nExec={}\n",
                crate::desktop::paths::desktop_exec(&installed)?
            ),
        )?;
        assert!(remove_integration(&home, &installed)?);
        assert!(!installed.exists() && !launcher.exists() && portable.exists());
        Ok(())
    }
}
