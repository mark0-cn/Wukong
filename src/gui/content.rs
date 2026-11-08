use super::*;

impl Workspace {
    pub(super) fn content(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(12.0);
            let run = self
                .data
                .runs
                .iter()
                .find(|r| Some(&r.id) == self.selection.selected.as_ref())
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
                        self.dialogs.task = TaskDialog::New;
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
                            !self.commands.contains(&Key::Export),
                            egui::Button::new(if self.commands.contains(&Key::Export) {
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
                                && !self.execution.busy()
                                && !self.commands.busy(),
                            egui::Button::new("恢复"),
                        )
                        .clicked()
                    {
                        self.dialogs.task = TaskDialog::Resume(Box::new(run.clone()));
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
                ui.label(format!("{} 个 Agent", self.data.agents.len()));
                ui.separator();
                ui.label(format!("{} 项发现", self.data.findings.len()));
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
                ui.selectable_value(&mut self.selection.view, View::Activity, "实时活动");
                ui.selectable_value(&mut self.selection.view, View::Agent, "Agents / 会话");
                ui.selectable_value(&mut self.selection.view, View::Findings, "分析发现");
                ui.selectable_value(&mut self.selection.view, View::Overview, "任务概览");
            });
            ui.separator();
            egui::TopBottomPanel::bottom("composer")
                .frame(egui::Frame::new().fill(SURFACE).inner_margin(14))
                .show_inside(ui, |ui| {
                    let recipient = self
                        .data
                        .agents
                        .iter()
                        .find(|a| Some(&a.id) == self.selection.agent.as_ref())
                        .map(|a| a.name.as_str())
                        .unwrap_or("Root Agent");
                    ui.horizontal(|ui| {
                        ui.label(format!("发送给 {recipient}"));
                        ui.checkbox(&mut self.composer.follow_up, "作为后续任务")
                            .on_hover_text("重新激活已完成的 Agent；暂停的分析仍需点击恢复。");
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [ui.available_width() - 88.0, 54.0],
                            egui::TextEdit::multiline(&mut self.composer.message)
                                .hint_text("补充约束、提出问题，或指定下一步分析…"),
                        );
                        if ui
                            .add_enabled(
                                !self.composer.message.trim().is_empty()
                                    && !self.commands.contains(&Key::Message(
                                        self.selection
                                            .agent
                                            .clone()
                                            .unwrap_or_else(|| run.root_id.clone()),
                                    )),
                                egui::Button::new("发送"),
                            )
                            .clicked()
                        {
                            self.send();
                        }
                    });
                });
            match self.selection.view {
                View::Activity => self.activity_view(ui, &run),
                View::Agent => {
                    if self.selection.agent.is_none() {
                        self.select_agent(run.root_id.clone());
                    }

                    egui::SidePanel::left("agents")
                        .default_width(220.0)
                        .resizable(true)
                        .show_inside(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("agent-list")
                                .show(ui, |ui| {
                                    for agent in &self.data.agents {
                                        ui.horizontal(|ui| {
                                            ui.add_space((agent.depth.min(5) * 10) as f32);
                                            if ui
                                                .selectable_label(
                                                    self.selection.agent.as_ref()
                                                        == Some(&agent.id),
                                                    format!(
                                                        "{}\n{} · {}",
                                                        agent.name,
                                                        agent.role,
                                                        status_label(agent.status)
                                                    ),
                                                )
                                                .clicked()
                                            {
                                                self.selection.selection_epoch += 1;
                                                self.selection.agent = Some(agent.id.clone());
                                                self.data.conversation = None;
                                                self.chat.chat_jump = true;
                                                self.chat.chat_follow = true;
                                            }
                                        });
                                    }
                                });
                        });
                    ui.horizontal_wrapped(|ui| {
                        if let Some(agent) = self
                            .data
                            .agents
                            .iter()
                            .find(|a| Some(&a.id) == self.selection.agent.as_ref())
                        {
                            ui.label(RichText::new(&agent.name).strong());
                            ui.colored_label(
                                status_color(agent.status),
                                status_label(agent.status),
                            );
                        }
                        if ui
                            .checkbox(&mut self.chat.chat_follow, "跟随最新")
                            .changed()
                            && self.chat.chat_follow
                        {
                            self.chat.chat_jump = true;
                        }
                        if ui.button("↓ 回到最新").clicked() {
                            self.chat.chat_follow = true;
                            self.chat.chat_jump = true;
                        }
                        ui.checkbox(&mut self.chat.chat_details, "技术详情");
                    });
                    ui.separator();
                    let jump = std::mem::take(&mut self.chat.chat_jump);
                    crate::gui_chat::scroll(
                        ui,
                        ("conversation-v2", &self.selection.agent),
                        &mut self.chat.chat_follow,
                        jump,
                        |ui| {
                            ui.set_max_width(ui.available_width().min(960.0));
                            if let Some(agent) = self
                                .data
                                .agents
                                .iter()
                                .find(|a| Some(&a.id) == self.selection.agent.as_ref())
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
                                        self.dialogs.stop = Some(StopTarget {
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
                                if let Some(conversation) = &self.data.conversation
                                    && conversation.id == agent.id
                                {
                                    crate::gui_chat::show(
                                        ui,
                                        conversation,
                                        &agent.name,
                                        &mut self.chat.chat_layout,
                                        self.chat.chat_details,
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
                            if self.data.findings.is_empty() {
                                ui.add_space(32.0);
                                ui.heading("尚无分析发现");
                                ui.label("发现将随分析推进显示；未验证的结论会保留其实际阶段。");
                            }
                            for finding in &self.data.findings {
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
}
