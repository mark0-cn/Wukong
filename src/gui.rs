mod commands;
mod content;
mod controller;
mod dialogs;
mod sidebar;
mod state;
use crate::gui_data::{self, AgentSummary, RunSummary};
use crate::{
    application::AssessmentService, config::AppConfig, domain::*, interface::DEFAULT_TASK, report,
    store::Store,
};
use anyhow::{Result, ensure};
use commands::{Command, Key, Outcome};
use eframe::egui::{self, Color32, RichText};
use state::*;
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const ACCENT: Color32 = Color32::from_rgb(83, 218, 189);
const MUTED: Color32 = Color32::from_rgb(145, 161, 181);
const SURFACE: Color32 = Color32::from_rgb(24, 33, 46);

pub fn run(
    config: AppConfig,
    store: Store,
    selected: Option<String>,
    config_path: Option<PathBuf>,
    state_override: Option<PathBuf>,
) -> Result<()> {
    if let Some(id) = &selected {
        store.require::<RunRecord>("run", id)?;
    }
    let mut app = Workspace::new(config, store, selected);
    app.config_path = config_path.unwrap_or_else(|| PathBuf::from("re-agent.toml"));
    app.state_override = state_override;
    eframe::run_native(
        "RE Agent · Analysis Workspace",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1320.0, 860.0])
                .with_min_inner_size([960.0, 640.0]),
            centered: true,
            ..Default::default()
        },
        Box::new(move |cc| {
            theme(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| anyhow::anyhow!("desktop window: {error}"))
}

fn theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = Color32::from_rgb(16, 23, 34);
    style.visuals.window_fill = SURFACE;
    style.visuals.extreme_bg_color = Color32::from_rgb(12, 19, 29);
    style.visuals.selection.bg_fill = Color32::from_rgb(30, 94, 86);
    style.visuals.selection.stroke.color = Color32::WHITE;
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(25, 37, 50);
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(25, 37, 50);
    style.visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0_f32, Color32::from_rgb(42, 56, 71));
    style.visuals.widgets.inactive.fg_stroke.color = Color32::from_rgb(216, 225, 235);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(34, 110, 98);
    style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(34, 110, 98);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(39, 62, 76);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(39, 62, 76);
    for visual in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
    ] {
        visual.corner_radius = egui::CornerRadius::same(6);
    }
    style.spacing.item_spacing = egui::vec2(12.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
    ctx.set_style(style);
    // Use installed CJK fonts without redistributing platform font files.
    for path in [
        "C:/Windows/Fonts/msyh.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("cjk".into(), egui::FontData::from_owned(bytes).into());
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push("cjk".into());
            }
            ctx.set_fonts(fonts);
            break;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReadRequest {
    root: PathBuf,
    run: Option<String>,
    agent: Option<String>,
    epoch: u64,
    serial: u64,
    event_limit: usize,
}
#[derive(Clone)]
struct StopTarget {
    root: PathBuf,
    run: String,
    agent: String,
    name: String,
}

enum Update {
    Started(String),
    Finished(Result<()>),
    CommandFinished {
        root: PathBuf,
        key: Key,
        result: Result<Outcome>,
    },
    Loaded {
        request: ReadRequest,
        reader: Box<gui_data::Reader>,
        result: Result<Box<gui_data::Delta>>,
    },
}
#[derive(PartialEq)]
enum View {
    Activity,
    Agent,
    Findings,
    Overview,
}

struct Workspace {
    config_path: PathBuf,
    state_override: Option<PathBuf>,
    settings: Option<crate::gui_settings::ConfigEditor>,
    config: AppConfig,
    store: Store,
    runtime: tokio::runtime::Handle,
    tx: mpsc::Sender<Update>,
    rx: mpsc::Receiver<Update>,
    commands: commands::Controller,
    data: WorkspaceData,
    selection: Selection,
    composer: Composer,
    chat: ChatView,
    draft: TaskDraft,
    dialogs: Dialogs,
    execution: Execution,
    technical_events: bool,
    follow: bool,
    notice: Option<(bool, String)>,
    refreshed: Instant,
    reader: Option<gui_data::Reader>,
}

impl Workspace {}

impl eframe::App for Workspace {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.commands.context = Some(ctx.clone());
        while let Ok(update) = self.rx.try_recv() {
            self.receive(update);
        }
        if self.dialogs.close == CloseState::Waiting
            && !self.execution.busy()
            && !self.commands.contains(&Key::Export)
            && !self.commands.busy()
        {
            self.dialogs.close = CloseState::Allowed;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input(|i| i.viewport().close_requested())
            && self.dialogs.close != CloseState::Allowed
            && (self.execution.busy()
                || self.commands.contains(&Key::Export)
                || self.commands.busy())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.dialogs.close == CloseState::Open {
                self.dialogs.close = CloseState::Confirming;
            }
        }
        if self.refreshed.elapsed() >= Duration::from_secs(1)
            && let Err(e) = self.refresh()
        {
            self.error(e);
            self.refreshed = Instant::now();
        }
        let dropped = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
        if let Some(path) = dropped {
            self.draft.target = path.display().to_string();
            self.dialogs.task = TaskDialog::New;
        }
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if self.execution.busy() {
                    ui.spinner();
                }
                let mut dismiss = false;
                if let Some((error, text)) = &self.notice {
                    ui.colored_label(if *error { Color32::LIGHT_RED } else { ACCENT }, text);
                    dismiss = ui.small_button("关闭提示").clicked();
                } else {
                    ui.label(
                        RichText::new(format!(
                            "{}  ·  {}",
                            self.config.model.model,
                            self.store.root().display()
                        ))
                        .small()
                        .color(MUTED),
                    );
                }
                if dismiss {
                    self.notice = None;
                }
            });
        });
        self.sidebar(ctx);
        self.content(ctx);
        self.dialogs(ctx);
        if self.selection.agent.as_deref()
            != self
                .data
                .conversation
                .as_ref()
                .map(|agent| agent.id.as_str())
        {
            let _ = self.refresh();
        }
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}

fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .corner_radius(10)
        .inner_margin(16)
}
fn selectable_text(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(text).wrap().selectable(true));
}
fn status_label(status: Status) -> &'static str {
    match status {
        Status::Queued => "等待开始",
        Status::Running => "分析中",
        Status::Waiting => "等待协作",
        Status::BudgetPaused => "已暂停",
        Status::Completed => "已完成",
        Status::Stopped => "已停止",
        Status::Crashed => "异常中断",
        Status::Failed => "失败",
    }
}
fn status_color(status: Status) -> Color32 {
    match status {
        Status::Running | Status::Completed => ACCENT,
        Status::Failed | Status::Crashed => Color32::LIGHT_RED,
        Status::BudgetPaused | Status::Waiting => Color32::from_rgb(240, 192, 110),
        _ => MUTED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::Coordinator;
    use clap::Parser;

    #[test]
    fn desktop_is_default_and_terminal_dashboard_is_removed() {
        assert!(
            crate::interface::Cli::try_parse_from(["re-agent"])
                .unwrap()
                .command
                .is_none()
        );
        assert!(matches!(
            crate::interface::Cli::try_parse_from(["re-agent", "gui", "run-1"])
                .unwrap()
                .command,
            Some(crate::interface::Command::Gui { run_id: Some(_) })
        ));
        assert!(crate::interface::Cli::try_parse_from(["re-agent", "tui"]).is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn background_commands_deduplicate_and_preserve_newer_drafts() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut app = Workspace::new(AppConfig::default(), Store::open(dir.path())?, None);
        app.dispatch(Command::Pause("missing".into()));
        app.dispatch(Command::Pause("missing".into()));
        assert!(app.commands.contains(&Key::Pause("missing".into())));
        loop {
            if let Update::CommandFinished { key, result, .. } =
                app.rx.recv_timeout(Duration::from_secs(5))?
            {
                assert!(result.is_err());
                app.commands.complete(&key);
                break;
            }
        }
        assert!(!app.commands.busy());
        app.selection.agent = Some("a".into());
        app.composer.message = "new draft".into();
        app.command_completed(Outcome::Sent {
            recipient: "a".into(),
            text: "old draft".into(),
        });
        assert_eq!(app.composer.message, "new draft");
        app.selection.agent = Some("b".into());
        app.command_completed(Outcome::Sent {
            recipient: "a".into(),
            text: "new draft".into(),
        });
        assert_eq!(app.composer.message, "new draft");
        Ok(())
    }

    #[tokio::test]
    async fn refresh_identity_includes_agent_and_selection_epoch() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut app = Workspace::new(
            AppConfig::default(),
            Store::open(dir.path())?,
            Some("run".into()),
        );
        let request = app.read_request();
        app.select_agent("agent-a".into());
        assert_ne!(request, app.read_request());
        let request = app.read_request();
        app.select_agent("agent-b".into());
        app.select_agent("agent-a".into());
        assert_ne!(request, app.read_request());
        let target = StopTarget {
            root: app.store.root().to_owned(),
            run: "run".into(),
            agent: "agent-a".into(),
            name: "A".into(),
        };
        app.dialogs.stop = Some(target);
        app.select_agent("agent-b".into());
        assert_eq!(app.dialogs.stop.as_ref().unwrap().agent, "agent-a");
        Ok(())
    }

    #[tokio::test]
    async fn reselecting_current_run_preserves_loaded_and_in_flight_data() -> Result<()> {
        fn finish_refresh(app: &mut Workspace) -> Result<()> {
            let Update::Loaded { reader, result, .. } =
                app.rx.recv_timeout(Duration::from_secs(5))?
            else {
                anyhow::bail!("expected a background data refresh");
            };
            app.reader = Some(*reader);
            app.apply_delta(*result?);
            Ok(())
        }

        let dir = tempfile::tempdir()?;
        let target = dir.path().join("sample");
        std::fs::write(&target, b"fixture")?;
        let store = Store::open(dir.path().join("state"))?;
        let config = AppConfig::default();
        let coordinator = Coordinator::new(store.clone());
        let run = coordinator.create_run(
            &target,
            "selection regression fixture",
            config.budget.clone(),
            serde_json::to_value(&config)?,
        )?;
        let mut app = Workspace::new(config.clone(), store, Some(run.id.clone()));
        finish_refresh(&mut app)?;
        app.selection.agent = Some(run.root_id.clone());
        app.selection.event_limit = 1_000;
        app.refresh()?;
        finish_refresh(&mut app)?;
        assert_eq!(app.data.agents.len(), 1);
        assert_eq!(app.data.events.len(), 1);
        assert_eq!(app.data.conversation.as_ref().unwrap().id, run.root_id);
        app.composer.message = "unsent operator message".into();

        for in_flight in [false, true] {
            if in_flight {
                app.refresh()?;
            }
            app.select(run.id.clone());
            app.refresh()?;
            finish_refresh(&mut app)?;
            assert_eq!(app.data.agents.len(), 1);
            assert_eq!(app.data.agents[0].id, run.root_id);
            assert_eq!(app.data.events.len(), 1);
            assert_eq!(app.selection.agent.as_deref(), Some(run.root_id.as_str()));
            assert_eq!(app.data.conversation.as_ref().unwrap().id, run.root_id);
            assert_eq!(app.selection.event_limit, 1_000);
            assert_eq!(app.composer.message, "unsent operator message");
        }

        let other = coordinator.create_run(
            &target,
            "another selection",
            config.budget.clone(),
            serde_json::to_value(&config)?,
        )?;
        app.select(other.id.clone());
        finish_refresh(&mut app)?;
        assert_eq!(app.selection.selected.as_deref(), Some(other.id.as_str()));
        assert_eq!(app.data.agents.len(), 1);
        assert_eq!(app.data.agents[0].id, other.root_id);
        assert!(app.selection.agent.is_none() && app.data.conversation.is_none());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn desktop_views_render_empty_and_saved_assessments() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let config = AppConfig::default();
        let mut app = Workspace::new(config.clone(), store.clone(), None);
        let ctx = egui::Context::default();
        let render = |app: &mut Workspace| {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(960.0, 640.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    app.sidebar(ctx);
                    app.content(ctx);
                    app.dialogs(ctx);
                },
            );
            assert!(!output.shapes.is_empty());
        };
        render(&mut app);
        let run = Coordinator::new(store.clone()).create_run(
            &std::env::current_exe()?,
            "GUI regression fixture",
            config.budget.clone(),
            serde_json::to_value(config)?,
        )?;
        app.select(run.id.clone());
        app.dialogs.task = TaskDialog::None;
        app.selection.agent = Some(run.root_id);
        let checkpoint = serde_json::json!({
            "messages": [
                {"role":"user", "content":[{"type":"text", "text":"internal context"}]},
                {"role":"assistant", "content":[{"type":"text", "text":"已确认文件格式，接下来检查输入处理。"}]}
            ],
            "asides": [], "meta": [{"id": 0}, {"id": 1}], "next_id": 2, "next_aside": 0
        });
        store.checkpoint_and_ack(app.selection.agent.as_deref().unwrap(), checkpoint, &[])?;
        let mut reader = gui_data::Reader::default();
        let delta = reader.read(
            &store,
            app.selection.selected.as_deref(),
            app.selection.agent.as_deref(),
            500,
        )?;
        app.apply_delta(delta);
        assert_eq!(
            app.data.agents[0].public_replies,
            vec!["已确认文件格式，接下来检查输入处理。"]
        );

        for view in [View::Activity, View::Agent, View::Findings, View::Overview] {
            app.selection.view = view;
            render(&mut app);
        }
        app.pause();
        loop {
            if let Update::CommandFinished { key, result, .. } =
                app.rx.recv_timeout(Duration::from_secs(5))?
            {
                app.commands.complete(&key);
                app.command_completed(result?);
                break;
            }
        }
        assert_eq!(
            store.require::<RunRecord>("run", &run.id)?.status,
            Status::BudgetPaused
        );
        Ok(())
    }

    #[tokio::test]
    async fn invalid_target_reports_error_without_creating_run() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let mut app = Workspace::new(AppConfig::default(), store.clone(), None);
        app.draft.target = dir.path().join("missing-sample").display().to_string();
        app.start(None);
        let update = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(update @ Update::Finished(_)) = app.rx.try_recv() {
                    break update;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        assert!(matches!(update, Update::Finished(Err(_))));
        assert!(store.runs()?.is_empty());
        Ok(())
    }
}
