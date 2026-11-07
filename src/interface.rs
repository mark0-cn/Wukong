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
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
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
enum Focus {
    Editor,
    Tree,
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

fn spinner(frame: u64) -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    FRAMES[(frame % 10) as usize]
}

fn sweep_span(frame: u64) -> Span<'static> {
    use ratatui::style::{Color, Style};
    let lit = (frame / 4) as usize % 5;
    let mut text = String::new();
    for index in 0..5 {
        text.push(if index == lit { '▪' } else { '·' });
    }
    Span::styled(text, Style::default().fg(Color::LightGreen))
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let width = area.width.saturating_mul(percent_x) / 100;
    let height = area.height.saturating_mul(percent_y) / 100;
    Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    )
}

fn clear_editor(editor: &mut ratatui_textarea::TextArea<'_>) {
    editor.clear();
}

fn help_lines() -> Vec<Line<'static>> {
    vec![
        Line::from("F1 close help · Ctrl+Q quit (pauses live runs) · Ctrl+P pause · Tab switch input/tree"),
        Line::from(""),
        Line::from("▸ Input (focused by default): type freely, Enter sends the message to the selected agent (or root), Esc clears."),
        Line::from("▸ Tree: ↑/↓ select · Enter opens that agent's conversation · g returns to the live activity stream."),
        Line::from("▸ Mouse: click an agent to view its conversation, click a finding to open its report, wheel scrolls."),
        Line::from("▸ Welcome screen: type a target path and Enter starts a new assessment live in this terminal."),
    ]
}

fn finding_modal_lines(finding: &Finding) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("title: ", Style::default().fg(Color::DarkGray)),
            Span::raw(finding.title.clone()),
        ]),
        Line::from(vec![
            Span::styled("severity: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{:?}", finding.severity)),
            Span::styled("  stage: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{:?}", finding.stage)),
            Span::styled("  agent: ", Style::default().fg(Color::DarkGray)),
            Span::raw(short_id(&finding.agent_id).to_owned()),
        ]),
        Line::from(""),
        Line::from(Span::styled("description", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from(finding.description.clone()),
    ];
    if let Some(root_cause) = &finding.root_cause {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("root cause", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))));
        lines.push(Line::from(root_cause.clone()));
    }
    if let Some(impact) = &finding.impact {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("impact", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))));
        lines.push(Line::from(impact.clone()));
    }
    if let Some(rationale) = &finding.severity_rationale {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("severity rationale", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))));
        lines.push(Line::from(rationale.clone()));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("evidence: ", Style::default().fg(Color::DarkGray)),
        Span::raw(finding.evidence_ids.join(", ")),
    ]));
    if let Some(verification) = &finding.verification_id {
        lines.push(Line::from(vec![
            Span::styled("verification: ", Style::default().fg(Color::DarkGray)),
            Span::raw(verification.clone()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Esc or Enter closes", Style::default().fg(Color::DarkGray))));
    lines
}

/// The selected agent's conversation: task, model text, reasoning, tool
/// calls with their recorded results, and coordinator messages — read from
/// the durable checkpoint and the tool-call ledger.
fn agent_detail_lines(
    store: &Store,
    run_id: &str,
    agent: &AgentRecord,
) -> Vec<Line<'static>> {
    use crate::llm::DurableRun;
    use rig::completion::{AssistantContent, Message};
    use rig::completion::message::UserContent;
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("task: ", Style::default().fg(Color::DarkGray)),
        Span::raw(agent.task.clone()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("scope: ", Style::default().fg(Color::DarkGray)),
        Span::raw(agent.scope.clone()),
    ]));
    lines.push(Line::from(""));
    let Some(checkpoint) = &agent.checkpoint else {
        lines.push(Line::from("no conversation yet"));
        return lines;
    };
    let durable = match DurableRun::from_json(&checkpoint.to_string()) {
        Ok(durable) => durable,
        Err(error) => {
            lines.push(Line::from(format!("checkpoint unreadable: {error}")));
            return lines;
        }
    };
    let calls = store.list::<ToolCallRecord>("call", run_id).unwrap_or_default();
    let mut results: std::collections::HashMap<String, &ToolCallRecord> =
        std::collections::HashMap::new();
    for record in calls.iter().filter(|record| record.agent_id == agent.id) {
        if let Some(call_id) = record.id.rsplit(':').next() {
            results.insert(call_id.to_owned(), record);
        }
    }
    for message in durable.history() {
        match message {
            Message::System { .. } => continue,
            Message::User { content } => {
                for part in content {
                    if let UserContent::Text(text) = part {
                        for line in text.text.lines() {
                            lines.push(Line::from(vec![
                                Span::styled("▸ ", Style::default().fg(Color::Cyan)),
                                Span::raw(line.to_owned()),
                            ]));
                        }
                    }
                }
            }
            Message::Assistant(assistant) => {
                for part in assistant.content {
                    match part {
                        AssistantContent::Text(text) => {
                            for line in text.text.lines() {
                                lines.push(Line::from(vec![
                                    Span::styled("✶ ", Style::default().fg(Color::Green)),
                                    Span::raw(line.to_owned()),
                                ]));
                            }
                        }
                        AssistantContent::Reasoning(reasoning) => {
                            for line in reasoning.text.lines().take(12) {
                                lines.push(Line::from(vec![
                                    Span::styled("… ", Style::default().fg(Color::DarkGray)),
                                    Span::styled(line.to_owned(), Style::default().fg(Color::DarkGray)),
                                ]));
                            }
                        }
                        AssistantContent::ToolCall(call) => {
                            let args = serde_json::to_string(&call.function.arguments)
                                .unwrap_or_default();
                            let args: String = args.chars().take(110).collect();
                            lines.push(Line::from(vec![
                                Span::styled("⚙ ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                                Span::styled(
                                    format!("{} {}", call.function.name, args),
                                    Style::default().fg(Color::Yellow),
                                ),
                            ]));
                            match results.get(&call.id.to_string()) {
                                Some(record) => {
                                    let brief = tool_result_brief(record);
                                    lines.push(Line::from(vec![
                                        Span::styled("   ↳ ", Style::default().fg(Color::DarkGray)),
                                        Span::styled(brief, Style::default().fg(Color::LightBlue)),
                                    ]));
                                }
                                None => {
                                    lines.push(Line::from(vec![Span::styled(
                                        "   ↳ …",
                                        Style::default().fg(Color::DarkGray),
                                    )]));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    lines
}

fn tool_result_brief(record: &ToolCallRecord) -> String {
    let mut brief = String::new();
    if let Some(result) = &record.result {
        if let Some(is_error) = result["is_error"].as_bool() {
            if is_error {
                brief.push_str("error: ");
            }
        }
        if let Some(value) = result["value"]["result"].as_str() {
            let text: String = value.chars().take(160).collect();
            brief.push_str(&text);
        } else if let Some(value) = result["value"]["result"].as_object() {
            let mut parts: Vec<String> = Vec::new();
            for key in ["state", "status", "job_id", "passed", "evidence_id"] {
                if let Some(v) = value[key].as_str() {
                    parts.push(format!("{key}={v}"));
                } else if let Some(v) = value[key].as_bool() {
                    parts.push(format!("{key}={v}"));
                }
            }
            if parts.is_empty() {
                let text = serde_json::to_string(value).unwrap_or_default();
                brief.push_str(&text.chars().take(160).collect::<String>());
            } else {
                brief.push_str(&parts.join(" "));
            }
        } else {
            let text = serde_json::to_string(result).unwrap_or_default();
            brief.push_str(&text.chars().take(160).collect::<String>());
        }
    } else {
        brief.push_str("pending");
    }
    brief
}

fn dashboard(
    store: &Store,
    run_id: Option<&str>,
    runner: Option<AssessmentRunner>,
) -> Result<()> {
    use crossterm::{
        event::{
            self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste,
            EnableMouseCapture, Event as CEvent, KeyCode, KeyEventKind, KeyModifiers,
            MouseButton, MouseEventKind,
        },
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    };
    use ratatui::{
        Terminal,
        backend::CrosstermBackend,
        layout::{Constraint, Direction, Layout, Rect},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph, Wrap},
    };
    use ratatui_textarea::TextArea;
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
    enable_raw_mode()?;
    let _restore = Restore;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut editor = TextArea::default();
    let _ = editor.set_placeholder_text("▸ type a message (Enter sends, Esc clears)");
    let mut scroll: usize = 0;
    let mut detail_scroll: usize = 0;
    let mut follow = true;
    let mut input_error: Option<String> = None;
    let mut drive_cancel: Option<CancellationToken> = None;
    let mut selected: Option<String> = run_id.map(str::to_owned);
    let mut tree_cursor: usize = 0;
    let mut detail_agent: Option<String> = None;
    let mut modal: Option<Finding> = None;
    let mut help_open = false;
    let mut focus = Focus::Editor;
    let mut frame_clock: u64 = 0;
    // Windows terminals can deliver a mouse click as a spurious Esc key
    // event (VT input translation); a bare Esc must never quit, and Esc
    // right after a mouse event is ignored entirely.
    let mut last_mouse_at: Option<std::time::Instant> = None;
    let mut last_tree_rect: Option<Rect> = None;
    let mut last_findings_rect: Option<Rect> = None;
    let mut last_right_rect: Option<Rect> = None;
    loop {
        if selected.is_none() {
            if let Ok(mut runs) = store.runs() {
                runs.reverse();
                selected = runs.into_iter().next().map(|run| run.id);
            }
        }
        let Some(selected_id) = selected.as_deref() else {
            // Welcome state: no assessment exists yet. Typing goes straight
            // into the task input, Strix-style.
            let _ = editor.set_placeholder_text("▸ task: target path (Enter to start)");
            terminal.draw(|frame| {
                let area = frame.area();
                let mut lines = banner_lines();
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled(
                        "What do you want me to do? Type a target path and press Enter.",
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        spinner(frame_clock),
                        Style::default().fg(Color::LightGreen),
                    ),
                ]));
                lines.push(Line::from(Span::styled(
                    "The run is driven live in this terminal; Ctrl+Q pauses and exits.",
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
                if let Some(error) = &input_error {
                    lines.push(Line::from(Span::styled(
                        format!("  {error}"),
                        Style::default().fg(Color::Red),
                    )));
                }
                frame.render_widget(
                    Paragraph::new(lines).block(
                        Block::default().borders(Borders::ALL).title(" RE-AGENT "),
                    ),
                    area,
                );
            })?;
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            let Ok(read) = event::read() else {
                // An unparseable terminal event (e.g. an unsupported mouse
                // protocol sequence) must never take the dashboard down.
                continue;
            };
            if let CEvent::Key(key) = read {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    break;
                }
                if key.code == KeyCode::Esc {
                    let spurious = last_mouse_at
                        .is_some_and(|at| at.elapsed() < Duration::from_millis(250));
                    if !spurious && !editor.is_empty() {
                        editor.clear();
                        input_error = None;
                    }
                    continue;
                }
                if key.code == KeyCode::Enter {
                    if let Some(runner) = &runner {
                        let text = editor.lines().join(" ").trim().to_owned();
                        match start_task(runner, &text) {
                            Ok((run_id, cancel)) => {
                                drive_cancel = Some(cancel);
                                selected = Some(run_id);
                                detail_agent = None;
                                follow = true;
                                scroll = 0;
                                input_error = None;
                                editor.clear();
                            }
                            Err(error) => input_error = Some(format!("{error:#}")),
                        }
                    }
                    continue;
                }
                let input: ratatui_textarea::Input = key.into();
                editor.input(input);
            } else if matches!(read, CEvent::Mouse(_)) {
                last_mouse_at = Some(std::time::Instant::now());
            }
            continue;
        };
        frame_clock = frame_clock.wrapping_add(1);
        let run: RunRecord = store.require("run", selected_id)?;
        let agents = store.list::<AgentRecord>("agent", selected_id)?;
        let findings = store.list::<Finding>("finding", selected_id)?;
        let events = store.recent_events(selected_id, 600)?;
        if tree_cursor >= agents.len() {
            tree_cursor = 0;
        }
        if let Some(agent_id) = detail_agent.as_deref()
            && !agents.iter().any(|agent| agent.id == agent_id)
        {
            detail_agent = None;
        }
        let right_height = terminal.size()?.height.saturating_sub(17).max(4) as usize;
        if follow {
            scroll = events.len().saturating_sub(right_height.saturating_sub(2));
        }
        let _ = editor.set_placeholder_text("▸ message (Enter sends, Esc clears)");
        let detail_lines: Vec<Line> = match detail_agent.as_deref() {
            Some(agent_id) => agents
                .iter()
                .find(|agent| agent.id == agent_id)
                .map(|agent| agent_detail_lines(store, selected_id, agent))
                .unwrap_or_default(),
            None => Vec::new(),
        };
        terminal.draw(|frame| {
            let areas = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(7),
                    Constraint::Min(8),
                    Constraint::Length(6),
                    Constraint::Length(3),
                    Constraint::Length(1),
                ])
                .split(frame.area());
            last_findings_rect = Some(areas[2]);
            // ---- banner ----
            frame.render_widget(
                Paragraph::new(banner_lines()).block(Block::default().borders(Borders::NONE)),
                areas[0],
            );
            // ---- middle: agent tree | activity/detail ----
            let middle = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
                .split(areas[1]);
            last_tree_rect = Some(middle[0]);
            last_right_rect = Some(middle[1]);
            let tree_lines: Vec<Line> = agents
                .iter()
                .enumerate()
                .map(|(index, agent)| {
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
                    let cursor = index == tree_cursor && focus == Focus::Tree;
                    let mut spans = vec![
                        Span::styled(
                            if cursor { "▸" } else { " " },
                            if cursor {
                                Style::default().bg(Color::DarkGray)
                            } else {
                                Style::default()
                            },
                        ),
                        Span::raw("  ".repeat(agent.depth as usize)),
                        Span::styled(glyph, Style::default().fg(status_color)),
                        Span::raw(if agent.depth > 0 { "▸ " } else { " " }),
                        Span::styled(
                            &agent.name,
                            Style::default()
                                .fg(role_color)
                                .add_modifier(Modifier::BOLD)
                                .add_modifier(if cursor { Modifier::UNDERLINED } else { Modifier::empty() }),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            format!("{} t={} {}", agent.role, agent.turns, agent.status),
                            Style::default().fg(status_color),
                        ),
                    ];
                    if agent.status == Status::Running {
                        spans.push(Span::raw("  "));
                        spans.push(sweep_span(frame_clock));
                    }
                    Line::from(spans)
                })
                .collect();
            frame.render_widget(
                Paragraph::new(tree_lines)
                    .block(Block::default().borders(Borders::ALL).title(" AGENT TREE "))
                    .wrap(Wrap { trim: false }),
                middle[0],
            );
            if detail_agent.is_some() {
                let detail_agent_name = detail_agent
                    .as_deref()
                    .and_then(|id| agents.iter().find(|agent| agent.id == id))
                    .map(|agent| agent.name.clone())
                    .unwrap_or_default();
                let start = detail_lines
                    .len()
                    .saturating_sub(right_height.saturating_sub(2))
                    .saturating_sub(detail_scroll);
                let window = detail_lines
                    .iter()
                    .skip(start)
                    .take(right_height.saturating_sub(2));
                frame.render_widget(
                    Paragraph::new(window.cloned().collect::<Vec<_>>())
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(format!(" {detail_agent_name} "))
                                .border_style(Style::default().fg(Color::Cyan)),
                        )
                        .wrap(Wrap { trim: false }),
                    middle[1],
                );
            } else {
                let visible = events
                    .iter()
                    .rev()
                    .skip(scroll)
                    .take(right_height.saturating_sub(2))
                    .collect::<Vec<_>>();
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
                                format!(
                                    "[{}] ",
                                    event
                                        .agent_id
                                        .as_deref()
                                        .map(short_id)
                                        .unwrap_or("assessment")
                                ),
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
                        .block(
                            Block::default().borders(Borders::ALL).title(format!(
                                " ACTIVITY {} ",
                                if follow { "· following" } else { "· scrolled" }
                            )),
                        )
                        .wrap(Wrap { trim: false }),
                    middle[1],
                );
            }
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
                                format!(
                                    "{} ",
                                    finding
                                        .severity
                                        .map(|s| format!("{s:?}"))
                                        .unwrap_or_else(|| "??".into())
                                        .to_uppercase()
                                ),
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
            // ---- editor ----
            frame.render_widget(&editor, areas[3]);
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
                    Span::styled(
                        if focus == Focus::Tree {
                            "tree: ↑/↓ · Enter view · g stream · Tab input"
                        } else {
                            "input: Enter send · Ctrl+Q quit · Ctrl+P pause · F1 help · Tab tree"
                        },
                        Style::default().fg(Color::DarkGray),
                    ),
                ])),
                areas[4],
            );
            // ---- overlays ----
            if let Some(finding) = &modal {
                let rect = centered_rect(72, 70, frame.area());
                frame.render_widget(Clear, rect);
                frame.render_widget(
                    Paragraph::new(finding_modal_lines(finding))
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(" FINDING ")
                                .border_style(Style::default().fg(Color::LightRed)),
                        )
                        .wrap(Wrap { trim: true }),
                    rect,
                );
            }
            if help_open {
                let rect = centered_rect(76, 55, frame.area());
                frame.render_widget(Clear, rect);
                frame.render_widget(
                    Paragraph::new(help_lines())
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(" HELP ")
                                .border_style(Style::default().fg(Color::Cyan)),
                        )
                        .wrap(Wrap { trim: true }),
                    rect,
                );
            }
        })?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        // An unparseable terminal event (e.g. an unsupported mouse protocol
        // sequence) must never take the dashboard down.
        let Ok(read) = event::read() else { continue; };
        match read {
            CEvent::Key(key) if key.kind == KeyEventKind::Press => {
                if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    if let Some(cancel) = &drive_cancel {
                        let _ = Coordinator::new(store.clone())
                            .pause_budget(selected_id, "operator quit TUI");
                        cancel.cancel();
                    }
                    break;
                }
                if key.code == KeyCode::F(1) {
                    help_open = !help_open;
                    continue;
                }
                if key.code == KeyCode::Esc {
                    // A bare Esc never quits: Windows terminals can deliver a
                    // mouse click as a spurious Esc, and clicks right before
                    // an Esc are ignored entirely.
                    let spurious = last_mouse_at
                        .is_some_and(|at| at.elapsed() < Duration::from_millis(250));
                    if spurious {
                        continue;
                    }
                    if modal.take().is_some() {
                        continue;
                    }
                    if detail_agent.take().is_some() {
                        continue;
                    }
                    if !editor.is_empty() {
                        clear_editor(&mut editor);
                    }
                    continue;
                }
                if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    if !run.status.terminal() {
                        Coordinator::new(store.clone())
                            .pause_budget(selected_id, "operator paused in TUI")?;
                    }
                    continue;
                }
                if modal.is_some() {
                    if key.code == KeyCode::Enter {
                        modal = None;
                    }
                    continue;
                }
                if help_open {
                    continue;
                }
                if key.code == KeyCode::Tab {
                    focus = if focus == Focus::Editor {
                        Focus::Tree
                    } else {
                        Focus::Editor
                    };
                    continue;
                }
                match focus {
                    Focus::Editor => {
                        if key.code == KeyCode::Enter {
                            let text = editor.lines().join(" ").trim().to_owned();
                            clear_editor(&mut editor);
                            if !text.is_empty() && !run.status.terminal() {
                                let target = detail_agent
                                    .clone()
                                    .or_else(|| agents.get(tree_cursor).map(|agent| agent.id.clone()))
                                    .unwrap_or_else(|| run.root_id.clone());
                                Coordinator::new(store.clone())
                                    .message("operator", &target, &text)?;
                            }
                            continue;
                        }
                        // Arrow keys scroll the panes even while typing.
                        match key.code {
                            KeyCode::Up => {
                                follow = false;
                                scroll = scroll.saturating_add(1);
                                continue;
                            }
                            KeyCode::Down => {
                                if scroll > 0 {
                                    scroll -= 1;
                                    if scroll == 0 {
                                        follow = true;
                                    }
                                }
                                continue;
                            }
                            KeyCode::PageUp => {
                                follow = false;
                                scroll = scroll.saturating_add(10);
                                continue;
                            }
                            KeyCode::PageDown => {
                                scroll = scroll.saturating_sub(10);
                                if scroll == 0 {
                                    follow = true;
                                }
                                continue;
                            }
                            _ => {}
                        }
                        let input: ratatui_textarea::Input = key.into();
                        editor.input(input);
                    }
                    Focus::Tree => match key.code {
                        KeyCode::Up => tree_cursor = tree_cursor.saturating_sub(1),
                        KeyCode::Down => {
                            tree_cursor = (tree_cursor + 1).min(agents.len().saturating_sub(1))
                        }
                        KeyCode::Enter => {
                            if let Some(agent) = agents.get(tree_cursor) {
                                detail_agent = Some(agent.id.clone());
                                detail_scroll = 0;
                            }
                        }
                        KeyCode::Char('g') => detail_agent = None,
                        _ => {}
                    },
                }
            }
            CEvent::Mouse(mouse) => {
                last_mouse_at = Some(std::time::Instant::now());
                match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(rect) = last_tree_rect {
                        if rect.contains((mouse.column, mouse.row).into()) {
                            let row = mouse.row.saturating_sub(rect.y + 1) as usize;
                            if row < agents.len() {
                                tree_cursor = row;
                                detail_agent = Some(agents[row].id.clone());
                                detail_scroll = 0;
                                focus = Focus::Tree;
                            }
                        }
                    }
                    if let Some(rect) = last_findings_rect {
                        if rect.contains((mouse.column, mouse.row).into()) {
                            let row = mouse.row.saturating_sub(rect.y + 1) as usize;
                            if row < findings.len() {
                                modal = Some(findings[row].clone());
                            }
                        }
                    }
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    let up = matches!(mouse.kind, MouseEventKind::ScrollUp);
                    let over_right = last_right_rect
                        .is_some_and(|rect| rect.contains((mouse.column, mouse.row).into()));
                    if detail_agent.is_some() && over_right {
                        if up {
                            detail_scroll = detail_scroll.saturating_add(1);
                        } else {
                            detail_scroll = detail_scroll.saturating_sub(1);
                        }
                    } else if up {
                        follow = false;
                        scroll = scroll.saturating_add(1);
                    } else if scroll > 0 {
                        scroll -= 1;
                        if scroll == 0 {
                            follow = true;
                        }
                    }
                }
                _ => {}
            }
            }
            CEvent::Paste(text) => {
                if focus == Focus::Editor {
                    editor.insert_str(&text);
                }
            }
            _ => {}
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
