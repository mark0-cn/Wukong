use super::*;

impl Workspace {
    pub(super) fn new(config: AppConfig, store: Store, selected: Option<String>) -> Self {
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
            data: WorkspaceData::default(),
            activity: activity::Activity::default(),
            last_loaded: None,
            selection: Selection {
                selected,
                ..Selection::default()
            },
            composer: Composer::default(),
            chat: ChatView::default(),
            draft: TaskDraft::default(),
            dialogs: Dialogs::default(),
            execution: Execution::Idle,
            technical_events: false,
            follow: true,
            notice: None,
            refreshed: Instant::now(),
            reader: Some(gui_data::Reader::default()),
        };
        if let Err(e) = app.refresh() {
            app.error(e);
        }
        if app.data.runs.is_empty() {
            app.dialogs.task = TaskDialog::New;
        }
        app
    }
    pub(super) fn error(&mut self, error: impl std::fmt::Display) {
        self.notice = Some((true, error.to_string()));
    }
    pub(super) fn refresh(&mut self) -> Result<()> {
        let Some(mut reader) = self.reader.take() else {
            return Ok(());
        };
        let store = self.store.clone();
        self.selection.request_serial += 1;
        let request = self.read_request();
        let tx = self.tx.clone();
        self.refreshed = Instant::now();
        let context = self.commands.context.clone();
        let work_request = request.clone();
        let work = self.runtime.spawn_blocking(move || {
            let result = reader
                .read(
                    &store,
                    work_request.run.as_deref(),
                    work_request.agent.as_deref(),
                    work_request.event_limit,
                )
                .map(Box::new);
            (reader, result)
        });
        self.runtime.spawn(async move {
            let (reader, result) = work
                .await
                .unwrap_or_else(|error| (gui_data::Reader::default(), Err(error.into())));
            let _ = tx.send(Update::Loaded {
                request,
                reader: Box::new(reader),
                result,
            });
            if let Some(context) = context {
                context.request_repaint();
            }
        });
        Ok(())
    }
    pub(super) fn refresh_interval(&self) -> Duration {
        if self.execution.busy() || self.commands.busy() {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(3)
        }
    }
    pub(super) fn read_request(&self) -> ReadRequest {
        ReadRequest {
            root: self.store.root().to_owned(),
            run: self.selection.selected.clone(),
            agent: self.selection.agent.clone(),
            epoch: self.selection.selection_epoch,
            serial: self.selection.request_serial,
            event_limit: self.selection.event_limit,
        }
    }
    pub(super) fn loaded(
        &mut self,
        request: ReadRequest,
        reader: gui_data::Reader,
        result: Result<Box<gui_data::Delta>>,
    ) {
        self.reader = Some(reader);
        if request == self.read_request() {
            match result {
                Ok(delta) => {
                    self.apply_delta(*delta);
                    let mut request = request;
                    request.run = self.selection.selected.clone();
                    self.last_loaded = Some(request);
                }
                Err(error) => {
                    self.last_loaded = Some(request);
                    self.error(error);
                }
            }
        } else {
            self.reader = Some(gui_data::Reader::default());
            let _ = self.refresh();
        }
    }
    pub(super) fn select_agent(&mut self, id: String) {
        if self.selection.agent.as_ref() != Some(&id) {
            self.selection.selection_epoch += 1;
            self.selection.agent = Some(id);
            self.data.conversation = None;
            self.chat.chat_jump = true;
            self.chat.chat_follow = true;
        }
    }
    pub(super) fn apply_delta(&mut self, mut delta: gui_data::Delta) {
        let activity_changed = delta.reset
            || !delta.agents.is_empty()
            || !delta.notes.is_empty()
            || delta.events.is_some()
            || delta.calls.is_some();
        let was_empty = self.data.runs.is_empty();
        gui_data::apply(&mut self.data.runs, delta.runs, |run| &run.id);
        self.data
            .runs
            .sort_by_key(|run| std::cmp::Reverse(run.created_at));
        if delta.reset {
            self.data.agents.clear();
            self.data.notes.clear();
            self.data.findings.clear();
            self.data.events.clear();
            self.data.calls.clear();
            self.data.conversation = None;
        }
        self.selection.selected = delta.selected;
        for change in &mut delta.agents {
            if let Some(agent) = change.value.as_mut()
                && agent.public_replies.is_empty()
                && let Some(previous) = self.data.agents.iter().find(|previous| {
                    previous.id == agent.id && previous.generation == agent.generation
                })
            {
                agent.public_replies = previous.public_replies.clone();
            }
        }
        gui_data::apply(&mut self.data.agents, delta.agents, |agent| &agent.id);
        self.data
            .agents
            .sort_by_key(|agent| (agent.depth, agent.created_at));
        gui_data::apply(&mut self.data.notes, delta.notes, |note| &note.id);
        gui_data::apply(&mut self.data.findings, delta.findings, |finding| {
            &finding.id
        });
        if let Some(events) = delta.events {
            self.data.events = events;
        }
        if let Some(calls) = delta.calls {
            self.data.calls = calls;
        }
        if let Some(conversation) = delta.conversation {
            self.data.conversation = conversation;
        }
        if activity_changed {
            self.activity.rebuild(&self.data);
        }
        if was_empty && !self.data.runs.is_empty() {
            self.dialogs.task = TaskDialog::None;
        }
    }
    pub(super) fn select(&mut self, id: String) {
        if self.selection.selected.as_deref() == Some(id.as_str()) {
            return;
        }
        self.selection.selection_epoch += 1;
        self.selection.selected = Some(id);
        self.selection.agent = None;
        self.data.conversation = None;
        self.data.events.clear();
        self.data.calls.clear();
        self.data.agents.clear();
        self.data.notes.clear();
        self.data.findings.clear();
        self.activity.rebuild(&self.data);
        self.selection.event_limit = 500;
        self.composer.message.clear();
        if let Err(e) = self.refresh() {
            self.error(e);
        }
    }
    pub(super) fn start(&mut self, resume: Option<RunSummary>) {
        if self.execution.busy() || self.commands.busy() {
            return;
        }
        let config = self.config.clone();
        let store = self.store.clone();
        let target = self.draft.target.trim().trim_matches('"').to_owned();
        let task = self.draft.task.trim().to_owned();
        if resume.is_none() && (target.is_empty() || task.is_empty()) {
            self.error("请选择样本，并填写分析目标。");
            return;
        }
        let cancel = CancellationToken::new();
        self.execution = Execution::Starting(cancel.clone());
        let tx = self.tx.clone();
        self.notice = Some((false, "正在连接分析引擎…".into()));
        let context = self.commands.context.clone();
        self.runtime.spawn(async move {
            let result = async {
                let service = AssessmentService::new(config, store);
                let prepared = if let Some(summary) = resume {
                    service.resume(&summary.id, Some(summary.budget)).await?
                } else {
                    service.start(std::path::Path::new(&target), &task).await?
                };
                let _ = tx.send(Update::Started(prepared.run().id.clone()));
                if let Some(context) = &context {
                    context.request_repaint();
                }
                let run = prepared.drive(cancel).await?;
                ensure!(
                    run.status != Status::Failed,
                    "分析失败，请查看 Agent 错误和活动记录。"
                );
                Ok(())
            }
            .await;
            let _ = tx.send(Update::Finished(result));
            if let Some(context) = context {
                context.request_repaint();
            }
        });
    }
    pub(super) fn dispatch(&mut self, command: Command) {
        self.commands
            .submit(command, self.config.clone(), self.store.clone());
    }
    pub(super) fn pause(&mut self) {
        if let Some(id) = self.selection.selected.clone() {
            self.dispatch(Command::Pause(id));
        }
    }
    pub(super) fn export(&mut self) {
        if let Some(id) = self.selection.selected.clone() {
            self.dispatch(Command::Export(id));
        }
    }
    pub(super) fn send(&mut self) {
        let recipient = self.selection.agent.clone().or_else(|| {
            self.data
                .runs
                .iter()
                .find(|r| Some(&r.id) == self.selection.selected.as_ref())
                .map(|r| r.root_id.clone())
        });
        if let Some(recipient) = recipient {
            let options = MessageOptions {
                kind: if self.composer.follow_up {
                    MessageKind::FollowUp
                } else {
                    MessageKind::Information
                },
                ..Default::default()
            };
            self.dispatch(Command::Message {
                recipient,
                text: self.composer.message.trim().to_owned(),
                options,
            });
        }
    }
    pub(super) fn command_completed(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Paused(id) => {
                if self.execution.run() == Some(&id)
                    && let Some(cancel) = self.execution.cancel()
                {
                    cancel.cancel();
                }
                self.notice = Some((false, "已请求暂停，正在保存进行中的工作。".into()));
            }
            Outcome::Stopped => self.notice = Some((false, "已停止指定 Agent 分支。".into())),
            Outcome::Sent { recipient, text } => {
                let current = self.selection.agent.clone().or_else(|| {
                    self.data
                        .runs
                        .iter()
                        .find(|r| Some(&r.id) == self.selection.selected.as_ref())
                        .map(|r| r.root_id.clone())
                });
                if current.as_ref() == Some(&recipient) && self.composer.message.trim() == text {
                    self.composer.message.clear();
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
                    self.draft.target = path.display().to_string();
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
                    self.selection.selection_epoch += 1;
                    self.dialogs.stop = None;
                    self.selection.selected = None;
                    self.selection.agent = None;
                    self.data.runs.clear();
                    self.data.conversation = None;
                    self.data.agents.clear();
                    self.data.events.clear();
                    self.data.calls.clear();
                    self.data.notes.clear();
                    self.data.findings.clear();
                    self.composer.message.clear();
                    self.dialogs.task = TaskDialog::None;
                }
                self.notice = Some((
                    false,
                    "配置已保存；新建分析使用新设置，恢复任务沿用历史配置。".into(),
                ));
            }
        }
    }
    pub(super) fn receive(&mut self, update: Update) {
        match update {
            Update::Loaded {
                request,
                reader,
                result,
            } => self.loaded(request, *reader, result),
            Update::Started(id) => {
                self.execution.started(id.clone());
                self.select(id);
                self.dialogs.task = TaskDialog::None;
                self.notice = None;
            }
            Update::Finished(result) => {
                self.execution = Execution::Idle;
                match result {
                    Ok(()) => self.notice = Some((false, "分析已结束或暂停，进度已保存。".into())),
                    Err(e) => self.error(format!("{e:#}")),
                }
                if let Err(e) = self.refresh() {
                    self.error(e);
                }
            }
            Update::CommandFinished { root, key, result } => {
                self.commands.complete(&key);
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
}
