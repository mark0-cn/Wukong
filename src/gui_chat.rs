//! Conversation presentation: pair tool calls with results instead of exposing protocol messages.
use crate::{domain::AgentRecord, gui_activity::tool_name};
use eframe::egui::{self, Color32, RichText};
use serde_json::Value;

#[derive(Default)]
struct Turn {
    text: Vec<String>,
    tools: Vec<Operation>,
    details: Vec<Value>,
}
struct Operation {
    name: String,
    args: Value,
    result: Option<Value>,
}
fn turns(checkpoint: &Value) -> anyhow::Result<Vec<Turn>> {
    use rig::completion::{AssistantContent, Message, message::UserContent};
    let conversation: rig_core::conversation::Conversation =
        serde_json::from_value(checkpoint.clone())?;
    let mut results = std::collections::HashMap::new();
    for message in conversation.messages() {
        if let Message::User { content } = &**message {
            for part in content {
                if let UserContent::ToolResult(result) = part {
                    results.insert(result.call.to_string(), serde_json::to_value(result)?);
                }
            }
        }
    }
    let mut turns = vec![];
    for message in conversation.messages() {
        if let Message::Assistant(assistant) = &**message {
            let mut turn = Turn::default();
            for part in &assistant.content {
                match part {
                    AssistantContent::Text(text) if !text.text.trim().is_empty() => {
                        turn.text.push(text.text.clone())
                    }
                    AssistantContent::ToolCall(call) => turn.tools.push(Operation {
                        name: call.function.name.to_string(),
                        args: Value::Object(call.function.arguments.clone()),
                        result: results.get(&call.id.to_string()).cloned(),
                    }),
                    _ => turn.details.push(serde_json::to_value(part)?),
                }
            }
            if !turn.text.is_empty() || !turn.tools.is_empty() || !turn.details.is_empty() {
                turns.push(turn);
            }
        }
    }
    Ok(turns)
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
    let mut code = false;
    for paragraph in text.split('\n') {
        if paragraph.starts_with("```") {
            code = !code;
            continue;
        }
        if paragraph.trim().is_empty() {
            ui.add_space(4.0);
            continue;
        }
        let label = if code {
            RichText::new(paragraph)
                .monospace()
                .color(Color32::from_rgb(182, 203, 220))
        } else if paragraph.starts_with('#') {
            RichText::new(paragraph.trim_start_matches('#').trim())
                .strong()
                .size(17.0)
        } else {
            RichText::new(paragraph).size(15.0)
        };
        ui.add(egui::Label::new(label).wrap().selectable(true));
    }
}
pub(crate) fn show(ui: &mut egui::Ui, agent: &AgentRecord, verbose: bool) {
    let Some(checkpoint) = &agent.checkpoint else {
        ui.label("Agent 正在准备，消息会自动出现在这里。");
        return;
    };
    let turns = match turns(checkpoint) {
        Ok(turns) => turns,
        Err(error) => {
            ui.colored_label(Color32::LIGHT_RED, format!("无法读取会话：{error}"));
            return;
        }
    };
    ui.spacing_mut().item_spacing.y = 6.0;
    if turns.iter().all(|turn| turn.text.is_empty()) {
        ui.label(
            RichText::new("这段历史记录只有工具操作，尚无公开回复。下方展示真实操作及返回结果。")
                .small()
                .color(Color32::from_rgb(145, 161, 181)),
        );
        ui.add_space(12.0);
    }
    for (index, turn) in turns.iter().enumerate() {
        if turn.text.is_empty() && turn.tools.is_empty() && !verbose {
            continue;
        }
        ui.push_id(index, |ui| {
            if !turn.text.is_empty() {
                ui.label(
                    RichText::new(&agent.name)
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
                                    body(
                                        ui,
                                        &serde_json::to_string_pretty(&operation.args)
                                            .unwrap_or_default(),
                                    );
                                    if let Some(result) = &operation.result {
                                        body(
                                            ui,
                                            &serde_json::to_string_pretty(result)
                                                .unwrap_or_default(),
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
                        body(
                            ui,
                            &serde_json::to_string_pretty(detail).unwrap_or_default(),
                        );
                    }
                });
            }
            ui.add_space(14.0);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
