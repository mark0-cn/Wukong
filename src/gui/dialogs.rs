use super::*;

impl Workspace {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(mut editor) = self.settings.take() {
            let outcome = editor.show(
                ctx,
                self.execution.busy()
                    || self.commands.contains(&Key::Export)
                    || self.commands.busy(),
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
            self.settings = Some(editor);
        }

        if matches!(self.dialogs.task, TaskDialog::New) {
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
                            egui::TextEdit::singleline(&mut self.draft.target)
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
                        egui::TextEdit::multiline(&mut self.draft.task),
                    );
                    ui.label(
                        RichText::new(format!(
                            "模型  {}    ·    预算  {} tokens",
                            self.config.model.model, self.config.budget.max_total_tokens
                        ))
                        .color(MUTED),
                    );
                    if self.execution.busy() {
                        ui.label("本窗口已有任务运行中，请先暂停并等待保存完成。");
                    }
                    launch = ui
                        .add_enabled(
                            !self.execution.busy()
                                && !self.commands.busy()
                                && !self.draft.target.trim().is_empty()
                                && !self.draft.task.trim().is_empty(),
                            egui::Button::new(RichText::new("开始分析").color(Color32::BLACK))
                                .fill(ACCENT),
                        )
                        .clicked();
                });
            if !open {
                self.dialogs.task = TaskDialog::None;
            }
            if launch {
                self.start(None);
            }
        }
        if matches!(self.dialogs.task, TaskDialog::Resume(_))
            && let TaskDialog::Resume(mut run) = std::mem::take(&mut self.dialogs.task)
        {
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
                            !self.execution.busy() && !self.commands.busy(),
                            egui::Button::new("恢复分析"),
                        )
                        .clicked();
                });
            if resume {
                self.start(Some(*run));
            } else if open {
                self.dialogs.task = TaskDialog::Resume(run);
            }
        }
        if let Some(target) = self.dialogs.stop.clone() {
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
                            self.dialogs.stop = None;
                        }
                        if ui.button("取消").clicked() {
                            self.dialogs.stop = None;
                        }
                    });
                });
        }
        if self.dialogs.close != CloseState::Open {
            egui::Window::new("暂停并关闭工作区？")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("本窗口启动的分析将暂停，保存后关闭。可在下次打开时恢复。");
                    if self.dialogs.close == CloseState::Waiting {
                        ui.spinner();
                        ui.label("正在等待分析保存，请稍候…");
                    } else {
                        ui.horizontal(|ui| {
                            if ui.button("暂停并关闭").clicked() {
                                if let Some(cancel) = self.execution.cancel() {
                                    cancel.cancel();
                                }
                                self.dialogs.close = CloseState::Waiting;
                            }
                            if ui.button("继续分析").clicked() {
                                self.dialogs.close = CloseState::Open;
                            }
                        });
                    }
                });
        }
    }
}
