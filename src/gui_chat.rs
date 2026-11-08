//! Conversation presentation: pair tool calls with results instead of exposing protocol messages.
use crate::{domain::AgentSnapshot, gui_activity::tool_name};
use eframe::egui::{self, Color32, RichText};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Default, Serialize)]
struct Turn {
    text: Vec<String>,
    tools: Vec<Operation>,
    details: Vec<Value>,
}
#[derive(Clone, Serialize)]
struct Operation {
    name: String,
    args: Value,
    result: Option<Value>,
}
fn parse(checkpoint: &Value, previous: Option<&Parsed>) -> anyhow::Result<Parsed> {
    use rig::completion::{AssistantContent, Message, message::UserContent};
    let messages = checkpoint["messages"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("conversation messages missing"))?;
    let reusable = previous.filter(|previous| {
        previous.error.is_none()
            && previous.source.len() <= messages.len()
            && previous
                .source
                .iter()
                .zip(messages)
                .all(|(before, after)| before.as_ref() == after)
    });
    let mut parsed = if let Some(previous) = reusable {
        Parsed {
            turns: previous.turns.clone(),
            keys: previous.keys.clone(),
            source: previous.source.clone(),
            operations: previous.operations.clone(),
            error: None,
        }
    } else {
        Parsed::default()
    };
    let mut changed = std::collections::BTreeSet::new();
    for raw in messages.iter().skip(parsed.source.len()) {
        let message: Message = serde_json::from_value(raw.clone())?;
        match message {
            Message::Assistant(assistant) => {
                let mut turn = Turn::default();
                for part in assistant.content {
                    match part {
                        AssistantContent::Text(text) if !text.text.trim().is_empty() => {
                            turn.text.push(text.text)
                        }
                        AssistantContent::ToolCall(call) => {
                            parsed.operations.insert(
                                call.id.to_string(),
                                (parsed.turns.len(), turn.tools.len()),
                            );
                            turn.tools.push(Operation {
                                name: call.function.name.to_string(),
                                args: Value::Object(call.function.arguments),
                                result: None,
                            });
                        }
                        part => turn.details.push(serde_json::to_value(part)?),
                    }
                }
                if !turn.text.is_empty() || !turn.tools.is_empty() || !turn.details.is_empty() {
                    changed.insert(parsed.turns.len());
                    parsed.turns.push(Arc::new(turn));
                    parsed.keys.push(0);
                }
            }
            Message::User { content } => {
                for part in content {
                    if let UserContent::ToolResult(result) = part
                        && let Some(&(turn, operation)) =
                            parsed.operations.get(&result.call.to_string())
                    {
                        Arc::make_mut(&mut parsed.turns[turn]).tools[operation].result =
                            Some(serde_json::to_value(result)?);
                        changed.insert(turn);
                    }
                }
            }
            _ => {}
        }
        parsed.source.push(Arc::new(raw.clone()));
    }
    for index in changed {
        let hash = Sha256::digest(serde_json::to_vec(&*parsed.turns[index])?);
        parsed.keys[index] = u64::from_le_bytes(hash[..8].try_into().expect("digest prefix"));
    }
    Ok(parsed)
}
#[cfg(test)]
fn turns(checkpoint: &Value) -> anyhow::Result<Vec<Arc<Turn>>> {
    Ok(parse(checkpoint, None)?.turns)
}
fn preview(value: &Value) -> String {
    match value {
        Value::String(text) => {
            if let Ok(value) = serde_json::from_str::<Value>(text) {
                return preview(&value);
            }
            text.chars().take(240).collect()
        }
        Value::Object(object) => {
            for key in [
                "error",
                "summary",
                "answer",
                "stdout",
                "text",
                "message",
                "description",
                "content",
                "value",
                "result",
                "state",
            ] {
                if let Some(value) = object.get(key) {
                    let text = preview(value);
                    if !text.trim().is_empty() {
                        return text;
                    }
                }
            }
            String::new()
        }
        Value::Array(values) => values
            .iter()
            .map(preview)
            .find(|s| !s.trim().is_empty())
            .unwrap_or_default(),
        _ => String::new(),
    }
}
/// Follow appended messages, while leaving the reader in control of history.
pub(crate) fn scroll(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    follow: &mut bool,
    jump: bool,
    contents: impl FnOnce(&mut egui::Ui),
) -> egui::scroll_area::ScrollAreaOutput<()> {
    let pointer_scroll = ui.input(|input| {
        input
            .pointer
            .hover_pos()
            .is_some_and(|p| ui.max_rect().contains(p))
            && (input.raw_scroll_delta.y != 0.0 || input.pointer.primary_down())
    });
    let output = egui::ScrollArea::vertical()
        .id_salt(id)
        .stick_to_bottom(*follow)
        .animated(false)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            contents(ui);
            if jump {
                // Aligning a last widget leaves egui's clip margin below it,
                // preventing stickiness. Move through the entire content instead.
                ui.scroll_with_delta(egui::vec2(0.0, -ui.min_rect().height()));
            }
        });
    if !jump && (*follow || pointer_scroll) {
        *follow = output.state.offset.y + output.inner_rect.height() >= output.content_size.y - 4.0;
    }
    output
}
fn arguments(value: &Value) -> String {
    if let Some(argv) = value.get("argv").and_then(Value::as_array) {
        return argv
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(180)
            .collect();
    }
    for key in [
        "description",
        "task",
        "query",
        "pattern",
        "path",
        "job_id",
        "evidence_id",
        "id",
    ] {
        if let Some(value) = value.get(key).and_then(Value::as_str) {
            return value.chars().take(180).collect();
        }
    }
    String::new()
}
fn body(ui: &mut egui::Ui, text: &str) {
    crate::gui_markdown::show(ui, text);
}
/// Parsed once by the background reader. No raw checkpoint is kept by the GUI.
#[derive(Clone)]
pub(crate) struct Document {
    pub id: String,
    generation: u64,
    digest: String,
    data: Arc<Parsed>,
}
#[derive(Default)]
struct Parsed {
    turns: Vec<Arc<Turn>>,
    source: Vec<Arc<Value>>,
    operations: std::collections::HashMap<String, (usize, usize)>,
    keys: Vec<u64>,
    error: Option<String>,
}
impl Document {
    pub fn prepare(agent: &AgentSnapshot, previous: Option<&Self>) -> Self {
        let bytes = serde_json::to_vec(&agent.checkpoint).unwrap_or_default();
        let digest = hex::encode(Sha256::digest(&bytes));
        if let Some(previous) = previous
            && previous.id == agent.id
            && previous.generation == agent.generation
            && previous.digest == digest
        {
            return previous.clone();
        }
        let reusable = previous
            .filter(|previous| previous.id == agent.id && previous.generation == agent.generation);
        let data = match agent
            .checkpoint
            .as_ref()
            .map(|checkpoint| parse(checkpoint, reusable.map(|previous| previous.data.as_ref())))
            .transpose()
        {
            Ok(Some(parsed)) => parsed,
            Ok(None) => Parsed::default(),
            Err(error) => Parsed {
                error: Some(error.to_string()),
                ..Parsed::default()
            },
        };
        Self {
            id: agent.id.clone(),
            generation: agent.generation,
            digest,
            data: Arc::new(data),
        }
    }
}

/// Prefix heights locate the visible blocks with two binary searches. Unchanged
/// blocks retain their measured height when a response or tool result is appended.
#[derive(Default)]
pub(crate) struct Layout {
    owner: Option<(String, u64)>,
    digest: String,
    width: f32,
    zoom: f32,
    verbose: bool,
    keys: Vec<u64>,
    heights: Vec<f32>,
    offsets: Vec<f32>,
}
impl Layout {
    fn prepare(&mut self, document: &Document, width: f32, zoom: f32, verbose: bool) {
        let reset = self.owner.as_ref() != Some(&(document.id.clone(), document.generation))
            || (self.width - width).abs() > 1.0
            || self.zoom != zoom
            || self.verbose != verbose;
        if !reset && self.digest == document.digest {
            return;
        }
        let heights = document
            .data
            .keys
            .iter()
            .enumerate()
            .map(|(index, key)| {
                if !reset && self.keys.get(index) == Some(key) {
                    self.heights[index]
                } else {
                    120.0
                }
            })
            .collect();
        self.owner = Some((document.id.clone(), document.generation));
        self.digest = document.digest.clone();
        self.width = width;
        self.zoom = zoom;
        self.verbose = verbose;
        self.keys = document.data.keys.clone();
        self.heights = heights;
        self.rebuild();
    }
    fn rebuild(&mut self) {
        self.offsets.clear();
        self.offsets.push(0.0);
        for height in &self.heights {
            self.offsets
                .push(self.offsets.last().copied().unwrap_or(0.0) + height);
        }
    }
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    document: &Document,
    name: &str,
    layout: &mut Layout,
    verbose: bool,
) -> usize {
    if let Some(error) = &document.data.error {
        ui.colored_label(Color32::LIGHT_RED, format!("无法读取会话：{error}"));
        return 0;
    }
    if document.data.turns.is_empty() {
        ui.label("Agent 正在准备，消息会自动出现在这里。");
        return 0;
    }
    ui.spacing_mut().item_spacing.y = 6.0;
    layout.prepare(
        document,
        ui.available_width(),
        ui.ctx().zoom_factor(),
        verbose,
    );
    let origin = ui.next_widget_position().y;
    let clip = ui.clip_rect();
    let low = (clip.min.y - origin - 200.0).max(0.0);
    let high = (clip.max.y - origin + 200.0).max(0.0);
    let start = layout
        .offsets
        .partition_point(|offset| *offset < low)
        .saturating_sub(1)
        .min(layout.heights.len());
    let end = layout
        .offsets
        .partition_point(|offset| *offset <= high)
        .min(layout.heights.len())
        .max(start);
    ui.add_space(layout.offsets[start]);
    let mut changed = false;
    for index in start..end {
        let turn = &document.data.turns[index];
        let before = ui.next_widget_position().y;
        render_turn(ui, index, turn, name, verbose);
        let height = (ui.next_widget_position().y - before).max(1.0);
        if (layout.heights[index] - height).abs() > 0.5 {
            layout.heights[index] = height;
            changed = true;
        }
    }
    let remaining = layout.offsets.last().copied().unwrap_or(0.0) - layout.offsets[end];
    ui.add_space(remaining);
    if changed {
        layout.rebuild();
        ui.ctx().request_repaint();
    }
    end - start
}
fn render_turn(ui: &mut egui::Ui, index: usize, turn: &Turn, name: &str, verbose: bool) {
    ui.push_id(index, |ui| {
        if !turn.text.is_empty() {
            ui.label(
                RichText::new(name)
                    .small()
                    .strong()
                    .color(Color32::from_rgb(83, 218, 189)),
            );
            for text in &turn.text {
                body(ui, text);
            }
            ui.add_space(8.0);
        }
        if !turn.tools.is_empty() {
            egui::Frame::new()
                .fill(Color32::from_rgb(23, 32, 43))
                .corner_radius(8)
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for (i, operation) in turn.tools.iter().enumerate() {
                        ui.push_id(i, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(if operation.result.is_some() {
                                        "● 已返回"
                                    } else {
                                        "◷ 等待返回"
                                    })
                                    .small()
                                    .color(Color32::from_rgb(145, 161, 181)),
                                );
                                ui.label(RichText::new(tool_name(&operation.name)).strong());
                            });
                            let args = arguments(&operation.args);
                            if !args.is_empty() {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(args)
                                            .small()
                                            .color(Color32::from_rgb(145, 161, 181)),
                                    )
                                    .wrap(),
                                );
                            }
                            if let Some(result) = &operation.result {
                                let summary = preview(result);
                                if !summary.is_empty() {
                                    body(ui, &summary);
                                }
                            }
                            egui::CollapsingHeader::new("参数与完整结果").show(ui, |ui| {
                                crate::gui_markdown::code(
                                    ui,
                                    "json",
                                    &serde_json::to_string_pretty(&operation.args)
                                        .unwrap_or_default(),
                                );
                                if let Some(result) = &operation.result {
                                    crate::gui_markdown::code(
                                        ui,
                                        "json",
                                        &serde_json::to_string_pretty(result).unwrap_or_default(),
                                    );
                                }
                            });
                        });
                        if i + 1 < turn.tools.len() {
                            ui.separator();
                        }
                    }
                });
        }
        if verbose && !turn.details.is_empty() {
            egui::CollapsingHeader::new("模型附加记录").show(ui, |ui| {
                for detail in &turn.details {
                    crate::gui_markdown::code(
                        ui,
                        "json",
                        &serde_json::to_string_pretty(detail).unwrap_or_default(),
                    );
                }
            });
        }
        ui.add_space(14.0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn appended_messages_reuse_old_turns_and_results_update_the_matching_turn() -> anyhow::Result<()>
    {
        let mut checkpoint = serde_json::json!({"messages":[
            {"role":"assistant","content":[{"type":"text","text":"first"}]},
            {"role":"assistant","content":[{"type":"toolcall","id":{"provider":"call"},"function":{"name":"evidence_read","arguments":{"id":"a"}}}]}
        ]});
        let initial = parse(&checkpoint, None)?;
        checkpoint["messages"].as_array_mut().unwrap().push(serde_json::json!({
            "role":"user","content":[{"type":"toolresult","call":{"provider":"call"},"name":"evidence_read","content":[{"type":"text","text":"arrived"}],"is_error":false}]
        }));
        let appended = parse(&checkpoint, Some(&initial))?;
        assert!(Arc::ptr_eq(&initial.turns[0], &appended.turns[0]));
        assert!(Arc::ptr_eq(&initial.source[0], &appended.source[0]));
        assert!(!Arc::ptr_eq(&initial.turns[1], &appended.turns[1]));
        assert!(initial.turns[1].tools[0].result.is_none());
        assert_eq!(
            preview(appended.turns[1].tools[0].result.as_ref().unwrap()),
            "arrived"
        );
        checkpoint["messages"].as_array_mut().unwrap().remove(0);
        let compacted = parse(&checkpoint, Some(&appended))?;
        assert_eq!(compacted.turns.len(), 1);
        assert!(compacted.turns[0].tools[0].result.is_some());
        Ok(())
    }

    #[test]
    fn long_conversations_only_layout_visible_blocks() {
        let document = Document {
            id: "fixture".into(),
            generation: 0,
            digest: "fixture".into(),
            data: Arc::new(Parsed {
                turns: (0..2000)
                    .map(|i| {
                        Arc::new(Turn {
                            text: vec![format!("message {i}")],
                            ..Turn::default()
                        })
                    })
                    .collect(),
                keys: (0..2000).collect(),
                ..Parsed::default()
            }),
        };
        let ctx = egui::Context::default();
        let mut layout = Layout::default();
        let mut count = 0;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 400.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        count = show(ui, &document, "fixture", &mut layout, false);
                    });
                });
            },
        );
        assert!(count > 0 && count < 20, "rendered {count} of 2000 turns");
        assert_eq!(layout.offsets.len(), 2001);
        assert!(layout.offsets.last().unwrap() > &200_000.0);
    }

    #[test]
    fn matches_parallel_tool_results_by_id_and_keeps_public_text() {
        let checkpoint = serde_json::json!({
            "messages": [
                {"role":"assistant","content":[
                    {"type":"text","text":"正在核对两份证据。"},
                    {"type":"toolcall","id":{"provider":"first"},"function":{"name":"evidence_read","arguments":{"id":"a"}}},
                    {"type":"toolcall","id":{"provider":"second"},"function":{"name":"evidence_read","arguments":{"id":"b"}}},
                    {"type":"toolcall","id":{"provider":"pending"},"function":{"name":"job_status","arguments":{}}}
                ]},
                {"role":"user","content":[
                    {"type":"toolresult","call":{"provider":"second"},"name":"evidence_read","content":[{"type":"text","text":"second result"}]},
                    {"type":"toolresult","call":{"provider":"first"},"name":"evidence_read","content":[{"type":"json","value":{"summary":"first result"}}]}
                ]}
            ],
            "asides":[],"meta":[{"id":0},{"id":1}],"next_id":2,"next_aside":0
        });
        let turns = turns(&checkpoint).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].text, ["正在核对两份证据。"]);
        assert_eq!(
            preview(turns[0].tools[0].result.as_ref().unwrap()),
            "first result"
        );
        assert_eq!(
            preview(turns[0].tools[1].result.as_ref().unwrap()),
            "second result"
        );
        assert!(turns[0].tools[2].result.is_none());
    }

    #[test]
    fn scrolling_follows_append_pauses_for_history_and_jumps_back() {
        let ctx = egui::Context::default();
        let mut follow = true;
        let frame = |height: f32, jump: bool, events: Vec<egui::Event>, follow: &mut bool| {
            let mut result = (0.0, 0.0);
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 400.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let output = scroll(ui, "test-chat", follow, jump, |ui| {
                            ui.allocate_space(egui::vec2(300.0, height));
                        });
                        result = (
                            output.state.offset.y,
                            output.content_size.y - output.inner_rect.height(),
                        );
                    });
                },
            );
            result
        };
        let first = frame(900.0, true, vec![], &mut follow);
        assert!((first.0 - first.1).abs() < 1.0);
        let appended = frame(1200.0, false, vec![], &mut follow);
        assert!((appended.0 - appended.1).abs() < 1.0);
        let history = frame(
            1200.0,
            false,
            vec![
                egui::Event::PointerMoved(egui::pos2(200.0, 200.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, 180.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            &mut follow,
        );
        assert!(history.0 < appended.0);
        assert!(!follow);
        let paused = frame(1500.0, false, vec![], &mut follow);
        assert!(paused.0 <= history.0 + 1.0);
        // Drain wheel smoothing, then verify an explicit jump resumes following.
        for _ in 0..30 {
            frame(1500.0, false, vec![], &mut follow);
        }
        follow = true;
        let resumed = frame(1500.0, true, vec![], &mut follow);
        assert!((resumed.0 - resumed.1).abs() < 1.0, "resumed {resumed:?}");
        let appended = frame(1800.0, false, vec![], &mut follow);
        assert!((appended.0 - appended.1).abs() < 1.0);
        // Disabling the checkbox at the bottom must remain disabled on the next frame.
        follow = false;
        frame(1800.0, false, vec![], &mut follow);
        frame(2100.0, false, vec![], &mut follow);
        assert!(!follow);
    }

    #[test]
    fn result_preview_reads_nested_text_and_is_bounded() {
        assert_eq!(
            preview(
                &serde_json::json!({"content":[{"type":"text","text":"{\"stdout\":\"verified output\"}"}]})
            ),
            "verified output"
        );
        assert_eq!(
            preview(&Value::String("中".repeat(400))).chars().count(),
            240
        );
    }
}
