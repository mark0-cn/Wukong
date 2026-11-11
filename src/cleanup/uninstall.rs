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
        if !status.code().is_some_and(|code| (32..40).contains(&code)) {
            return Ok(());
        }
        let cleanup = Cleanup::current()?;
        let plan = read_plan(&review)?;
        if plan.selection.any() {
            let report = cleanup.execute(&plan)?;
            crate::workbench::write_json(&temp.path().join("report.json"), &report)?;
            if !report.complete {
                let saved=temp.keep();
                anyhow::bail!("cleanup incomplete: {}; report: {}",report.errors.join("; "),saved.join("report.json").display());
            }
        }
        let installed = cleanup.home.join("application/Wukong.AppImage");
        if files::absolute(&exe)? == files::absolute(&installed)? {
            files::safe(&installed)?;
            let base = cleanup.home.parent().context("application home missing")?;
            let desktop = base.join("applications/io.wukong.desktop.desktop");
            if desktop.is_file() {
                files::safe(&desktop)?;
                let content = std::fs::read_to_string(&desktop)?;
                ensure!(
                    content.lines().any(|line| line.starts_with("Exec=")
                        && line.contains(&installed.to_string_lossy().to_string())),
                    "desktop launcher ownership changed"
                );
                std::fs::remove_file(desktop)?;
            }
            std::fs::remove_file(installed)?;
            let icon = cleanup.home.join("application/wukong.svg");
            if icon.is_file() {
                files::safe(&icon)?;
                std::fs::remove_file(icon)?;
            }
            let _ = std::fs::remove_dir(cleanup.home.join("application"));
            let _ = std::fs::remove_dir(&cleanup.home);
        } else {
            println!("Portable application retained: {}", exe.display());
        }
    }
    Ok(())
}
