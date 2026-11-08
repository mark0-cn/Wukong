mod commands;
use crate::gui_data::{self, AgentSummary, RunSummary};
use crate::{
    application::AssessmentService, config::AppConfig, domain::*, interface::DEFAULT_TASK, report,
    store::Store,
};
use anyhow::{Result, ensure};
use commands::{Command, Key, Outcome};
use eframe::egui::{self, Color32, RichText};
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
    commands: commands::Controller,
    config_path: PathBuf,
    state_override: Option<PathBuf>,
    settings: Option<crate::gui_settings::ConfigEditor>,
    config: AppConfig,
    store: Store,
    runtime: tokio::runtime::Handle,
    tx: mpsc::Sender<Update>,
    rx: mpsc::Receiver<Update>,
    runs: Vec<RunSummary>,
    selected: Option<String>,
    agents: Vec<AgentSummary>,
    events: Vec<Event>,
    calls: Vec<ToolCallRecord>,
    notes: Vec<SharedDocument>,
    technical_events: bool,
    findings: Vec<Finding>,
    agent: Option<String>,
    view: View,
    target: String,
    task: String,
    message: String,
    follow_up: bool,
    filter: String,
    follow: bool,
    chat_follow: bool,
    chat_jump: bool,
    chat_details: bool,
    new_task: bool,
    cancel: Option<CancellationToken>,
    active_run: Option<String>,
    exporting: bool,
    notice: Option<(bool, String)>,
    refreshed: Instant,
    resume_run: Option<RunSummary>,
    reader: Option<gui_data::Reader>,
    conversation: Option<crate::gui_chat::Document>,
    chat_layout: crate::gui_chat::Layout,
    event_limit: usize,
    confirm_stop: Option<StopTarget>,
    selection_epoch: u64,
    request_serial: u64,
    confirm_close: bool,
    closing: bool,
    allow_close: bool,
}

impl Workspace {
    fn new(config: AppConfig, store: Store, selected: Option<String>) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            commands: commands::Controller::new(tokio::runtime::Handle::current(), tx.clone()),
            config_path: PathBuf::from("re-agent.toml"),
            state_override: None,
            settings: None,
            config,
            store,
            runtime: tokio::runtime::Handle::current(),
            tx,
            rx,
            runs: vec![],
            selected,
            agents: vec![],
            events: vec![],
            calls: vec![],
            notes: vec![],
            technical_events: false,
            findings: vec![],
            agent: None,
            view: View::Activity,
            target: String::new(),
            task: DEFAULT_TASK.into(),
            message: String::new(),
            follow_up: false,
            filter: String::new(),
            follow: true,
            chat_follow: true,
            chat_jump: true,
            chat_details: false,
            new_task: false,
            cancel: None,
            active_run: None,
            exporting: false,
            notice: None,
            refreshed: Instant::now(),
            resume_run: None,
            reader: Some(gui_data::Reader::default()),
            conversation: None,
            chat_layout: crate::gui_chat::Layout::default(),
            event_limit: 500,
            confirm_stop: None,
            selection_epoch: 0,
            request_serial: 0,
            confirm_close: false,
            closing: false,
            allow_close: false,
        };
        if let Err(e) = app.refresh() {
            app.error(e);
        }
        app.new_task = app.runs.is_empty();
        app
    }
    fn error(&mut self, error: impl std::fmt::Display) {
        self.notice = Some((true, error.to_string()));
    }
    fn refresh(&mut self) -> Result<()> {
        let Some(mut reader) = self.reader.take() else {
            return Ok(());
        };
        let store = self.store.clone();
        self.request_serial += 1;
        let request = self.read_request();
        let tx = self.tx.clone();
        self.refreshed = Instant::now();
        self.runtime.spawn_blocking(move || {
            let result = reader
                .read(
                    &store,
                    request.run.as_deref(),
                    request.agent.as_deref(),
                    request.event_limit,
                )
                .map(Box::new);
            let _ = tx.send(Update::Loaded {
                request,
                reader: Box::new(reader),
                result,
            });
        });
        Ok(())
    }
    fn read_request(&self) -> ReadRequest {
        ReadRequest {
            root: self.store.root().to_owned(),
            run: self.selected.clone(),
            agent: self.agent.clone(),
            epoch: self.selection_epoch,
            serial: self.request_serial,
            event_limit: self.event_limit,
        }
    }
    fn loaded(
        &mut self,
        request: ReadRequest,
        reader: gui_data::Reader,
        result: Result<Box<gui_data::Delta>>,
    ) {
        self.reader = Some(reader);
        if request == self.read_request() {
            match result {
                Ok(delta) => self.apply_delta(*delta),
                Err(error) => self.error(error),
            }
        } else {
            self.reader = Some(gui_data::Reader::default());
            let _ = self.refresh();
        }
    }
    fn select_agent(&mut self, id: String) {
        if self.agent.as_ref() != Some(&id) {
            self.selection_epoch += 1;
            self.agent = Some(id);
            self.conversation = None;
            self.chat_jump = true;
            self.chat_follow = true;
        }
    }
    fn apply_delta(&mut self, mut delta: gui_data::Delta) {
        let was_empty = self.runs.is_empty();
        gui_data::apply(&mut self.runs, delta.runs, |run| &run.id);
        self.runs
            .sort_by_key(|run| std::cmp::Reverse(run.created_at));
        if delta.reset {
            self.agents.clear();
            self.notes.clear();
            self.findings.clear();
            self.events.clear();
            self.calls.clear();
            self.conversation = None;
        }
        self.selected = delta.selected;
        for change in &mut delta.agents {
            if let Some(agent) = change.value.as_mut()
                && agent.public_replies.is_empty()
                && let Some(previous) = self.agents.iter().find(|previous| {
                    previous.id == agent.id && previous.generation == agent.generation
                })
            {
                agent.public_replies = previous.public_replies.clone();
            }
        }
        gui_data::apply(&mut self.agents, delta.agents, |agent| &agent.id);
        self.agents
            .sort_by_key(|agent| (agent.depth, agent.created_at));
        gui_data::apply(&mut self.notes, delta.notes, |note| &note.id);
        gui_data::apply(&mut self.findings, delta.findings, |finding| &finding.id);
        if let Some(events) = delta.events {
            self.events = events;
        }
        if let Some(calls) = delta.calls {
            self.calls = calls;
        }
        if let Some(conversation) = delta.conversation {
            self.conversation = conversation;
        }
        if was_empty && !self.runs.is_empty() {
            self.new_task = false;
        }
    }
    fn select(&mut self, id: String) {
        if self.selected.as_deref() == Some(id.as_str()) {
            return;
        }
        self.selection_epoch += 1;
        self.selected = Some(id);
        self.agent = None;
        self.conversation = None;
        self.events.clear();
        self.calls.clear();
        self.agents.clear();
        self.notes.clear();
        self.findings.clear();
        self.event_limit = 500;
        self.message.clear();
        if let Err(e) = self.refresh() {
            self.error(e);
        }
    }
    fn start(&mut self, resume: Option<RunSummary>) {
        if self.cancel.is_some() || self.commands.busy() {
            return;
        }
        let config = self.config.clone();
        let store = self.store.clone();
        let target = self.target.trim().trim_matches('"').to_owned();
        let task = self.task.trim().to_owned();
        if resume.is_none() && (target.is_empty() || task.is_empty()) {
            self.error("请选择样本，并填写分析目标。");
            return;
        }
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        let tx = self.tx.clone();
        self.notice = Some((false, "正在连接分析引擎…".into()));
        self.runtime.spawn(async move {
            let result = async {
                let service = AssessmentService::new(config, store);
                let prepared = if let Some(summary) = resume {
                    service.resume(&summary.id, Some(summary.budget)).await?
                } else {
                    service.start(std::path::Path::new(&target), &task).await?
                };
                let _ = tx.send(Update::Started(prepared.run().id.clone()));
                let run = prepared.drive(cancel).await?;
                ensure!(
                    run.status != Status::Failed,
                    "分析失败，请查看 Agent 错误和活动记录。"
                );
                Ok(())
            }
            .await;
            let _ = tx.send(Update::Finished(result));
        });
    }
    fn dispatch(&mut self, command: Command) {
        self.commands
            .submit(command, self.config.clone(), self.store.clone());
    }
    fn pause(&mut self) {
        if let Some(id) = self.selected.clone() {
            self.dispatch(Command::Pause(id));
        }
    }
    fn export(&mut self) {
        if let Some(id) = self.selected.clone() {
            self.exporting = true;
            self.dispatch(Command::Export(id));
        }
    }
    fn send(&mut self) {
        let recipient = self.agent.clone().or_else(|| {
            self.runs
                .iter()
                .find(|r| Some(&r.id) == self.selected.as_ref())
                .map(|r| r.root_id.clone())
        });
        if let Some(recipient) = recipient {
            let options = MessageOptions {
                kind: if self.follow_up {
                    MessageKind::FollowUp
                } else {
                    MessageKind::Information
                },
                ..Default::default()
            };
            self.dispatch(Command::Message {
                recipient,
                text: self.message.trim().to_owned(),
                options,
            });
        }
    }
    fn command_completed(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Paused(id) => {
                if self.active_run.as_ref() == Some(&id)
                    && let Some(cancel) = &self.cancel
                {
                    cancel.cancel();
                }
                self.notice = Some((false, "已请求暂停，正在保存进行中的工作。".into()));
            }
            Outcome::Stopped => self.notice = Some((false, "已停止指定 Agent 分支。".into())),
            Outcome::Sent { recipient, text } => {
                let current = self.agent.clone().or_else(|| {
                    self.runs
                        .iter()
                        .find(|r| Some(&r.id) == self.selected.as_ref())
                        .map(|r| r.root_id.clone())
                });
                if current.as_ref() == Some(&recipient) && self.message.trim() == text {
                    self.message.clear();
                }
                self.notice = Some((false, "消息已送入 Agent 收件箱。".into()));
            }
            Outcome::Exported(path) => {
                if let Some(path) = path {
                    self.notice = Some((false, format!("报告已导出：{}", path.display())));
                }
            }
            Outcome::Target(path) => {
                if let Some(path) = path {
                    self.target = path.display().to_string();
                }
            }
            Outcome::SettingsOpened(editor) => {
                if let Some(editor) = editor {
                    self.settings = Some(*editor);
                }
            }
            Outcome::SettingsSaved {
                editor,
                config,
                store,
            } => {
                let changed_store = store.root() != self.store.root();
                self.config = *config;
                self.config_path = editor.path.clone();
                self.store = store;
                self.settings = Some(*editor);
                if changed_store {
                    self.selection_epoch += 1;
                    self.confirm_stop = None;
                    self.selected = None;
                    self.agent = None;
                    self.runs.clear();
                    self.conversation = None;
                    self.agents.clear();
                    self.events.clear();
                    self.calls.clear();
                    self.notes.clear();
                    self.findings.clear();
                    self.message.clear();
                    self.resume_run = None;
                }
                self.notice = Some((
                    false,
                    "配置已保存；新建分析使用新设置，恢复任务沿用历史配置。".into(),
                ));
            }
        }
    }
    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("navigation")
            .default_width(245.0)
            .min_width(205.0)
            .max_width(360.0)
            .show(ctx, |ui| {
                ui.add_space(18.0);
                ui.label(
                    RichText::new("RE / AGENT")
                        .size(25.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.label(
                    RichText::new("BINARY ANALYSIS WORKSPACE")
                        .size(10.0)
                        .color(MUTED),
                );
                ui.add_space(24.0);
                if ui
                    .add_sized(
                        [ui.available_width(), 40.0],
                        egui::Button::new("＋  新建分析"),
                    )
                    .clicked()
                {
                    self.new_task = true;
                }
                if ui
                    .add_sized(
                        [ui.available_width(), 36.0],
                        egui::Button::new("设置 / 配置文件"),
                    )
                    .clicked()
                {
                    self.dispatch(Command::OpenSettings(Some(self.config_path.clone())));
                    self.new_task = false;
                }
                ui.add_space(18.0);
                ui.label(RichText::new("分析任务").color(MUTED));
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("搜索样本 / 任务"));
                ui.separator();
                let mut selected = None;
                let filter = self.filter.to_lowercase();
                let visible: Vec<_> = self
                    .runs
                    .iter()
                    .filter(|run| {
                        format!(
                            "{} {} {}",
                            run.target_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy(),
                            run.id,
                            run.task
                        )
                        .to_lowercase()
                        .contains(&filter)
                    })
                    .collect();
                egui::ScrollArea::vertical().id_salt("runs").show_rows(
                    ui,
                    64.0,
                    visible.len(),
                    |ui, range| {
                        for run in &visible[range] {
                            let name = run
                                .target_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy();
                            let label = format!(
                                "{}\n{} · {}",
                                name,
                                status_label(run.status),
                                run.created_at
                                    .with_timezone(&chrono::Local)
                                    .format("%m/%d %H:%M")
                            );
                            if ui
                                .add_sized(
                                    [ui.available_width(), 64.0],
                                    egui::Button::new(label)
                                        .selected(self.selected.as_ref() == Some(&run.id)),
                                )
                                .on_hover_text(&run.id)
                                .clicked()
                            {
                                selected = Some(run.id.clone());
                            }
                        }
                        if self.runs.is_empty() {
                            ui.label(
                                RichText::new("还没有分析任务\n选择样本，开始第一次分析。")
                                    .color(MUTED),
                            );
                        }
                    },
                );
                if let Some(id) = selected {
                    self.select(id);
                }
            });
    }
    fn content(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(12.0);
            let run = self
                .runs
                .iter()
                .find(|r| Some(&r.id) == self.selected.as_ref())
                .cloned();
            let Some(run) = run else {
                ui.add_space(100.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("从一个二进制文件开始").size(32.0).strong());
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new("理解行为，追踪证据，协作完成分析。")
                            .size(18.0)
                            .color(MUTED),
                    );
                    ui.add_space(30.0);
                    if ui.button("选择样本并创建分析").clicked() {
                        self.new_task = true;
                    }
                    ui.label(RichText::new("也可以把 ELF、PE 或 Mach-O 文件拖入窗口").color(MUTED));
                });
                return;
            };
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(
                            run.target_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy(),
                        )
                        .size(27.0)
                        .strong(),
                    );
                    ui.label(
                        RichText::new(run.target_path.display().to_string())
                            .small()
                            .color(MUTED),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !self.exporting,
                            egui::Button::new(if self.exporting {
                                "导出中…"
                            } else {
                                "导出报告"
                            }),
                        )
                        .clicked()
                    {
                        self.export();
                    }
                    if ui
                        .add_enabled(
                            !run.status.terminal()
                                && self.cancel.is_none()
                                && !self.commands.busy(),
                            egui::Button::new("恢复"),
                        )
                        .clicked()
                    {
                        self.resume_run = Some(run.clone());
                    }
                    if ui
                        .add_enabled(
                            !run.status.terminal()
                                && run.status != Status::BudgetPaused
                                && !self.commands.contains(&Key::Pause(run.id.clone())),
                            egui::Button::new("暂停"),
                        )
                        .clicked()
                    {
                        self.pause();
                    }
                });
            });
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(status_color(run.status), status_label(run.status));
                ui.separator();
                ui.label(format!("{} 个 Agent", self.agents.len()));
                ui.separator();
                ui.label(format!("{} 项发现", self.findings.len()));
                ui.separator();
                ui.label(format!(
                    "{} / {} tokens",
                    run.total_tokens, run.budget.max_total_tokens
                ));
                ui.separator();
                ui.label(format!("${:.4}", run.cost_usd));
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.view, View::Activity, "实时活动");
                ui.selectable_value(&mut self.view, View::Agent, "Agents / 会话");
                ui.selectable_value(&mut self.view, View::Findings, "分析发现");
                ui.selectable_value(&mut self.view, View::Overview, "任务概览");
            });
            ui.separator();
            egui::TopBottomPanel::bottom("composer")
                .frame(egui::Frame::new().fill(SURFACE).inner_margin(14))
                .show_inside(ui, |ui| {
                    let recipient = self
                        .agents
                        .iter()
                        .find(|a| Some(&a.id) == self.agent.as_ref())
                        .map(|a| a.name.as_str())
                        .unwrap_or("Root Agent");
                    ui.horizontal(|ui| {
                        ui.label(format!("发送给 {recipient}"));
                        ui.checkbox(&mut self.follow_up, "作为后续任务")
                            .on_hover_text("重新激活已完成的 Agent；暂停的分析仍需点击恢复。");
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [ui.available_width() - 88.0, 54.0],
                            egui::TextEdit::multiline(&mut self.message)
                                .hint_text("补充约束、提出问题，或指定下一步分析…"),
                        );
                        if ui
                            .add_enabled(
                                !self.message.trim().is_empty()
                                    && !self.commands.contains(&Key::Message(
                                        self.agent.clone().unwrap_or_else(|| run.root_id.clone()),
                                    )),
                                egui::Button::new("发送"),
                            )
                            .clicked()
                        {
                            self.send();
                        }
                    });
                });
            match self.view {
                View::Activity => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Agent 动态与分析记录").color(MUTED));
                        ui.checkbox(&mut self.technical_events, "显示技术事件");
                        ui.checkbox(&mut self.follow, "跟随最新");
                    });
                    if self.events.len() >= self.event_limit
                        && self.event_limit < 10_000
                        && ui.button("加载更早活动").clicked()
                    {
                        self.event_limit = (self.event_limit + 500).min(10_000);
                        let _ = self.refresh();
                    }
                    egui::ScrollArea::vertical()
                        .id_salt(("events", &run.id))
                        .stick_to_bottom(self.follow)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for agent in &self.agents {
                                let replies = &agent.public_replies;
                                if !replies.is_empty() {
                                    card().show(ui, |ui| {
                                        ui.colored_label(
                                            ACCENT,
                                            format!("{} · 公开回复", agent.name),
                                        );
                                        for reply in replies
                                            .iter()
                                            .rev()
                                            .take(3)
                                            .collect::<Vec<_>>()
                                            .into_iter()
                                            .rev()
                                        {
                                            selectable_text(ui, reply);
                                            ui.add_space(8.0);
                                        }
                                        if ui.small_button("查看完整会话").clicked() {
                                            self.selection_epoch += 1;
                                            self.agent = Some(agent.id.clone());
                                            self.conversation = None;
                                            self.view = View::Agent;
                                        }
                                    });
                                    ui.add_space(10.0);
                                }
                            }
                            for note in &self.notes {
                                card().show(ui, |ui| {
                                    let author = self
                                        .agents
                                        .iter()
                                        .find(|a| a.id == note.agent_id)
                                        .map(|a| a.name.as_str())
                                        .unwrap_or("Agent");
                                    ui.colored_label(ACCENT, format!("{author} · 分析笔记"));
                                    egui::CollapsingHeader::new(&note.title)
                                        .id_salt(&note.id)
                                        .default_open(true)
                                        .show(ui, |ui| {
                                            selectable_text(ui, &note.body);
                                        });
                                });
                                ui.add_space(10.0);
                            }
                            if self.events.is_empty() {
                                ui.label("等待 Agent 开始分析…");
                            }
                            for event in &self.events {
                                if !self.technical_events && crate::gui_activity::technical(event) {
                                    continue;
                                }
                                let call = event
                                    .data
                                    .get("call_id")
                                    .and_then(serde_json::Value::as_str)
                                    .and_then(|id| self.calls.iter().find(|call| call.id == id));
                                let (title, detail) = crate::gui_activity::describe(event, call);
                                let author = self
                                    .agents
                                    .iter()
                                    .find(|a| Some(&a.id) == event.agent_id.as_ref())
                                    .map(|a| a.name.as_str())
                                    .unwrap_or("工作区");
                                ui.push_id(event.seq, |ui| {
                                    card().show(ui, |ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.colored_label(ACCENT, author);
                                            ui.label(
                                                RichText::new(
                                                    event
                                                        .created_at
                                                        .with_timezone(&chrono::Local)
                                                        .format("%H:%M:%S")
                                                        .to_string(),
                                                )
                                                .small()
                                                .color(MUTED),
                                            );
                                        });
                                        ui.label(RichText::new(title).strong());
                                        if !detail.is_empty() {
                                            selectable_text(ui, &detail);
                                        }
                                        egui::CollapsingHeader::new("技术详情 / 原始记录").show(
                                            ui,
                                            |ui| {
                                                ui.label(&event.kind);
                                                let raw = call
                                                    .map(|c| {
                                                        serde_json::to_value(c).unwrap_or_default()
                                                    })
                                                    .unwrap_or_else(|| event.data.clone());
                                                selectable_text(
                                                    ui,
                                                    &serde_json::to_string_pretty(&raw)
                                                        .unwrap_or_default(),
                                                );
                                            },
                                        );
                                    });
                                });
                                ui.add_space(8.0);
                            }
                        });
                }
                View::Agent => {
                    if self.agent.is_none() {
                        self.select_agent(run.root_id.clone());
                    }

                    egui::SidePanel::left("agents")
                        .default_width(220.0)
                        .resizable(true)
                        .show_inside(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("agent-list")
                                .show(ui, |ui| {
                                    for agent in &self.agents {
                                        ui.horizontal(|ui| {
                                            ui.add_space((agent.depth.min(5) * 10) as f32);
                                            if ui
                                                .selectable_label(
                                                    self.agent.as_ref() == Some(&agent.id),
                                                    format!(
                                                        "{}\n{} · {}",
                                                        agent.name,
                                                        agent.role,
                                                        status_label(agent.status)
                                                    ),
                                                )
                                                .clicked()
                                            {
                                                self.selection_epoch += 1;
                                                self.agent = Some(agent.id.clone());
                                                self.conversation = None;
                                                self.chat_jump = true;
                                                self.chat_follow = true;
                                            }
                                        });
                                    }
                                });
                        });
                    ui.horizontal_wrapped(|ui| {
                        if let Some(agent) = self
                            .agents
                            .iter()
                            .find(|a| Some(&a.id) == self.agent.as_ref())
                        {
                            ui.label(RichText::new(&agent.name).strong());
                            ui.colored_label(
                                status_color(agent.status),
                                status_label(agent.status),
                            );
                        }
                        if ui.checkbox(&mut self.chat_follow, "跟随最新").changed()
                            && self.chat_follow
                        {
                            self.chat_jump = true;
                        }
                        if ui.button("↓ 回到最新").clicked() {
                            self.chat_follow = true;
                            self.chat_jump = true;
                        }
                        ui.checkbox(&mut self.chat_details, "技术详情");
                    });
                    ui.separator();
                    let jump = std::mem::take(&mut self.chat_jump);
                    crate::gui_chat::scroll(
                        ui,
                        ("conversation-v2", &self.agent),
                        &mut self.chat_follow,
                        jump,
                        |ui| {
                            ui.set_max_width(ui.available_width().min(960.0));
                            if let Some(agent) = self
                                .agents
                                .iter()
                                .find(|a| Some(&a.id) == self.agent.as_ref())
                            {
                                egui::CollapsingHeader::new("任务目标与状态").show(ui, |ui| {
                                    selectable_text(ui, &agent.task);
                                    ui.label(format!(
                                        "{} 轮 · {} tokens",
                                        agent.turns, agent.total_tokens
                                    ));
                                    if ui
                                        .add_enabled(
                                            !agent.status.terminal(),
                                            egui::Button::new("停止分支"),
                                        )
                                        .clicked()
                                    {
                                        self.confirm_stop = Some(StopTarget {
                                            root: self.store.root().to_owned(),
                                            run: run.id.clone(),
                                            agent: agent.id.clone(),
                                            name: agent.name.clone(),
                                        });
                                    }
                                });
                                if let Some(error) = &agent.error {
                                    ui.colored_label(Color32::LIGHT_RED, error);
                                }
                                if let Some(summary) = &agent.summary {
                                    selectable_text(ui, summary);
                                }
                                ui.add_space(10.0);
                                if let Some(conversation) = &self.conversation
                                    && conversation.id == agent.id
                                {
                                    crate::gui_chat::show(
                                        ui,
                                        conversation,
                                        &agent.name,
                                        &mut self.chat_layout,
                                        self.chat_details,
                                    );
                                } else {
                                    ui.spinner();
                                }
                            }
                        },
                    );
                }

                View::Findings => {
                    egui::ScrollArea::vertical()
                        .id_salt("findings")
                        .show(ui, |ui| {
                            if self.findings.is_empty() {
                                ui.add_space(32.0);
                                ui.heading("尚无分析发现");
                                ui.label("发现将随分析推进显示；未验证的结论会保留其实际阶段。");
                            }
                            for finding in &self.findings {
                                card().show(ui, |ui| {
                                    ui.heading(&finding.title);
                                    ui.colored_label(
                                        ACCENT,
                                        format!("{:?}  ·  {:?}", finding.stage, finding.severity),
                                    );
                                    selectable_text(ui, &finding.description);
                                    egui::CollapsingHeader::new("根因、影响与证据")
                                        .id_salt(&finding.id)
                                        .show(ui, |ui| {
                                            selectable_text(
                                                ui,
                                                &serde_json::to_string_pretty(finding)
                                                    .unwrap_or_default(),
                                            );
                                        });
                                });
                                ui.add_space(8.0);
                            }
                        });
                }
                View::Overview => {
                    egui::ScrollArea::vertical()
                        .id_salt("overview")
                        .show(ui, |ui| {
                            ui.heading("分析目标");
                            selectable_text(ui, &run.task);
                            ui.add_space(14.0);
                            ui.heading("任务标识");
                            selectable_text(ui, &run.id);
                            ui.label("SHA-256");
                            selectable_text(ui, &run.target_sha256);
                            ui.add_space(14.0);
                            ui.heading("分析总结");
                            selectable_text(
                                ui,
                                run.summary
                                    .as_deref()
                                    .unwrap_or("分析完成后将在这里显示总结。"),
                            );
                        });
                }
            }
        });
    }
    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(mut editor) = self.settings.take() {
            let outcome = editor.show(
                ctx,
                self.cancel.is_some() || self.exporting || self.commands.busy(),
                self.commands.contains(&Key::Settings),
                self.state_override.as_deref(),
            );
            if outcome.save {
                self.dispatch(Command::SaveSettings {
                    editor: Box::new(editor.clone()),
                    state_override: self.state_override.clone(),
                });
            }
            if let Some(path) = outcome.load {
                self.dispatch(Command::OpenSettings(path));
            }
            if outcome.open {
                self.settings = Some(editor);
            }
        }

        if self.new_task {
            let mut open = true;
            let mut launch = false;
            egui::Window::new("新建分析")
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_width(650.0)
                .show(ctx, |ui| {
                    ui.label(RichText::new("让证据引导分析").size(24.0).strong());
                    ui.label(
                        RichText::new("选择二进制样本，并描述你希望了解的行为或问题。")
                            .color(MUTED),
                    );
                    ui.add_space(14.0);
                    ui.label("目标文件");
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [ui.available_width() - 90.0, 30.0],
                            egui::TextEdit::singleline(&mut self.target)
                                .hint_text("文件路径，或拖入文件"),
                        );
                        if ui
                            .add_enabled(
                                !self.commands.contains(&Key::Target),
                                egui::Button::new("浏览…"),
                            )
                            .clicked()
                        {
                            self.dispatch(Command::Target);
                        }
                    });
                    ui.label("分析目标");
                    ui.add_sized(
                        [ui.available_width(), 125.0],
                        egui::TextEdit::multiline(&mut self.task),
                    );
                    ui.label(
                        RichText::new(format!(
                            "模型  {}    ·    预算  {} tokens",
                            self.config.model.model, self.config.budget.max_total_tokens
                        ))
                        .color(MUTED),
                    );
                    if self.cancel.is_some() {
                        ui.label("本窗口已有任务运行中，请先暂停并等待保存完成。");
                    }
                    launch = ui
                        .add_enabled(
                            self.cancel.is_none()
                                && !self.commands.busy()
                                && !self.target.trim().is_empty()
                                && !self.task.trim().is_empty(),
                            egui::Button::new(RichText::new("开始分析").color(Color32::BLACK))
                                .fill(ACCENT),
                        )
                        .clicked();
                });
            self.new_task = open;
            if launch {
                self.start(None);
            }
        }
        if let Some(mut run) = self.resume_run.take() {
            let mut open = true;
            let mut resume = false;
            egui::Window::new("恢复分析")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("沿用已保存的模型与后端配置，可在恢复前追加预算。");
                    ui.label(format!(
                        "已使用 {} tokens / ${:.4}",
                        run.total_tokens, run.cost_usd
                    ));
                    ui.horizontal(|ui| {
                        ui.label("Token 总预算");
                        ui.add(
                            egui::DragValue::new(&mut run.budget.max_total_tokens)
                                .range(1..=u64::MAX)
                                .speed(1000.0),
                        );
                    });
                    let mut limit_cost = run.budget.max_usd.is_some();
                    if ui.checkbox(&mut limit_cost, "限制费用（USD）").changed() {
                        run.budget.max_usd = limit_cost.then_some((run.cost_usd + 1.0).max(1.0));
                    }
                    if let Some(usd) = &mut run.budget.max_usd {
                        ui.add(
                            egui::DragValue::new(usd)
                                .range(0.01..=1_000_000.0)
                                .speed(0.1),
                        );
                    }
                    resume = ui
                        .add_enabled(
                            self.cancel.is_none() && !self.commands.busy(),
                            egui::Button::new("恢复分析"),
                        )
                        .clicked();
                });
            if resume {
                self.start(Some(run));
            } else if open {
                self.resume_run = Some(run);
            }
        }
        if let Some(target) = self.confirm_stop.clone() {
            egui::Window::new("停止 Agent 分支？")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("停止 {} 及其子任务？已有证据会保留。", target.name));
                    ui.label(
                        RichText::new(format!("任务 {} · Agent {}", target.run, target.agent))
                            .small(),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("确认停止").clicked() {
                            if target.root != self.store.root() {
                                self.error("工作区已切换，请重新选择停止对象。");
                            } else {
                                self.dispatch(Command::Stop(target.agent.clone()));
                            }
                            self.confirm_stop = None;
                        }
                        if ui.button("取消").clicked() {
                            self.confirm_stop = None;
                        }
                    });
                });
        }
        if self.confirm_close {
            egui::Window::new("暂停并关闭工作区？")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("本窗口启动的分析将暂停，保存后关闭。可在下次打开时恢复。");
                    if self.closing {
                        ui.spinner();
                        ui.label("正在等待分析保存，请稍候…");
                    } else {
                        ui.horizontal(|ui| {
                            if ui.button("暂停并关闭").clicked() {
                                if let Some(cancel) = &self.cancel {
                                    cancel.cancel();
                                }
                                self.closing = true;
                            }
                            if ui.button("继续分析").clicked() {
                                self.confirm_close = false;
                            }
                        });
                    }
                });
        }
    }
}

impl eframe::App for Workspace {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.commands.context = Some(ctx.clone());
        while let Ok(update) = self.rx.try_recv() {
            match update {
                Update::Loaded {
                    request,
                    reader,
                    result,
                } => self.loaded(request, *reader, result),
                Update::Started(id) => {
                    self.active_run = Some(id.clone());
                    self.select(id);
                    self.new_task = false;
                    self.notice = None;
                }
                Update::Finished(result) => {
                    self.cancel = None;
                    self.active_run = None;
                    match result {
                        Ok(()) => {
                            self.notice = Some((false, "分析已结束或暂停，进度已保存。".into()))
                        }
                        Err(e) => self.error(format!("{e:#}")),
                    }
                    if let Err(e) = self.refresh() {
                        self.error(e);
                    }
                }
                Update::CommandFinished { root, key, result } => {
                    self.commands.complete(&key);
                    if key == Key::Export {
                        self.exporting = false;
                    }
                    if root == self.store.root() {
                        match result {
                            Ok(outcome) => self.command_completed(outcome),
                            Err(error) => self.error(format!("{error:#}")),
                        }
                        let _ = self.refresh();
                    }
                }
            }
        }
        if self.closing && self.cancel.is_none() && !self.exporting && !self.commands.busy() {
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
            && (self.cancel.is_some() || self.exporting || self.commands.busy())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }
        if self.refreshed.elapsed() >= Duration::from_secs(1)
            && let Err(e) = self.refresh()
        {
            self.error(e);
            self.refreshed = Instant::now();
        }
        let dropped = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
        if let Some(path) = dropped {
            self.target = path.display().to_string();
            self.new_task = true;
        }
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if self.cancel.is_some() {
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
        if self.agent.as_deref() != self.conversation.as_ref().map(|agent| agent.id.as_str()) {
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
        app.agent = Some("a".into());
        app.message = "new draft".into();
        app.command_completed(Outcome::Sent {
            recipient: "a".into(),
            text: "old draft".into(),
        });
        assert_eq!(app.message, "new draft");
        app.agent = Some("b".into());
        app.command_completed(Outcome::Sent {
            recipient: "a".into(),
            text: "new draft".into(),
        });
        assert_eq!(app.message, "new draft");
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
        app.confirm_stop = Some(target);
        app.select_agent("agent-b".into());
        assert_eq!(app.confirm_stop.as_ref().unwrap().agent, "agent-a");
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
        app.agent = Some(run.root_id.clone());
        app.event_limit = 1_000;
        app.refresh()?;
        finish_refresh(&mut app)?;
        assert_eq!(app.agents.len(), 1);
        assert_eq!(app.events.len(), 1);
        assert_eq!(app.conversation.as_ref().unwrap().id, run.root_id);
        app.message = "unsent operator message".into();

        for in_flight in [false, true] {
            if in_flight {
                app.refresh()?;
            }
            app.select(run.id.clone());
            app.refresh()?;
            finish_refresh(&mut app)?;
            assert_eq!(app.agents.len(), 1);
            assert_eq!(app.agents[0].id, run.root_id);
            assert_eq!(app.events.len(), 1);
            assert_eq!(app.agent.as_deref(), Some(run.root_id.as_str()));
            assert_eq!(app.conversation.as_ref().unwrap().id, run.root_id);
            assert_eq!(app.event_limit, 1_000);
            assert_eq!(app.message, "unsent operator message");
        }

        let other = coordinator.create_run(
            &target,
            "another selection",
            config.budget.clone(),
            serde_json::to_value(&config)?,
        )?;
        app.select(other.id.clone());
        finish_refresh(&mut app)?;
        assert_eq!(app.selected.as_deref(), Some(other.id.as_str()));
        assert_eq!(app.agents.len(), 1);
        assert_eq!(app.agents[0].id, other.root_id);
        assert!(app.agent.is_none() && app.conversation.is_none());
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
        app.new_task = false;
        app.agent = Some(run.root_id);
        let checkpoint = serde_json::json!({
            "messages": [
                {"role":"user", "content":[{"type":"text", "text":"internal context"}]},
                {"role":"assistant", "content":[{"type":"text", "text":"已确认文件格式，接下来检查输入处理。"}]}
            ],
            "asides": [], "meta": [{"id": 0}, {"id": 1}], "next_id": 2, "next_aside": 0
        });
        store.checkpoint_and_ack(app.agent.as_deref().unwrap(), checkpoint, &[])?;
        let mut reader = gui_data::Reader::default();
        let delta = reader.read(&store, app.selected.as_deref(), app.agent.as_deref(), 500)?;
        app.apply_delta(delta);
        assert_eq!(
            app.agents[0].public_replies,
            vec!["已确认文件格式，接下来检查输入处理。"]
        );

        for view in [View::Activity, View::Agent, View::Findings, View::Overview] {
            app.view = view;
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
        app.target = dir.path().join("missing-sample").display().to_string();
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
