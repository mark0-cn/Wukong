#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = wukong::interface::Cli::parse_localized();
    let locale = cli.resolved_locale();
    let recovery = std::env::var_os("APPIMAGE")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .and_then(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().to_string())
        })
        .is_some_and(|name| name == "Wukong-Recovery");
    let recover = recovery && cli.command.is_none();
    if recover
        && !rfd::MessageDialog::new()
            .set_title(locale.text("cli-recovery-title"))
            .set_description(locale.text("cli-recovery-description"))
            .set_buttons(rfd::MessageButtons::OkCancel)
            .show()
            .eq(&rfd::MessageDialogResult::Ok)
    {
        return std::process::ExitCode::SUCCESS;
    }
    let desktop = matches!(
        cli.command,
        None | Some(wukong::interface::Command::Gui { .. })
            | Some(wukong::interface::Command::Cleanup {
                command: wukong::cleanup::cli::Command::Choose { .. }
                    | wukong::cleanup::cli::Command::ReviewWindow { .. }
            })
    );
    let interactive_diagnostics = !cli.json;
    #[cfg(windows)]
    if desktop && interactive_diagnostics {
        // Detach only the graphical entry point; CLI stdout remains usable in pipelines.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn FreeConsole() -> i32;
        }
        // SAFETY: `FreeConsole` takes no arguments and only detaches this process
        // from its console; a failure returns zero and has no other effect.
        unsafe {
            FreeConsole();
        }
    }
    std::panic::set_hook(Box::new(|info| {
        // Keep diagnostics available when launched from the desktop: the working
        // directory is arbitrary and may be read-only, so use the OS temp directory
        // and say where the report went.
        let message = format!("wukong: internal error: {info}");
        let path = std::env::temp_dir().join("wukong-panic.log");
        match std::fs::write(&path, &message) {
            Ok(()) => eprintln!("{message} (report: {})", path.display()),
            Err(_) => eprintln!("{message}"),
        }
    }));
    let telemetry = wukong::adapters::telemetry::TelemetryRuntime::new();
    let result = if recover {
        wukong::updates::recovery::recover()
    } else {
        wukong::interface::run(cli, &telemetry).await
    };
    telemetry.shutdown().await;
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let message = wukong::i18n::Message::from_error(error).render(locale);
            eprintln!("wukong: {message}");
            if (desktop || recovery) && interactive_diagnostics {
                rfd::MessageDialog::new()
                    .set_title(locale.text("cli-startup-failed"))
                    .set_description(message)
                    .set_level(rfd::MessageLevel::Error)
                    .show();
            }
            std::process::ExitCode::FAILURE
        }
    }
}
