//! Activity display rows are prepared on projection changes, not every frame.
use super::*;
use std::collections::HashMap;
#[derive(Default)]
pub(super) struct Activity {
    rows: Vec<Row>,
    visible: Vec<usize>,
    keys: Vec<u64>,
    technical: bool,
    revision: u64,
    layout: super::virtual_list::VirtualList,
}
struct Row {
    id: String,
    author: String,
    title: String,
    detail: String,
    raw: String,
    technical: bool,
    agent: Option<String>,
}
impl Activity {
    pub fn rebuild(&mut self, data: &WorkspaceData) {
        let names: HashMap<_, _> = data
            .agents
            .iter()
            .map(|agent| (agent.id.as_str(), agent.name.as_str()))
            .collect();
        let calls: HashMap<_, _> = data
            .calls
            .iter()
            .map(|call| (call.id.as_str(), call))
            .collect();
        self.rows.clear();
        for agent in &data.agents {
            if !agent.public_replies.is_empty() {
                self.rows.push(Row {
                    id: format!("replies-{}", agent.id),
                    author: agent.name.clone(),
                    title: "公开回复".into(),
                    detail: agent.public_replies.join("\n\n"),
                    raw: String::new(),
                    technical: false,
                    agent: Some(agent.id.clone()),
                });
            }
        }
        for note in &data.notes {
            self.rows.push(Row {
                id: format!("note-{}", note.id),
                author: names
                    .get(note.agent_id.as_str())
                    .copied()
                    .unwrap_or("Agent")
                    .into(),
                title: note.title.clone(),
                detail: note.body.clone(),
                raw: String::new(),
                technical: false,
                agent: Some(note.agent_id.clone()),
            });
        }
        for event in &data.events {
            let call = event.data["call_id"]
                .as_str()
                .and_then(|id| calls.get(id))
                .copied();
            let (title, detail) = crate::gui_activity::describe(event, call);
            self.rows.push(Row {
                id: format!("event-{}", event.seq),
                author: format!(
                    "{} · {}",
                    event
                        .agent_id
                        .as_deref()
                        .and_then(|id| names.get(id))
                        .copied()
                        .unwrap_or("工作区"),
                    event
                        .created_at
                        .with_timezone(&chrono::Local)
                        .format("%H:%M:%S")
                ),
                title,
                detail,
                raw: call
                    .map(serde_json::to_string_pretty)
                    .unwrap_or_else(|| serde_json::to_string_pretty(&event.data))
                    .unwrap_or_default(),
                technical: crate::gui_activity::technical(event),
                agent: event.agent_id.clone(),
            });
        }
        self.filter(self.technical);
    }
    fn filter(&mut self, technical: bool) {
        self.technical = technical;
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| technical || !row.technical)
            .map(|(i, _)| i)
            .collect();
        use std::hash::{Hash, Hasher};
        self.keys = self
            .visible
            .iter()
            .map(|index| {
                let row = &self.rows[*index];
                let mut hash = std::collections::hash_map::DefaultHasher::new();
                (&row.id, &row.author, &row.title, &row.detail, &row.raw).hash(&mut hash);
                hash.finish()
            })
            .collect();
        self.revision += 1;
    }
    pub fn show(&mut self, ui: &mut egui::Ui, technical: bool) -> Option<String> {
        if technical != self.technical {
            self.filter(technical);
        }
        if self.visible.is_empty() {
            ui.label("等待 Agent 开始分析…");
            return None;
        }
        let mut selected = None;
        let rows = &self.rows;
        let visible = &self.visible;
        self.layout
            .show(ui, self.revision, &self.keys, |ui, index| {
                let row = &rows[visible[index]];
                ui.push_id(&row.id, |ui| {
                    card().show(ui, |ui| {
                        ui.colored_label(ACCENT, &row.author);
                        ui.label(RichText::new(&row.title).strong());
                        crate::gui_markdown::show(ui, &row.detail);
                        if let Some(agent) = &row.agent
                            && ui.small_button("查看 Agent 会话").clicked()
                        {
                            selected = Some(agent.clone());
                        }
                        if !row.raw.is_empty() {
                            egui::CollapsingHeader::new("技术详情 / 原始记录").show(ui, |ui| {
                                selectable_text(ui, &row.raw);
                            });
                        }
                    });
                    ui.add_space(8.0);
                });
            });
        selected
    }
}
impl Workspace {
    pub(super) fn activity_view(&mut self, ui: &mut egui::Ui, run: &RunSummary) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Agent 动态与分析记录").color(MUTED));
            ui.checkbox(&mut self.technical_events, "显示技术事件");
            ui.checkbox(&mut self.follow, "跟随最新");
        });
        if self.data.events.len() >= self.selection.event_limit
            && self.selection.event_limit < 10_000
            && ui.button("加载更早活动").clicked()
        {
            self.selection.event_limit = (self.selection.event_limit + 500).min(10_000);
            let _ = self.refresh();
        }
        let mut selected = None;
        crate::gui_chat::scroll(ui, ("events", &run.id), &mut self.follow, false, |ui| {
            selected = self.activity.show(ui, self.technical_events);
        });
        if let Some(id) = selected {
            self.select_agent(id);
            self.selection.view = View::Agent;
        }
    }
}
