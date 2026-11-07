use clap::Parser;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    std::panic::set_hook(Box::new(|info| {
        // The panic fires before the terminal is restored, so the console
        // copy is invisible; the file survives for diagnosis.
        let message = format!("re-agent: internal error: {info}");
        let _ = std::fs::write("re-agent-panic.log", &message);
        eprintln!("{message}");
    }));
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    match re_agent::interface::run(re_agent::interface::Cli::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("re-agent: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
