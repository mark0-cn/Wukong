//! Public progress and tool outcomes form a readable transcript, independently of UI widgets.
use crate::{
    domain::*,
    i18n::{DisplayText, Message as UiMessage},
    queries::AgentSummary,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Serialize)]
pub struct ToolDisplay {
    pub call_id: String,
    pub title: String,
    pub target: String,
    pub purpose: String,
    pub state: String,
    pub summary: DisplayText,
    pub duration_ms: Option<i64>,
    pub evidence_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<CodePreview>,
}
#[derive(Clone, Serialize)]
pub struct CodePreview {
    pub language: String,
    pub text: String,
    pub truncated: bool,
    pub evidence_id: String,
}
#[derive(Clone, Serialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: &'static str,
    pub agent_id: Option<String>,
    pub author: DisplayText,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub content: Vec<ChatPart>,
    pub delivery: Option<MessageDelivery>,
}
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ChatPart {
    Text {
        text: DisplayText,
    },
    ToolCall {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        args: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(rename = "isError")]
        is_error: bool,
    },
}
pub fn tool_title(name: &str) -> &str {
    crate::tools::tool_display_title(name)
        .or_else(|| super::tool_aliases::title(name))
        .unwrap_or(name)
}
pub fn excerpt(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let mut output: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        output.push('…');
    }
    output
}
fn text<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
    })
}
mod tools;
pub use tools::{result_summary, tool_display};

pub fn transcript(
    events: &[Event],
    calls: &[ToolCallRecord],
    agents: &[AgentSummary],
    mail: &[Message],
    target_name: &str,
) -> Vec<ChatMessage> {
    let agents: BTreeMap<_, _> = agents
        .iter()
        .map(|agent| (agent.id.as_str(), agent))
        .collect();
    let by_call: BTreeMap<_, _> = calls.iter().map(|call| (call.id.as_str(), call)).collect();
    let mut seen = BTreeSet::new();
    let mut messages = Vec::new();
    for event in events {
        let author = event
            .agent_id
            .as_deref()
            .and_then(|id| agents.get(id))
            .map_or_else(
                || UiMessage::new("transcript-workspace").into(),
                |agent| agent.name.clone().into(),
            );
        let mut message = ChatMessage {
            id: format!("event-{}", event.seq),
            role: "assistant",
            agent_id: event.agent_id.clone(),
            author,
            created_at: event.created_at,
            content: vec![],
            delivery: None,
        };
        if matches!(event.kind.as_str(), "tool.started" | "tool.finished") {
            let Some(id) = event.data["call_id"].as_str() else {
                continue;
            };
            if !seen.insert(id.to_owned()) {
                continue;
            }
            let Some(call) = by_call.get(id) else {
                continue;
            };
            let display = tool_display(
                call,
                agents.get(call.agent_id.as_str()).copied(),
                target_name,
            );
            message.id = format!("call-{}", call.id);
            message.created_at = call.created_at;
            message.content.push(ChatPart::ToolCall {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                args: json!({"display":display}),
                result: call.result.as_ref().map(|_| json!({"display":display})),
                is_error: matches!(call.state, CallState::Failed | CallState::Uncertain),
            });
        } else {
            let content = match event.kind.as_str() {
                "model.text" => text(&event.data, &["text"])
                    .filter(|text| !text.starts_with("requested tools:"))
                    .map(|value| DisplayText::from(value.to_owned())),
                "agent.error" => text(&event.data, &["message", "error"])
                    .map(|text| UiMessage::new("transcript-error").arg("body", text).into()),
                "agent.status"
                    if matches!(event.data["status"].as_str(), Some("failed" | "crashed")) =>
                {
                    text(&event.data, &["error"])
                        .map(|text| UiMessage::new("transcript-error").arg("body", text).into())
                }
                "agent.completed" => text(&event.data, &["summary"]).map(|text| {
                    UiMessage::new("transcript-conclusion")
                        .arg("body", text)
                        .into()
                }),
                "agents.stopped" => Some(UiMessage::new("transcript-stopped").into()),
                "run.budget_paused" => Some(match event.data["reason"].as_str() {
                    Some("operator paused from desktop" | "operator paused assessment") => {
                        UiMessage::new("system-analysis-paused-progress-is-saved").into()
                    }
                    Some(reason) => UiMessage::new("transcript-paused-reason")
                        .arg("reason", reason)
                        .into(),
                    None => UiMessage::new("system-analysis-paused-progress-is-saved").into(),
                }),
                "run.resumed" => Some(UiMessage::new("transcript-resumed").into()),
                _ => None,
            };
            let Some(content) = content else {
                continue;
            };
            message.content.push(ChatPart::Text { text: content });
        }
        messages.push(message);
    }
    for mail in mail {
        messages.push(ChatMessage {
            id: format!("operator-{}", mail.id),
            role: "user",
            agent_id: Some(mail.recipient.clone()),
            author: agents.get(mail.recipient.as_str()).map_or_else(
                || UiMessage::new("transcript-you").into(),
                |agent| {
                    UiMessage::new("transcript-recipient")
                        .arg("name", agent.name.clone())
                        .into()
                },
            ),
            created_at: mail.created_at,
            content: vec![ChatPart::Text {
                text: mail.content.clone().into(),
            }],
            delivery: Some(mail.envelope.delivery),
        });
    }
    messages.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut grouped: Vec<ChatMessage> = vec![];
    for message in messages {
        let tools = message
            .content
            .iter()
            .all(|part| matches!(part, ChatPart::ToolCall { .. }));
        if tools
            && let Some(previous) = grouped.last_mut()
            && previous.role == "assistant"
            && previous.agent_id == message.agent_id
        {
            previous.content.extend(message.content);
        } else {
            grouped.push(message);
        }
    }
    grouped
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn start_and_finish_update_one_operation_and_preserve_errors() {
        let mut call = ToolCallRecord {
            id: "call".into(),
            run_id: "run".into(),
            agent_id: "agent".into(),
            name: "binary_strings".into(),
            arguments: json!({"pattern":"read"}),
            state: CallState::Failed,
            side_effecting: false,
            result: Some(json!({"error":"worker unavailable"})),
            evidence_id: None,
            created_at: now(),
            updated_at: now(),
        };
        let events = ["tool.started", "tool.finished"]
            .into_iter()
            .enumerate()
            .map(|(i, kind)| Event {
                seq: i as i64,
                run_id: "run".into(),
                agent_id: Some("agent".into()),
                kind: kind.into(),
                data: json!({"call_id":"call"}),
                created_at: now(),
            })
            .collect::<Vec<_>>();
        let messages = transcript(&events, &[call.clone()], &[], &[], "sample.elf");
        assert_eq!(messages.len(), 1);
        let ChatPart::ToolCall { args, is_error, .. } = &messages[0].content[0] else {
            panic!("tool missing")
        };
        assert!(*is_error);
        assert_eq!(args["display"]["state"], "failed");
        call.state = CallState::Completed;
        call.result = Some(json!({"matches":[1,2,3]}));
        assert_eq!(
            tool_display(&call, None, "sample.elf")
                .summary
                .render(crate::i18n::Locale::ZhCn),
            "返回 3 项匹配"
        );
    }
}
