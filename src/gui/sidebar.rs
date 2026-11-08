use super::*;

impl Workspace {
    pub(super) fn sidebar(&mut self, ctx: &egui::Context) {
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
                    self.dialogs.task = TaskDialog::New;
                }
                if ui
                    .add_sized(
                        [ui.available_width(), 36.0],
                        egui::Button::new("设置 / 配置文件"),
                    )
                    .clicked()
                {
                    self.dispatch(Command::OpenSettings(Some(self.config_path.clone())));
                    self.dialogs.task = TaskDialog::None;
                }
                ui.add_space(18.0);
                ui.label(RichText::new("分析任务").color(MUTED));
                ui.add(
                    egui::TextEdit::singleline(&mut self.selection.filter)
                        .hint_text("搜索样本 / 任务"),
                );
                ui.separator();
                let mut selected = None;
                let filter = self.selection.filter.to_lowercase();
                let visible: Vec<_> = self
                    .data
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
                                    egui::Button::new(label).selected(
                                        self.selection.selected.as_ref() == Some(&run.id),
                                    ),
                                )
                                .on_hover_text(&run.id)
                                .clicked()
                            {
                                selected = Some(run.id.clone());
                            }
                        }
                        if self.data.runs.is_empty() {
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
}
