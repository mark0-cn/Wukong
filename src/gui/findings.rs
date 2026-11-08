//! Findings are reviewed through domain fields and scoped, bounded evidence previews.
use super::*;
use crate::queries::{EvidencePreview, FindingDetails};
#[derive(Default)]
pub(super) struct FindingsView {
    selected: Option<String>,
    query: String,
    stage: Option<FindingStage>,
    severity: Option<Severity>,
    revision: i64,
    loaded: Option<(String, i64)>,
    details: Option<Box<FindingDetails>>,
    error: Option<String>,
    preview: Option<EvidencePreview>,
    preview_target: Option<(String, u64)>,
}
impl FindingsView {
    pub fn revision(&mut self, revision: i64) {
        self.revision = revision;
    }
    pub fn loaded(&mut self, id: String, revision: i64, details: Box<FindingDetails>) {
        if self.selected.as_ref() == Some(&id) {
            self.loaded = Some((id, revision));
            self.details = Some(details);
            self.error = None;
        }
    }
    pub fn previewed(&mut self, preview: EvidencePreview) {
        if self.preview_target.as_ref() == Some(&(preview.evidence.id.clone(), preview.offset)) {
            self.preview = Some(preview);
        }
    }
    pub fn failed(&mut self, error: String) {
        self.error = Some(error);
        self.loaded = self.selected.clone().map(|id| (id, self.revision));
    }
}
fn stage_label(stage: FindingStage) -> &'static str {
    match stage {
        FindingStage::Suspected => "疑似",
        FindingStage::Reproduced => "已复现",
        FindingStage::RootCauseConfirmed => "根因已确认",
        FindingStage::ImpactVerified => "影响已验证",
        FindingStage::Inconclusive => "未能定论",
        FindingStage::Rejected => "已排除",
    }
}
fn severity_label(severity: Option<Severity>) -> &'static str {
    match severity {
        Some(Severity::Critical) => "严重",
        Some(Severity::High) => "高",
        Some(Severity::Medium) => "中",
        Some(Severity::Low) => "低",
        Some(Severity::Info) => "信息",
        None => "未评级",
    }
}
fn section(ui: &mut egui::Ui, title: &str, text: Option<&str>) {
    ui.heading(title);
    crate::gui_markdown::show(ui, text.unwrap_or("尚未提供"));
    ui.add_space(10.0);
}
fn preview_text(preview: &EvidencePreview) -> String {
    if !preview.bytes.contains(&0)
        && let Ok(text) = std::str::from_utf8(&preview.bytes)
    {
        return text.into();
    }
    preview
        .bytes
        .chunks(16)
        .enumerate()
        .map(|(index, row)| {
            format!(
                "{:08x}  {}  {}",
                preview.offset + index as u64 * 16,
                row.iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                row.iter()
                    .map(|byte| if byte.is_ascii_graphic() || *byte == b' ' {
                        *byte as char
                    } else {
                        '.'
                    })
                    .collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
impl Workspace {
    pub(super) fn findings_view(&mut self, ui: &mut egui::Ui, run: &RunSummary) {
        let view = &mut self.findings_view;
        let mut selected = view.selected.clone();
        egui::SidePanel::left("finding-list")
            .default_width(260.0)
            .resizable(true)
            .show_inside(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut view.query).hint_text("搜索发现"));
                egui::ComboBox::from_id_salt("finding-stage")
                    .selected_text(view.stage.map(stage_label).unwrap_or("全部阶段"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut view.stage, None, "全部阶段");
                        for stage in [
                            FindingStage::Suspected,
                            FindingStage::Reproduced,
                            FindingStage::RootCauseConfirmed,
                            FindingStage::ImpactVerified,
                            FindingStage::Inconclusive,
                            FindingStage::Rejected,
                        ] {
                            ui.selectable_value(&mut view.stage, Some(stage), stage_label(stage));
                        }
                    });
                egui::ComboBox::from_id_salt("finding-severity")
                    .selected_text(
                        view.severity
                            .map(|s| severity_label(Some(s)))
                            .unwrap_or("全部等级"),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut view.severity, None, "全部等级");
                        for severity in [
                            Severity::Critical,
                            Severity::High,
                            Severity::Medium,
                            Severity::Low,
                            Severity::Info,
                        ] {
                            ui.selectable_value(
                                &mut view.severity,
                                Some(severity),
                                severity_label(Some(severity)),
                            );
                        }
                    });
                let query = view.query.to_lowercase();
                let visible = self
                    .data
                    .findings
                    .iter()
                    .filter(|finding| {
                        view.stage.is_none_or(|stage| finding.stage == stage)
                            && view
                                .severity
                                .is_none_or(|severity| finding.severity == Some(severity))
                            && format!("{} {}", finding.title, finding.description)
                                .to_lowercase()
                                .contains(&query)
                    })
                    .collect::<Vec<_>>();
                if !visible
                    .iter()
                    .any(|finding| Some(&finding.id) == selected.as_ref())
                {
                    selected = visible.first().map(|finding| finding.id.clone());
                }
                egui::ScrollArea::vertical()
                    .id_salt(("findings", &run.id))
                    .show(ui, |ui| {
                        if visible.is_empty() {
                            ui.label("暂无匹配的分析发现");
                        }
                        for finding in visible {
                            if ui
                                .selectable_label(
                                    selected.as_ref() == Some(&finding.id),
                                    format!(
                                        "{}\n{} · {}",
                                        finding.title,
                                        stage_label(finding.stage),
                                        severity_label(finding.severity)
                                    ),
                                )
                                .clicked()
                            {
                                selected = Some(finding.id.clone());
                            }
                        }
                    });
            });
        if selected != view.selected {
            view.selected = selected;
            view.details = None;
            view.loaded = None;
            view.error = None;
            view.preview = None;
            view.preview_target = None;
        }
        let mut command = None;
        let mut agent = None;
        if let Some(id) = &view.selected
            && view.loaded.as_ref() != Some(&(id.clone(), view.revision))
            && !self.commands.contains(&Key::Finding)
        {
            command = Some(Command::Finding {
                run: run.id.clone(),
                id: id.clone(),
                revision: view.revision,
            });
        }
        egui::ScrollArea::vertical()
            .id_salt(("finding-details", &run.id, &view.selected))
            .show(ui, |ui| {
                if let Some(error) = &view.error {
                    ui.colored_label(Color32::LIGHT_RED, error);
                    if ui.button("重试").clicked() {
                        view.loaded = None;
                    }
                }
                let Some(details) = &view.details else {
                    if view.selected.is_some() && view.error.is_none() {
                        ui.spinner();
                        ui.label("读取发现与证据…");
                    } else if view.selected.is_none() {
                        ui.heading("尚无分析发现");
                        ui.label("发现将随分析推进显示，并保留实际验证阶段。");
                    }
                    return;
                };
                let finding = &details.finding;
                ui.heading(&finding.title);
                ui.label(format!(
                    "{} · {}",
                    stage_label(finding.stage),
                    severity_label(finding.severity)
                ));
                if ui.button("查看来源 Agent").clicked() {
                    agent = Some(finding.agent_id.clone());
                }
                section(ui, "发现描述", Some(&finding.description));
                section(ui, "根因", finding.root_cause.as_deref());
                section(ui, "影响", finding.impact.as_deref());
                if let Some(rationale) = &finding.severity_rationale {
                    section(ui, "评级依据", Some(rationale));
                }
                if let Some(location) = &finding.location {
                    ui.heading("位置");
                    crate::gui_markdown::code(
                        ui,
                        "json",
                        &serde_json::to_string_pretty(location).unwrap_or_default(),
                    );
                }
                ui.heading("验证结果");
                if let Some(verification) = &details.verification {
                    ui.colored_label(
                        if verification.passed {
                            ACCENT
                        } else {
                            Color32::LIGHT_RED
                        },
                        if verification.passed {
                            "验证通过"
                        } else {
                            "验证未通过"
                        },
                    );
                    ui.label(format!(
                        "{} 次尝试 / {} 次成功",
                        verification.attempts, verification.successes
                    ));
                    section(ui, "验证影响", Some(&verification.impact));
                    for (title, value) in [
                        ("环境", &verification.environment),
                        ("触发条件", &verification.conditions),
                        ("结果", &verification.result),
                    ] {
                        egui::CollapsingHeader::new(title)
                            .default_open(title != "环境")
                            .show(ui, |ui| {
                                crate::gui_markdown::code(
                                    ui,
                                    "json",
                                    &serde_json::to_string_pretty(value).unwrap_or_default(),
                                );
                            });
                    }
                    if ui.button("查看验证 Agent").clicked() {
                        agent = Some(verification.verifier_agent_id.clone());
                    }
                } else {
                    ui.label(if finding.verification_id.is_some() {
                        "引用的验证记录不存在"
                    } else {
                        "尚无独立验证记录"
                    });
                }
                ui.heading("关联证据");
                if details.evidence.is_empty() {
                    ui.label("尚无关联证据");
                }
                for missing in &details.missing_evidence {
                    ui.colored_label(Color32::LIGHT_RED, format!("证据缺失：{missing}"));
                }
                for evidence in &details.evidence {
                    ui.push_id(&evidence.id, |ui| {
                        ui.label(format!(
                            "{} · {} · {} bytes",
                            evidence.source, evidence.media_type, evidence.size
                        ));
                        if ui
                            .add_enabled(
                                !self.commands.contains(&Key::Evidence),
                                egui::Button::new("预览证据"),
                            )
                            .clicked()
                        {
                            view.preview_target = Some((evidence.id.clone(), 0));
                            view.preview = None;
                            command = Some(Command::Evidence {
                                run: run.id.clone(),
                                id: evidence.id.clone(),
                                offset: 0,
                            });
                        }
                        ui.label(RichText::new(format!("SHA-256 {}", evidence.sha256)).small());
                    });
                }
                if let Some(preview) = &view.preview {
                    ui.separator();
                    ui.heading(format!("证据预览 · {}", preview.evidence.source));
                    let end = preview.offset + preview.bytes.len() as u64;
                    ui.label(format!(
                        "字节 {}–{} / {}",
                        preview.offset, end, preview.evidence.size
                    ));
                    crate::gui_markdown::code(ui, "", &preview_text(preview));
                    ui.horizontal(|ui| {
                        for (enabled, label, offset) in [
                            (
                                preview.offset > 0,
                                "上一页",
                                preview.offset.saturating_sub(16 * 1024),
                            ),
                            (end < preview.evidence.size, "下一页", end),
                        ] {
                            if ui
                                .add_enabled(
                                    enabled && !self.commands.contains(&Key::Evidence),
                                    egui::Button::new(label),
                                )
                                .clicked()
                            {
                                view.preview_target = Some((preview.evidence.id.clone(), offset));
                                command = Some(Command::Evidence {
                                    run: run.id.clone(),
                                    id: preview.evidence.id.clone(),
                                    offset,
                                });
                            }
                        }
                    });
                }
                if self.commands.contains(&Key::Evidence) {
                    ui.spinner();
                }
                ui.heading("来源工具调用");
                for call in &details.calls {
                    egui::CollapsingHeader::new(format!(
                        "{} · {:?}",
                        crate::gui_activity::tool_name(&call.name),
                        call.state
                    ))
                    .id_salt(&call.id)
                    .show(ui, |ui| {
                        if ui.button("查看执行 Agent").clicked() {
                            agent = Some(call.agent_id.clone());
                        }
                        crate::gui_markdown::code(
                            ui,
                            "json",
                            &serde_json::to_string_pretty(call).unwrap_or_default(),
                        );
                    });
                }
            });
        if let Some(command) = command {
            self.dispatch(command);
        }
        if let Some(id) = agent {
            self.select_agent(id);
            self.selection.view = View::Agent;
        }
    }
}
