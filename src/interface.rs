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
use anyhow::{Context, Result, ensure};
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
    /// Strix-style live dashboard: agent tree, activity stream, findings and
    /// budget status. Without a run id it opens an interactive terminal: type
    /// a target path to start a new assessment, or watch the latest run.
    /// q exits, p pauses, i starts a task, m messages the root, up/down
    /// scroll the activity, f follows.
    Tui {
        /// Optional run to watch; omitted selects the latest run or the
        /// interactive new-task prompt.
        run_id: Option<String>,
    },
}

pub async fn run(cli: Cli) -> Result<()> {
    if let Command::Init { path } = &cli.command {
        AppConfig::write_example(path)?;
        println!("{}", json!({"config":path}));
        return Ok(());
    }
    // Config lookup: explicit --config, then ./re-agent.toml in the working
    // directory, then re-agent.toml next to the executable (so a portable
    // copy works from any directory), then built-in defaults.
    let auto = PathBuf::from("re-agent.toml");
    let path: Option<PathBuf> = cli
        .config
        .clone()
        .or_else(|| auto.exists().then_some(auto.clone()))
        .or_else(|| {
            std::env::current_exe().ok().and_then(|exe| {
                let candidate = exe.parent()?.join("re-agent.toml");
                candidate.exists().then_some(candidate)
            })
        });
    let mut config = AppConfig::load(path.as_deref())?;
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
        Command::Tui { run_id } => {
            // Watch-only mode stays available without model credentials; the
            // interactive new-task prompt needs a working engine.
            let runner = match AssessmentRunner::new(config.clone(), store.clone()).await {
                Ok(runner) => Some(runner),
                Err(error) => {
                    eprintln!("re-agent: watch-only mode: {error:#}");
                    None
                }
            };
            dashboard(&store, run_id.as_deref(), runner.clone())?;
            if let Some(runner) = runner {
                runner.tools.close().await;
            }
        }
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

/// The default analyze task, shared by the `analyze` command and the TUI's
/// new-task prompt.
const DEFAULT_TASK: &str = "Analyze the binary, establish its input/trust boundaries, investigate concrete vulnerability hypotheses, independently verify reproducible findings and report evidence and limitations.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum TuiInput {
    None,
    Task,
    Message,
}

/// Validate a target path and drive a fresh assessment from the TUI,
/// returning the new run id and its cancellation token.
fn start_task(
    runner: &AssessmentRunner,
    target: &str,
) -> Result<(String, CancellationToken)> {
    let path = std::fs::canonicalize(target.trim())?;
    ensure!(path.is_file(), "not a file: {}", target.trim());
    let config = runner.config();
    let created = runner.coordinator.create_run(
        &path,
        DEFAULT_TASK,
        config.budget.clone(),
        serde_json::to_value(config.clone())?,
    )?;
    let cancel = CancellationToken::new();
    let drive_runner = runner.clone();
    let drive_id = created.id.clone();
    let drive_token = cancel.clone();
    tokio::spawn(async move {
        let _ = drive_runner.drive(&drive_id, drive_token).await;
    });
    Ok((created.id, cancel))
}

fn banner_lines() -> Vec<ratatui::text::Line<'static>> {
    use ratatui::{
        style::{Color, Modifier, Style},
        text::{Line, Span},
    };
    vec![
        Line::from(Span::styled(
            "██████╗ ███████╗    █████╗  ██████╗ ███████╗ ███╗   ██╗ ████████╗",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "██╔══██╗██╔════╝   ██╔══██╗██╔═════╝ ██╔════╝ ████╗  ██║ ╚══██╔══╝",
            Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "██████╔╝█████╗     ███████║██║  ███╗█████╗   ██╔██╗ ██║    ██║",
            Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "██╔══██╗██╔══╝     ██╔══██║██║   ██║██╔══╝   ██║╚██╗██║    ██║",
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "██║  ██║███████╗    ██║  ██║╚██████╔╝███████╗ ██║ ╚████║    ██║",
            Style::default().fg(Color::LightMagenta).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                "╚═╝  ╚═╝╚══════╝    ╚═╝  ╚═╝ ╚═════╝ ╚══════╝ ╚═╝  ╚═══╝    ╚═╝  ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " binary reverse-engineering agents",
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ]
}

fn dashboard(
    store: &Store,
    run_id: Option<&str>,
    runner: Option<AssessmentRunner>,
) -> Result<()> {
    use crossterm::{
        event::{self, Event as CEvent, KeyCode},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    };
    use ratatui::{
        Terminal,
        backend::CrosstermBackend,
        layout::{Constraint, Direction, Layout},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Paragraph, Wrap},
    };
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
    enable_raw_mode()?;
    let _restore = Restore;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut scroll: usize = 0;
    let mut follow = true;
    let mut input = TuiInput::None;
    let mut buffer = String::new();
    let mut input_error: Option<String> = None;
    let mut drive_cancel: Option<CancellationToken> = None;
    let mut selected: Option<String> = run_id.map(str::to_owned);
    loop {
        if selected.is_none() {
            if let Ok(mut runs) = store.runs() {
                runs.reverse();
                selected = runs.into_iter().next().map(|run| run.id);
            }
        }
        let Some(selected_id) = selected.as_deref() else {
            // Welcome state: no assessment exists yet. Typing starts the
            // task input directly, Strix-style.
            terminal.draw(|frame| {
                let area = frame.area();
                let mut lines = banner_lines();
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled(
                        "What do you want me to do? Type a target path and press Enter.",
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(Span::styled(
                    "The run is driven live in this terminal; q pauses and exits.",
                    Style::default().fg(Color::DarkGray),
                )));
                lines.push(Line::from(Span::styled(
                    if runner.is_some() {
                        "Ready to start tasks."
                    } else {
                        "Watch-only: model credentials are missing, so new tasks cannot start."
                    },
                    Style::default().fg(if runner.is_some() { Color::Green } else { Color::Red }),
                )));
                lines.push(Line::from(""));
                let mut prompt = vec![
                    Span::styled(" ▸ ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::styled("task: target path ", Style::default().fg(Color::Cyan)),
                    Span::raw(&buffer),
                    Span::styled("▌", Style::default().fg(Color::Yellow)),
                ];
                if let Some(error) = &input_error {
                    prompt.push(Span::styled(format!("  {error}"), Style::default().fg(Color::Red)));
                }
                lines.push(Line::from(prompt));
                frame.render_widget(
                    Paragraph::new(lines).block(
                        Block::default().borders(Borders::ALL).title(" RE-AGENT "),
                    ),
                    area,
                );
            })?;
            if event::poll(Duration::from_millis(250))?
                && let CEvent::Key(key) = event::read()?
            {
                // Windows terminals report press and release as separate
                // events; handle only presses so a keystroke never lands
                // twice in the input buffer.
                if key.kind != crossterm::event::KeyEventKind::Press {
                    continue;
                }
                if input == TuiInput::Task {
                    match key.code {
                        KeyCode::Esc => {
                            input = TuiInput::None;
                            buffer.clear();
                            input_error = None;
                        }
                        KeyCode::Enter => {
                            if let Some(runner) = &runner {
                                match start_task(runner, &buffer) {
                                    Ok((run_id, cancel)) => {
                                        drive_cancel = Some(cancel);
                                        selected = Some(run_id);
                                        follow = true;
                                        scroll = 0;
                                        input = TuiInput::None;
                                        buffer.clear();
                                        input_error = None;
                                    }
                                    Err(error) => input_error = Some(format!("{error:#}")),
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            buffer.pop();
                        }
                        KeyCode::Char(c) => buffer.push(c),
                        _ => {}
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('i') if runner.is_some() => input = TuiInput::Task,
                    KeyCode::Char(c) if runner.is_some() && !c.is_control() => {
                        input = TuiInput::Task;
                        buffer.push(c);
                    }
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    _ => {}
                }
            }
            continue;
        };
        let run: RunRecord = store.require("run", selected_id)?;
        let agents = store.list::<AgentRecord>("agent", selected_id)?;
        let findings = store.list::<Finding>("finding", selected_id)?;
        let events = store.recent_events(selected_id, 600)?;
        let tree_height = terminal.size()?.height.saturating_sub(14).max(4) as usize;
        let event_height = tree_height;
        if follow {
            scroll = events.len().saturating_sub(event_height.saturating_sub(2));
        }
        terminal.draw(|frame| {
            let areas = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(7),
                    Constraint::Min(10),
                    Constraint::Length(6),
                    Constraint::Length(if input != TuiInput::None { 1 } else { 0 }),
                    Constraint::Length(1),
                ])
                .split(frame.area());
            // ---- banner ----
            frame.render_widget(
                Paragraph::new(banner_lines()).block(Block::default().borders(Borders::NONE)),
                areas[0],
            );
            // ---- middle: agent tree | activity ----
            let middle = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
                .split(areas[1]);
            let tree_lines: Vec<Line> = agents
                .iter()
                .map(|agent| {
                    let (glyph, status_color) = match agent.status {
                        Status::Running => ("◉", Color::Yellow),
                        Status::Queued => ("○", Color::DarkGray),
                        Status::Waiting => ("◌", Color::LightYellow),
                        Status::BudgetPaused => ("⏸", Color::Yellow),
                        Status::Completed => ("✔", Color::Green),
                        Status::Stopped => ("◼", Color::DarkGray),
                        Status::Failed => ("✖", Color::Red),
                        Status::Crashed => ("✖", Color::LightRed),
                    };
                    let role_color = match agent.role {
                        Role::Root => Color::Cyan,
                        Role::Recon => Color::Blue,
                        Role::Reverse => Color::Magenta,
                        Role::Hunter => Color::Yellow,
                        Role::Fuzz => Color::Green,
                        Role::Crash => Color::Red,
                        Role::Exploit => Color::LightRed,
                        Role::Verifier => Color::LightBlue,
                        Role::Reporter => Color::LightGreen,
                    };
                    Line::from(vec![
                        Span::raw("  ".repeat(agent.depth as usize)),
                        Span::styled(glyph, Style::default().fg(status_color)),
                        Span::raw(if agent.depth > 0 { "▸ " } else { " " }),
                        Span::styled(&agent.name, Style::default().fg(role_color).add_modifier(Modifier::BOLD)),
                        Span::raw("  "),
                        Span::styled(
                            format!("{} t={} {}", agent.role, agent.turns, agent.status),
                            Style::default().fg(status_color),
                        ),
                    ])
                })
                .collect();
            frame.render_widget(
                Paragraph::new(tree_lines)
                    .block(Block::default().borders(Borders::ALL).title(" AGENT TREE "))
                    .wrap(Wrap { trim: false }),
                middle[0],
            );
            let visible = events.iter().rev().skip(scroll).take(event_height).collect::<Vec<_>>();
            let event_lines: Vec<Line> = visible
                .iter()
                .map(|event| {
                    let role = store_agent_role(&agents, event.agent_id.as_deref());
                    let color = match role {
                        Some(Role::Root) => Color::Cyan,
                        Some(Role::Recon) => Color::Blue,
                        Some(Role::Reverse) => Color::Magenta,
                        Some(Role::Hunter) => Color::Yellow,
                        Some(Role::Fuzz) => Color::Green,
                        Some(Role::Crash) => Color::Red,
                        Some(Role::Exploit) => Color::LightRed,
                        Some(Role::Verifier) => Color::LightBlue,
                        Some(Role::Reporter) => Color::LightGreen,
                        None => Color::DarkGray,
                    };
                    let stamp = event
                        .created_at
                        .with_timezone(&chrono::Local)
                        .format("%H:%M:%S")
                        .to_string();
                    Line::from(vec![
                        Span::styled(format!("{stamp} "), Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("[{}] ", event.agent_id.as_deref().map(short_id).unwrap_or("assessment")),
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(event_kind(&event.kind), Style::default().fg(color)),
                        Span::raw("  "),
                        Span::raw(event_brief(&event)),
                    ])
                })
                .collect();
            frame.render_widget(
                Paragraph::new(event_lines)
                    .block(Block::default().borders(Borders::ALL).title(format!(" ACTIVITY {} ", if follow { "· following" } else { "· scrolled" })))
                    .wrap(Wrap { trim: false }),
                middle[1],
            );
            // ---- findings ----
            let finding_lines: Vec<Line> = if findings.is_empty() {
                vec![Line::from(Span::styled(
                    "no findings recorded",
                    Style::default().fg(Color::DarkGray),
                ))]
            } else {
                findings
                    .iter()
                    .map(|finding| {
                        let severity_color = match finding.severity {
                            Some(Severity::Critical) => Color::LightRed,
                            Some(Severity::High) => Color::Red,
                            Some(Severity::Medium) => Color::Yellow,
                            Some(Severity::Low) => Color::LightBlue,
                            Some(Severity::Info) | None => Color::Gray,
                        };
                        Line::from(vec![
                            Span::styled(
                                format!("{} ", finding.severity.map(|s| format!("{s:?}")).unwrap_or_else(|| "??".into()).to_uppercase()),
                                Style::default().fg(severity_color).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(format!("{:?} ", finding.stage), Style::default().fg(Color::DarkGray)),
                            Span::raw(&finding.title),
                        ])
                    })
                    .collect()
            };
            frame.render_widget(
                Paragraph::new(finding_lines)
                    .block(Block::default().borders(Borders::ALL).title(" FINDINGS "))
                    .wrap(Wrap { trim: true }),
                areas[2],
            );
            // ---- input line ----
            if input != TuiInput::None {
                let label = match input {
                    TuiInput::Task => " task: target path ",
                    TuiInput::Message => " message to root ",
                    TuiInput::None => "",
                };
                let mut spans = vec![
                    Span::styled(" ▸ ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::styled(label, Style::default().fg(Color::Cyan)),
                    Span::raw(&buffer),
                    Span::styled("▌", Style::default().fg(Color::Yellow)),
                ];
                if let Some(error) = &input_error {
                    spans.push(Span::styled(
                        format!("  {error}"),
                        Style::default().fg(Color::Red),
                    ));
                }
                frame.render_widget(
                    Paragraph::new(Line::from(spans)).block(
                        Block::default()
                            .borders(Borders::TOP)
                            .border_style(Style::default().fg(Color::Cyan)),
                    ),
                    areas[3],
                );
            }
            // ---- status bar ----
            let status_color = match run.status {
                Status::Running => Color::Green,
                Status::BudgetPaused | Status::Waiting => Color::Yellow,
                Status::Completed => Color::LightGreen,
                Status::Failed | Status::Crashed => Color::Red,
                _ => Color::Gray,
            };
            let elapsed = chrono::Utc::now().signed_duration_since(run.created_at);
            let elapsed = format!(
                "{:02}:{:02}:{:02}",
                elapsed.num_hours(),
                elapsed.num_minutes() % 60,
                elapsed.num_seconds() % 60
            );
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" re-agent ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" {} ", run.status), Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("tokens {}/{}  ", run.total_tokens, run.budget.max_total_tokens), Style::default().fg(Color::Gray)),
                    Span::styled(format!("cost ${:.4}  ", run.cost_usd), Style::default().fg(Color::Gray)),
                    Span::styled(format!("elapsed {elapsed}  "), Style::default().fg(Color::Gray)),
                    Span::styled(format!("agents {}  ", agents.len()), Style::default().fg(Color::Gray)),
                    Span::styled("q quit · p pause · i task · m message · ↑/↓ scroll · f follow", Style::default().fg(Color::DarkGray)),
                ])),
                areas[4],
            );
        })?;
        if event::poll(Duration::from_millis(250))? && let CEvent::Key(key) = event::read()? {
            // Press-only, as above: release events must not double the input.
            if key.kind != crossterm::event::KeyEventKind::Press {
                continue;
            }
            if input != TuiInput::None {
                match key.code {
                    KeyCode::Esc => {
                        input = TuiInput::None;
                        buffer.clear();
                        input_error = None;
                    }
                    KeyCode::Enter => match input {
                        TuiInput::Task => {
                            if let Some(runner) = &runner {
                                match start_task(runner, &buffer) {
                                    Ok((created, cancel)) => {
                                        drive_cancel = Some(cancel);
                                        selected = Some(created);
                                        follow = true;
                                        scroll = 0;
                                        input = TuiInput::None;
                                        buffer.clear();
                                        input_error = None;
                                    }
                                    Err(error) => input_error = Some(format!("{error:#}")),
                                }
                            }
                        }
                        TuiInput::Message => {
                            if !run.status.terminal() && !buffer.trim().is_empty() {
                                Coordinator::new(store.clone())
                                    .message("operator", &run.root_id, buffer.trim())?;
                            }
                            input = TuiInput::None;
                            buffer.clear();
                            input_error = None;
                        }
                        TuiInput::None => {}
                    },
                    KeyCode::Backspace => {
                        buffer.pop();
                    }
                    KeyCode::Char(c) => buffer.push(c),
                    _ => {}
                }
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    if let Some(cancel) = &drive_cancel {
                        let _ = Coordinator::new(store.clone())
                            .pause_budget(selected_id, "operator quit TUI");
                        cancel.cancel();
                    }
                    break;
                }
                KeyCode::Char('p') if !run.status.terminal() => {
                    Coordinator::new(store.clone())
                        .pause_budget(selected_id, "operator paused in TUI")?;
                }
                KeyCode::Char('i') if runner.is_some() => {
                    input = TuiInput::Task;
                    buffer.clear();
                    input_error = None;
                }
                KeyCode::Char('m') if !run.status.terminal() => {
                    input = TuiInput::Message;
                    buffer.clear();
                    input_error = None;
                }
                KeyCode::Char('f') => follow = true,
                KeyCode::Up => {
                    follow = false;
                    scroll = scroll.saturating_add(1);
                }
                KeyCode::Down => {
                    if scroll > 0 {
                        scroll -= 1;
                        if scroll == 0 {
                            follow = true;
                        }
                    }
                }
                KeyCode::PageUp => {
                    follow = false;
                    scroll = scroll.saturating_add(10);
                }
                KeyCode::PageDown => {
                    scroll = scroll.saturating_sub(10);
                    if scroll == 0 {
                        follow = true;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn store_agent_role(agents: &[AgentRecord], agent_id: Option<&str>) -> Option<Role> {
    agent_id.and_then(|id| agents.iter().find(|agent| agent.id == id).map(|agent| agent.role))
}
fn short_id(id: &str) -> &str {
    id.strip_prefix("agent_").and_then(|hex| hex.get(..6)).unwrap_or(id)
}
fn event_kind(kind: &str) -> &str {
    match kind {
        "model.started" => "✶ model",
        "model.finished" => "✶ model done",
        "tool.started" => "⚙ run",
        "tool.finished" => "⚙ done",
        "tool.result" => "⚙ result",
        "agent.created" => "✦ spawn",
        "agent.status" => "◆ status",
        "agent.finished" => "◆ finished",
        "agent.budget_cap" => "◆ quota",
        "agent.lifecycle_nudge" => "◆ nudge",
        "agent.lifecycle_fallback" => "◆ fallback",
        "finding.updated" => "◆ finding",
        "verification.recorded" => "◆ verify",
        "controller.started" => "▶ start",
        "controller.finished" => "■ finish",
        "controller.stopping" => "■ stopping",
        "context.compacted" => "≡ compact",
        _ => kind,
    }
}
fn event_brief(event: &Event) -> String {
    let data = &event.data;
    let brief = match event.kind.as_str() {
        "model.finished" => data["usage"]
            .get("total_tokens")
            .and_then(Value::as_u64)
            .map(|tokens| format!("{tokens} tok"))
            .unwrap_or_default(),
        "tool.started" | "tool.finished" | "tool.result" => data["name"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        "agent.created" => format!(
            "{} {}",
            data["role"].as_str().unwrap_or_default(),
            data["scope"].as_str().unwrap_or_default()
        ),
        "agent.status" => data["status"].as_str().unwrap_or_default().to_string(),
        "agent.budget_cap" => format!(
            "{}>=cap {}",
            data["tokens"].as_u64().unwrap_or_default(),
            data["cap"].as_u64().unwrap_or_default()
        ),
        "finding.updated" => data["title"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_default(),
        "verification.recorded" => format!(
            "passed={} {}",
            data["passed"].as_bool().unwrap_or_default(),
            data["verification_id"].as_str().unwrap_or_default()
        ),
        "controller.finished" | "controller.stopping" | "controller.started" => data["status"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_default(),
        _ => {
            let text = serde_json::to_string(data).unwrap_or_default();
            let text: String = text.chars().take(110).collect();
            text
        }
    };
    brief
}
