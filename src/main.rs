use clap::Parser;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let mut cli = wukong::interface::Cli::parse();
    let recovery = std::env::var_os("APPIMAGE")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .and_then(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().to_string())
        })
        .is_some_and(|name| name == "Wukong-Recovery");
    if recovery && cli.command.is_none() {
        if !rfd::MessageDialog::new()
            .set_title("Wukong · 恢复上一版本")
            .set_description(
                "请先关闭 Wukong。恢复将重新安装上一版本客户端和工具环境，保留题目与模型配置。",
            )
            .set_buttons(rfd::MessageButtons::OkCancel)
            .show()
            .eq(&rfd::MessageDialogResult::Ok)
        {
            return std::process::ExitCode::SUCCESS;
        }
        cli.command = Some(wukong::interface::Command::RecoverUpdate);
    }
    let desktop = matches!(
        cli.command,
        None | Some(wukong::interface::Command::Gui { .. })
    );
    #[cfg(windows)]
    if desktop {
        // Detach only the graphical entry point; CLI stdout remains usable in pipelines.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn FreeConsole() -> i32;
        }
        unsafe {
            FreeConsole();
        }
    }
    std::panic::set_hook(Box::new(|info| {
        // Keep diagnostics available when launched from the desktop.
        let message = format!("wukong: internal error: {info}");
        let _ = std::fs::write("wukong-panic.log", &message);
        eprintln!("{message}");
    }));
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    match wukong::interface::run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wukong: {error:#}");
            if desktop || recovery {
                rfd::MessageDialog::new()
                    .set_title("Wukong · 启动失败")
                    .set_description(format!("{error:#}"))
                    .set_level(rfd::MessageLevel::Error)
                    .show();
            }
            std::process::ExitCode::FAILURE
        }
    }
}
