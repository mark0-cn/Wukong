use crate::{
    application::{AssessmentService, PreparedAssessment},
    binary,
    config::AppConfig,
    domain::*,
    report,
    runtime::{RuntimeBackend, RuntimeClient},
    store::Store,
};
use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(
    name = "re-agent",
    version,
    about = "Strix-inspired Rust/Rig binary analysis agents"
)]
pub struct Cli {
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
    /// Write a configuration without overwriting an existing file.
    Init {
        #[arg(default_value = "re-agent.toml")]
        path: PathBuf,
    },
    /// Print the built-in tool catalog without opening state or connecting to a model.
    ToolCatalog,
    /// Inspect model settings and installed analysis capabilities without calling a model.
    Doctor,
    /// Parse a binary without running it or calling a model.
    Inspect {
        target: PathBuf,
        #[arg(long)]
        strings: bool,
    },
    /// Start a real model-driven multi-agent assessment.
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
    Message {
        agent_id: String,
        text: String,
        #[arg(long, default_value="information", value_parser=["information","question","reply","follow_up","result","blocked"])]
        kind: String,
        #[arg(long)]
        reply_to: Option<i64>,
        #[arg(long)]
        generation: Option<u64>,
        #[arg(long)]
        correlation_id: Option<String>,
    },
    Pause {
        run_id: String,
    },
    Stop {
        agent_id: String,
        #[arg(long, default_value = "operator stopped task")]
        reason: String,
    },
    Report {
        run_id: String,
        #[arg(long)]
        output: PathBuf,
    },
    VerifyBundle {
        path: PathBuf,
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
    let command = cli.command.unwrap_or(Command::Gui { run_id: None });
    if let Command::Init { path } = &command {
        AppConfig::write_example(path)?;
        println!("{}", json!({"config":path}));
        return Ok(());
    }
    if matches!(command, Command::ToolCatalog) {
        if cli.json {
            print_json(&serde_json::to_value(
                crate::tools::builtin_tool_definitions(),
            )?)?;
        } else {
            print!("{}", crate::tools::builtin_catalog_markdown());
        }
        return Ok(());
    }
    // Desktop settings travel with the executable. CLI keeps its working-directory
    // convention, and an explicit --config always takes precedence.
    let path = config_path(
        cli.config.clone(),
        matches!(command, Command::Gui { .. }),
        &std::env::current_dir()?,
        std::env::current_exe().ok().as_deref(),
    );
    let mut config = AppConfig::load(path.as_deref())?;
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
    let store = Store::open(&config.state_dir)?;
    let service = AssessmentService::new(config.clone(), store.clone());
    match command {
        Command::Init { .. } | Command::Inspect { .. } | Command::ToolCatalog => unreachable!(),
        Command::Doctor => {
            let runtime = if config.runtime.backend == RuntimeBackend::Disabled {
                json!({"available":false,"reason":"execution worker disabled; configure Docker/SSH or explicitly select trusted_local fixtures"})
            } else {
                match RuntimeClient::connect(config.runtime.clone()).await {
                    Ok(client) => {
                        let result = client.doctor().await;
                        let _ = client.close().await;
                        match result {
                            Ok(v) => v,
                            Err(e) => json!({"available":false,"error":format!("{e:#}")}),
                        }
                    }
                    Err(e) => json!({"available":false,"error":format!("{e:#}")}),
                }
            };
            let key_env =
                config
                    .model
                    .api_key_env
                    .as_deref()
                    .unwrap_or(match config.model.provider {
                        crate::llm::Provider::Anthropic => "ANTHROPIC_API_KEY",
                        crate::llm::Provider::Ollama => "OLLAMA_API_KEY",
                        _ => "OPENAI_API_KEY",
                    });
            print_json(
                &json!({"version":env!("CARGO_PKG_VERSION"),"rig_version":crate::llm::RIG_VERSION,"state_dir":store.root(),"model":{"provider":config.model.provider,"model":config.model.model,"role_model":config.model.role_model,"role_models":config.model.role_models,"base_url":config.model.base_url,"credential_env":key_env,"credential_present":crate::credentials::lookup(key_env).is_some()},"static":{"binary_parser":true,"ghidra_executable":config.analysis.ghidra_executable,"configured":config.analysis.ghidra_executable.is_some()},"runtime":runtime,"mcp_servers":config.mcp.iter().map(|s|&s.name).collect::<Vec<_>>()}),
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
                print_json(&serde_json::to_value(report::export(
                    &store, &run.id, &path,
                )?)?)?;
            }
            print_json(&serde_json::to_value(&result)?)?;
            if result.status == Status::Failed {
                anyhow::bail!("assessment failed; inspect stored errors and partial evidence");
            }
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
                print_json(&serde_json::to_value(report::export(
                    &store, &run_id, &path,
                )?)?)?;
            }
            print_json(&serde_json::to_value(&result)?)?;
            if result.status == Status::Failed {
                anyhow::bail!("assessment failed; inspect stored errors and partial evidence");
            }
        }
        Command::Runs => print_json(&serde_json::to_value(
            crate::queries::AssessmentQueries::new(&store).runs()?,
        )?)?,
        Command::Status { run_id, events } => print_json(&serde_json::to_value(
            crate::queries::AssessmentQueries::new(&store).status(&run_id, events)?,
        )?)?,
        Command::Message {
            agent_id,
            text,
            kind,
            reply_to,
            generation,
            correlation_id,
        } => {
            let receipt = service.message(
                &agent_id,
                &text,
                MessageOptions {
                    kind: serde_json::from_value(json!(kind))?,
                    reply_to,
                    recipient_generation: generation,
                    correlation_id,
                    ..Default::default()
                },
            )?;
            print_json(&serde_json::to_value(receipt)?)?;
        }
        Command::Pause { run_id } => {
            service.pause(&run_id, "operator paused assessment")?;
            let run: RunRecord = store.require("run", &run_id)?;
            println!("{}", json!({"run_id":run_id,"status":run.status}));
        }
        Command::Stop { agent_id, reason } => {
            print_json(&json!({"stopped_agents":service.stop(&agent_id,&reason)?}))?
        }
        Command::Report { run_id, output } => print_json(&serde_json::to_value(report::export(
            &store, &run_id, &output,
        )?)?)?,
        Command::VerifyBundle { path } => {
            print_json(&serde_json::to_value(report::verify_bundle(&path)?)?)?
        }
        Command::Reconcile {
            call_id,
            result_file,
            evidence_ids,
            error,
        } => {
            let result: Value = serde_json::from_slice(&std::fs::read(result_file)?)?;
            let outcome =
                crate::tools::reconcile_record(&store, &call_id, result, &evidence_ids, error)?;
            print_json(&serde_json::to_value(outcome)?)?;
        }
        Command::Gui { run_id } => crate::gui::run(config, store, run_id, path, cli.state)?,
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
pub(crate) const DEFAULT_TASK: &str = "分析二进制文件的用途、结构、输入输出和关键行为，还原相关格式或算法，在可行时验证观察结果，并报告证据、不确定性与限制。仅在明确要求时调查漏洞。请用中文提供简短的公开进展说明和分析结论，保留必要的技术标识与证据引用。";

fn config_path(
    explicit: Option<PathBuf>,
    desktop: bool,
    cwd: &std::path::Path,
    executable: Option<&std::path::Path>,
) -> Option<PathBuf> {
    if explicit.is_some() {
        return explicit;
    }
    let local = cwd.join("re-agent.toml");
    let portable = executable
        .and_then(|exe| exe.parent())
        .map(|dir| dir.join("re-agent.toml"));
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
    fn desktop_prefers_executable_config_and_cli_keeps_local_config() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin)?;
        let local = temp.path().join("re-agent.toml");
        let portable = bin.join("re-agent.toml");
        let exe = bin.join("re-agent.exe");
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
