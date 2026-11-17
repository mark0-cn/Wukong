pub mod cleanup;
pub mod lab;
pub mod workbench;
use crate::{
    adapters::binary,
    application::assessment::{AssessmentService, PreparedAssessment},
    config::AppConfig,
    domain::*,
    interfaces::report,
    storage::Store,
};
use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
mod localization;

#[derive(Parser)]
#[command(
    name = "wukong",
    version,
    about = "Strix-inspired Rust/Rig binary analysis agents"
)]
pub struct Cli {
    #[arg(long, global = true, env = "WUKONG_LOCALE", default_value = "auto", value_parser = ["auto", "en-US", "zh-CN"])]
    pub locale: String,
    #[arg(long, global = true, env = "WUKONG_OUTPUT_LANGUAGE", value_enum)]
    pub output_language: Option<crate::i18n::Locale>,
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    #[arg(long, global = true)]
    pub state: Option<PathBuf>,
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}
#[derive(Subcommand)]
pub enum Command {
    /// Preview or remove explicitly selected local application data.
    Cleanup {
        #[command(subcommand)]
        command: crate::interfaces::cli::cleanup::Command,
    },
    /// Manage the portable tool environment on the Docker host.
    Env {
        #[command(subcommand)]
        command: crate::interfaces::cli::workbench::EnvCommand,
    },
    /// Import and manage isolated Pwn challenges.
    Challenge {
        #[command(subcommand)]
        command: crate::interfaces::cli::workbench::ChallengeCommand,
    },
    /// Manage the registered Windows experiment VM lifecycle without a model.
    Lab {
        #[command(subcommand)]
        command: crate::interfaces::cli::lab::Command,
    },
    /// Write a configuration without overwriting an existing file.
    Init {
        #[arg(default_value = "wukong.toml")]
        path: PathBuf,
    },
    /// Inspect model settings and installed analysis capabilities without calling a model.
    Doctor,
    /// Parse a binary without running it or calling a model.
    Inspect {
        target: PathBuf,
        #[arg(long)]
        strings: bool,
    },
    /// Start a real model-driven multi-agent assessment. TARGET may be a
    /// binary or a directory bundle; directories are scanned automatically.
    Analyze {
        target: PathBuf,
        #[arg(long, default_value = DEFAULT_TASK)]
        task: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Resume nonterminal work using its saved configuration and checkpoints.
    Resume {
        run_id: String,
        #[arg(long)]
        max_tokens: Option<u64>,
        #[arg(long)]
        max_usd: Option<f64>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Runs,
    Status {
        run_id: String,
        #[arg(long, default_value_t = 20)]
        events: usize,
    },
    /// Resolve an uncertain external effect using an operator-reviewed result and evidence.
    Reconcile {
        call_id: String,
        #[arg(long)]
        result_file: PathBuf,
        #[arg(long = "evidence-id", required = true)]
        evidence_ids: Vec<String>,
        #[arg(long)]
        error: bool,
    },
    /// Open the desktop workspace, optionally selecting a saved assessment.
    Gui {
        run_id: Option<String>,
    },
}

pub async fn run(cli: Cli) -> Result<()> {
    let locale = cli.resolved_locale();
    let command = cli.command.unwrap_or(Command::Gui { run_id: None });
    if let Command::Cleanup { command } = command {
        return crate::interfaces::cli::cleanup::run(command, cli.json);
    }
    if let Command::Init { path } = &command {
        AppConfig::write_example(path)?;
        println!("{}", json!({"config":path}));
        return Ok(());
    }
    // Explicit/portable configurations remain supported. New desktop installs
    // use writable per-user storage, independent of installer replacement.
    let mut path = config_path(
        cli.config.clone(),
        matches!(command, Command::Gui { .. }),
        &std::env::current_dir()?,
        std::env::current_exe().ok().as_deref(),
    );
    if path.is_none() && matches!(command, Command::Gui { .. }) {
        path = Some(crate::adapters::files::paths::default_config()?);
    }
    let mut config = AppConfig::load(path.as_deref())?;
    if let Some(language) = cli.output_language {
        config.output_language = Some(language);
    }
    if let Some(state) = &cli.state {
        config.state_dir = state.clone();
    }
    if let Command::Inspect { target, strings } = &command {
        let profile = binary::inspect(target)?;
        let result = if *strings {
            json!({"binary":profile,"strings":binary::strings(target,&binary::StringsQuery::default())?})
        } else {
            serde_json::to_value(profile)?
        };
        print_json(&result)?;
        return Ok(());
    }
    crate::application::maintenance::cleanup::register_configuration(&config, path.as_deref())?;
    if let Command::Env { command } = command {
        print_json(&crate::interfaces::cli::workbench::environment(
            &config, command,
        )?)?;
        return Ok(());
    }
    if let Command::Challenge { command } = command {
        print_json(&crate::interfaces::cli::workbench::challenge(
            &config, command,
        )?)?;
        return Ok(());
    }
    if let Command::Lab { command } = command {
        print_json(&crate::interfaces::cli::lab::run(&config, command).await?)?;
        return Ok(());
    }
    let store = Store::open(&config.state_dir)?;
    let service = AssessmentService::new(config.clone(), store.clone());
    match command {
        Command::Cleanup { .. }
        | Command::Init { .. }
        | Command::Inspect { .. }
        | Command::Env { .. }
        | Command::Challenge { .. }
        | Command::Lab { .. } => unreachable!(),
        Command::Doctor => {
            let registry = crate::adapters::environments::EnvironmentRegistry::new(
                config.environments.clone(),
            )?;
            let mut environments = serde_json::Map::new();
            for id in config.environments.keys() {
                environments.insert(
                    id.clone(),
                    match registry.doctor(id).await {
                        Ok(value) => value,
                        Err(error) => json!({"available":false,"error":format!("{error:#}")}),
                    },
                );
            }
            registry.close().await;
            let key_env =
                config
                    .model
                    .api_key_env
                    .as_deref()
                    .unwrap_or(match config.model.provider {
                        crate::adapters::models::Provider::Anthropic => "ANTHROPIC_API_KEY",
                        crate::adapters::models::Provider::Ollama => "OLLAMA_API_KEY",
                        _ => "OPENAI_API_KEY",
                    });
            print_json(
                &json!({"version":env!("CARGO_PKG_VERSION"),"rig_version":crate::adapters::models::RIG_VERSION,"state_dir":store.root(),"model":{"provider":config.model.provider,"model":config.model.model,"role_model":config.model.role_model,"role_models":config.model.role_models,"base_url":config.model.base_url,"credential_env":key_env,"credential_present":crate::adapters::credentials::lookup(key_env).is_some()},"static":{"binary_parser":true,"ghidra_executable":config.analysis.ghidra_executable,"static_environment":config.analysis.static_environment,"configured":config.analysis.ghidra_executable.is_some()},"environments":environments,"mcp_servers":config.mcp.iter().map(|s|&s.name).collect::<Vec<_>>()}),
            )?;
        }
        Command::Analyze {
            target,
            task,
            output,
        } => {
            let prepared = service.start(&target, &task).await?;
            let run = prepared.run().clone();
            println!(
                "{}",
                json!({"run_id":run.id,"root_agent_id":run.root_id,"state_dir":store.root()})
            );
            let result = drive_with_events(prepared, cli.json).await?;
            if let Some(path) = output {
                print_json(&serde_json::to_value(report::export_with_locale(
                    &store, &run.id, &path, locale,
                )?)?)?;
            }
            print_json(&serde_json::to_value(&result)?)?;
            require_completed(result.status)?;
        }
        Command::Resume {
            run_id,
            max_tokens,
            max_usd,
            output,
        } => {
            let run: RunRecord = store.require("run", &run_id)?;
            let mut budget = run.budget.clone();
            if let Some(tokens) = max_tokens {
                budget.max_total_tokens = tokens;
            }
            if let Some(usd) = max_usd {
                budget.max_usd = Some(usd);
            }
            let prepared = service.resume(&run_id, Some(budget)).await?;
            let result = drive_with_events(prepared, cli.json).await?;
            if let Some(path) = output {
                print_json(&serde_json::to_value(report::export_with_locale(
                    &store, &run_id, &path, locale,
                )?)?)?;
            }
            print_json(&serde_json::to_value(&result)?)?;
            require_completed(result.status)?;
        }
        Command::Runs => print_json(&serde_json::to_value(
            crate::queries::AssessmentQueries::new(&store).runs()?,
        )?)?,
        Command::Status { run_id, events } => print_json(&serde_json::to_value(
            crate::queries::AssessmentQueries::new(&store).status(&run_id, events)?,
        )?)?,
        Command::Reconcile {
            call_id,
            result_file,
            evidence_ids,
            error,
        } => {
            let result: Value = serde_json::from_slice(&std::fs::read(result_file)?)?;
            let outcome = crate::application::tools::reconcile_record(
                &store,
                &call_id,
                result,
                &evidence_ids,
                error,
            )?;
            print_json(&serde_json::to_value(outcome)?)?;
        }
        Command::Gui { run_id } => {
            #[cfg(feature = "desktop")]
            crate::interfaces::desktop::host::run(config, store, run_id, path, cli.state)?;
            #[cfg(not(feature = "desktop"))]
            {
                let _ = (config, store, run_id, path);
                anyhow::bail!("此构建仅包含 CLI；桌面版本请使用 npm run desktop:build 构建。");
            }
        }
    }
    Ok(())
}
fn print_json(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
async fn drive_with_events(prepared: PreparedAssessment, json_events: bool) -> Result<RunRecord> {
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    let signal_task = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.cancel();
        }
    });
    let mut events = prepared.subscribe();
    let printer = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    if event["kind"] == "model.stream_progress" {
                        continue;
                    }
                    if json_events {
                        eprintln!("{event}");
                    } else {
                        eprintln!(
                            "{} {}",
                            event["kind"].as_str().unwrap_or("event"),
                            event["agent_id"].as_str().unwrap_or("assessment")
                        );
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    let result = prepared.drive(cancel).await;
    signal_task.abort();
    printer.abort();
    result
}

/// Default objective shared by CLI and desktop assessments.
pub(crate) const DEFAULT_TASK: &str = "Analyze the binary's purpose, structure, inputs, outputs and key behavior. Recover relevant formats or algorithms, validate observations where feasible, and report evidence, uncertainty and limitations. Investigate vulnerabilities only when explicitly requested.";

fn require_completed(status: Status) -> Result<()> {
    anyhow::ensure!(
        status == Status::Completed,
        "assessment did not complete (status={status}); inspect stored state and partial evidence"
    );
    Ok(())
}

fn config_path(
    explicit: Option<PathBuf>,
    desktop: bool,
    cwd: &std::path::Path,
    executable: Option<&std::path::Path>,
) -> Option<PathBuf> {
    if explicit.is_some() {
        return explicit;
    }
    let local = cwd.join("wukong.toml");
    let portable = executable
        .and_then(|exe| exe.parent())
        .map(|dir| dir.join("wukong.toml"));
    let candidates = if desktop {
        [portable, Some(local)]
    } else {
        [Some(local), portable]
    };
    candidates.into_iter().flatten().find(|path| path.is_file())
}

#[cfg(test)]
mod config_path_tests {
    use super::*;

    #[test]
    fn only_completed_assessments_succeed() {
        assert!(require_completed(Status::Completed).is_ok());
        for status in [
            Status::Queued,
            Status::Running,
            Status::Waiting,
            Status::BudgetPaused,
            Status::Stopped,
            Status::Crashed,
            Status::Failed,
        ] {
            let error = require_completed(status).unwrap_err().to_string();
            assert!(error.contains(status.as_str()));
        }
    }

    #[test]
    fn desktop_prefers_executable_config_and_cli_keeps_local_config() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin)?;
        let local = temp.path().join("wukong.toml");
        let portable = bin.join("wukong.toml");
        let exe = bin.join("wukong.exe");
        std::fs::write(&local, "")?;
        std::fs::write(&portable, "")?;
        assert_eq!(
            config_path(None, true, temp.path(), Some(&exe)),
            Some(portable)
        );
        assert_eq!(
            config_path(None, false, temp.path(), Some(&exe)),
            Some(local)
        );
        let explicit = temp.path().join("custom.toml");
        assert_eq!(
            config_path(Some(explicit.clone()), true, temp.path(), Some(&exe)),
            Some(explicit)
        );
        Ok(())
    }
}
