use crate::{
    binary,
    config::AppConfig,
    coordinator::Coordinator,
    domain::*,
    report,
    runner::AssessmentRunner,
    runtime::{RuntimeBackend, RuntimeClient},
    store::Store,
};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::{io, path::PathBuf, time::Duration};
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
    pub command: Command,
}
#[derive(Subcommand)]
pub enum Command {
    /// Write a configuration without overwriting an existing file.
    Init {
        #[arg(default_value = "re-agent.toml")]
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
    /// Start a real model-driven multi-agent assessment.
    Analyze {
        target: PathBuf,
        #[arg(
            long,
            default_value = "Analyze the binary, establish its input/trust boundaries, investigate concrete vulnerability hypotheses, independently verify reproducible findings and report evidence and limitations."
        )]
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
    /// Live local dashboard; q exits, p pauses the selected assessment.
    Tui {
        run_id: String,
    },
}

pub async fn run(cli: Cli) -> Result<()> {
    if let Command::Init { path } = &cli.command {
        AppConfig::write_example(path)?;
        println!("{}", json!({"config":path}));
        return Ok(());
    }
    let auto = PathBuf::from("re-agent.toml");
    let path = cli
        .config
        .as_deref()
        .or_else(|| auto.exists().then_some(auto.as_path()));
    let mut config = AppConfig::load(path)?;
    if let Some(state) = cli.state {
        config.state_dir = state;
    }
    if let Command::Inspect { target, strings } = &cli.command {
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
    let coordinator = Coordinator::new(store.clone());
    match cli.command {
        Command::Init { .. } | Command::Inspect { .. } => unreachable!(),
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
                &json!({"version":env!("CARGO_PKG_VERSION"),"rig_version":crate::llm::RIG_STATE_VERSION,"state_dir":store.root(),"model":{"provider":config.model.provider,"model":config.model.model,"role_model":config.model.role_model,"role_models":config.model.role_models,"base_url":config.model.base_url,"credential_env":key_env,"credential_present":std::env::var_os(key_env).is_some()},"static":{"binary_parser":true,"ghidra_executable":config.analysis.ghidra_executable,"configured":config.analysis.ghidra_executable.is_some()},"runtime":runtime,"mcp_servers":config.mcp.iter().map(|s|&s.name).collect::<Vec<_>>()}),
            )?;
        }
        Command::Analyze {
            target,
            task,
            output,
        } => {
            // Validate provider and worker before creating a durable assessment.
            let runner = AssessmentRunner::new(config.clone(), store.clone()).await?;
            let run = coordinator.create_run(
                &target,
                &task,
                config.budget.clone(),
                serde_json::to_value(&config)?,
            )?;
            println!(
                "{}",
                json!({"run_id":run.id,"root_agent_id":run.root_id,"state_dir":store.root()})
            );
            let result = drive_with_events(&runner, &run.id, cli.json).await?;
            if let Some(path) = output {
                print_json(&serde_json::to_value(report::export(
                    &store, &run.id, &path,
                )?)?)?;
            }
            print_json(&serde_json::to_value(&result)?)?;
            runner.tools.close().await;
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
            let mut saved: AppConfig = serde_json::from_value(run.config.clone())
                .context("saved run configuration invalid")?;
            // --state selects the durable store; all model/backend identities come from the checkpoint.
            saved.state_dir = config.state_dir.clone();
            let mut budget = run.budget.clone();
            if let Some(tokens) = max_tokens {
                budget.max_total_tokens = tokens;
            }
            if let Some(usd) = max_usd {
                budget.max_usd = Some(usd);
            }
            saved.budget = budget.clone();
            saved.validate()?;
            let runner = AssessmentRunner::new(saved, store.clone()).await?;
            // Do not mutate a live controller's state while it owns this assessment.
            let owner = id("resume");
            if !store.acquire_lease(&run_id, &owner, 1)? {
                anyhow::bail!("assessment has a live controller");
            }
            coordinator.resume(&run_id, Some(budget))?;
            store.release_lease(&run_id, &owner)?;
            let result = drive_with_events(&runner, &run_id, cli.json).await?;
            if let Some(path) = output {
                print_json(&serde_json::to_value(report::export(
                    &store, &run_id, &path,
                )?)?)?;
            }
            print_json(&serde_json::to_value(&result)?)?;
            runner.tools.close().await;
            if result.status == Status::Failed {
                anyhow::bail!("assessment failed; inspect stored errors and partial evidence");
            }
        }
        Command::Runs => print_json(&serde_json::to_value(store.runs()?)?)?,
        Command::Status { run_id, events } => {
            let agents = store.list::<AgentRecord>("agent", &run_id)?;
            let all_events = store.recent_events(&run_id, events.min(1000))?;
            print_json(
                &json!({"run":store.require::<RunRecord>("run",&run_id)?,"agents":agents.iter().map(|a|json!({"id":a.id,"name":a.name,"role":a.role,"parent_id":a.parent_id,"status":a.status,"scope":a.scope,"turns":a.turns,"tokens":a.total_tokens,"error":a.error,"summary":a.summary})).collect::<Vec<_>>(),"events":all_events.iter().rev().take(events.min(1000)).collect::<Vec<_>>()}),
            )?;
        }
        Command::Message { agent_id, text } => {
            coordinator.message("operator", &agent_id, &text)?;
            println!("{}", json!({"delivered":true,"agent_id":agent_id}));
        }
        Command::Pause { run_id } => {
            coordinator.pause_budget(&run_id, "operator paused assessment")?;
            let run: RunRecord = store.require("run", &run_id)?;
            println!("{}", json!({"run_id":run_id,"status":run.status}));
        }
        Command::Stop { agent_id, reason } => {
            print_json(&json!({"stopped_agents":coordinator.stop(&agent_id,&reason)?}))?
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
        Command::Tui { run_id } => dashboard(&store, &run_id)?,
    }
    Ok(())
}
fn print_json(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
async fn drive_with_events(
    runner: &AssessmentRunner,
    run_id: &str,
    json_events: bool,
) -> Result<RunRecord> {
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    let signal_task = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.cancel();
        }
    });
    let mut events = runner.subscribe();
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
    let result = runner.drive(run_id, cancel).await;
    signal_task.abort();
    printer.abort();
    result
}

fn dashboard(store: &Store, run_id: &str) -> Result<()> {
    use crossterm::{
        event::{self, Event as CEvent, KeyCode},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    };
    use ratatui::{
        Terminal,
        backend::CrosstermBackend,
        layout::{Constraint, Direction, Layout},
        widgets::{Block, Borders, Paragraph, Row, Table, Wrap},
    };
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
    store.require::<RunRecord>("run", run_id)?;
    enable_raw_mode()?;
    let _restore = Restore;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    loop {
        let run: RunRecord = store.require("run", run_id)?;
        let agents = store.list::<AgentRecord>("agent", run_id)?;
        let events = store.recent_events(run_id, 12)?;
        terminal.draw(|frame|{
            let areas=Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(4),Constraint::Percentage(55),Constraint::Min(5)]).split(frame.area());
            frame.render_widget(Paragraph::new(format!("{}  status={}  tokens={}/{}  cost~${:.4}\nTarget {}\nq: exit   p: pause   Use CLI message/resume to steer work.",run.id,run.status,run.total_tokens,run.budget.max_total_tokens,run.cost_usd,run.target_path.display())).block(Block::default().borders(Borders::ALL).title("re-agent")),areas[0]);
            let rows=agents.iter().map(|a|Row::new(vec![format!("{}{}","  ".repeat(a.depth as usize),a.name),a.id.clone(),a.role.to_string(),a.status.to_string(),a.turns.to_string(),a.scope.clone()]));
            frame.render_widget(Table::new(rows,[Constraint::Percentage(18),Constraint::Percentage(28),Constraint::Length(10),Constraint::Length(14),Constraint::Length(6),Constraint::Percentage(25)]).header(Row::new(["Agent","ID","Role","Status","Turns","Scope"])).block(Block::default().borders(Borders::ALL).title("Agent graph")),areas[1]);
            let text=events.iter().rev().take(12).map(|e|format!("{} {} {}",e.seq,e.kind,e.agent_id.as_deref().unwrap_or("assessment"))).collect::<Vec<_>>().join("\n");
            frame.render_widget(Paragraph::new(text).wrap(Wrap{trim:false}).block(Block::default().borders(Borders::ALL).title("Latest events")),areas[2]);
        })?;
        if event::poll(Duration::from_millis(250))?
            && let CEvent::Key(key) = event::read()?
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('p') if !run.status.terminal() => {
                    Coordinator::new(store.clone())
                        .pause_budget(run_id, "operator paused in TUI")?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
