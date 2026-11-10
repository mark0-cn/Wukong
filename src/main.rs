use clap::Parser;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = wukong::interface::Cli::parse();
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
            if desktop {
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
