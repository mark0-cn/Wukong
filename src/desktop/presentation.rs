//! Public progress and tool outcomes form a readable transcript, independently of UI widgets.
use crate::{domain::*, queries::AgentSummary};
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
    pub summary: String,
    pub duration_ms: Option<i64>,
    pub evidence_id: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: &'static str,
    pub agent_id: Option<String>,
    pub author: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub content: Vec<ChatPart>,
    pub delivery: Option<MessageDelivery>,
}
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ChatPart {
    Text {
        text: String,
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
pub fn result_summary(value: &Value) -> String {
    summary(value, 0)
}
fn summary(value: &Value, depth: usize) -> String {
    if depth > 6 {
        return String::new();
    }
    match value {
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .map(|value| summary(&value, depth + 1))
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| excerpt(text, 600)),
        Value::Array(items) => items
            .iter()
            .map(|item| summary(item, depth + 1))
            .find(|text| !text.is_empty())
            .unwrap_or_default(),
        Value::Object(map) => {
            if let Some(text) = text(
                value,
                &[
                    "error",
                    "summary",
                    "answer",
                    "message",
                    "stdout",
                    "stderr",
                    "text",
                    "description",
                ],
            ) {
                return excerpt(text, 600);
            }
            let facts = [
                ("format", "格式"),
                ("architecture", "架构"),
                ("arch", "架构"),
                ("status", "状态"),
                ("exit_code", "退出码"),
            ]
            .into_iter()
            .filter_map(|(key, label)| {
                map.get(key)
                    .filter(|v| v.is_string() || v.is_number())
                    .map(|value| {
                        format!(
                            "{label}：{}",
                            value
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| value.to_string())
                        )
                    })
            })
            .collect::<Vec<_>>();
            if !facts.is_empty() {
                return facts.join(" · ");
            }
            for key in ["matches", "functions", "strings", "findings", "results"] {
                if let Some(Value::Array(items)) = map.get(key) {
                    return format!(
                        "返回 {} 项{}",
                        items.len(),
                        match key {
                            "matches" => "匹配",
                            "functions" => "函数",
                            "strings" => "字符串",
                            "findings" => "发现",
                            _ => "结果",
                        }
                    );
                }
            }
            for key in ["result", "content", "output", "value", "data"] {
                if let Some(value) = map.get(key) {
                    let text = summary(value, depth + 1);
                    if !text.is_empty() {
                        return text;
                    }
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}
pub fn tool_display(call: &ToolCallRecord, agent: Option<&AgentSummary>) -> ToolDisplay {
    let terminal = agent.is_some_and(|agent| agent.status.terminal());
    let paused = agent.is_some_and(|agent| agent.status == Status::BudgetPaused);
    let state = match call.state {
        CallState::Prepared => "queued",
        CallState::Running if paused => "paused",
        CallState::Running if terminal => "interrupted",
        CallState::Running => "running",
        CallState::Completed => "completed",
        CallState::Failed => "failed",
        CallState::Uncertain => "uncertain",
    };
    let target = text(
        &call.arguments,
        &[
            "path",
            "function",
            "function_name",
            "address",
            "query",
            "pattern",
            "name",
            "job_id",
            "evidence_id",
            "scope",
        ],
    )
    .map(|text| excerpt(text, 180))
    .unwrap_or_else(|| {
        call.arguments["argv"]
            .as_array()
            .map(|argv| {
                excerpt(
                    &argv
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                    180,
                )
            })
            .unwrap_or_default()
    });
    let purpose = text(&call.arguments, &["description", "objective", "task"])
        .map(|text| excerpt(text, 240))
        .unwrap_or_default();
    let summary = call
        .result
        .as_ref()
        .map(result_summary)
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| {
            match state {
                "completed" => "操作已完成，可展开查看原始结果。",
                "failed" => "操作执行失败，可展开查看错误详情。",
                "uncertain" => "执行结果尚未确认，需要核对后再继续。",
                "paused" => "分析已暂停，保留当前操作状态。",
                "interrupted" => "所属任务已结束，本次操作未确认完成。",
                _ => "",
            }
            .into()
        });
    ToolDisplay {
        call_id: call.id.clone(),
        title: tool_title(&call.name).into(),
        target,
        purpose,
        state: state.into(),
        summary,
        duration_ms: matches!(call.state, CallState::Completed | CallState::Failed).then_some(
            (call.updated_at - call.created_at)
                .num_milliseconds()
                .max(0),
        ),
        evidence_id: call.evidence_id.clone(),
    }
}
pub fn transcript(
    events: &[Event],
    calls: &[ToolCallRecord],
    agents: &[AgentSummary],
    mail: &[Message],
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
            .map_or("工作区", |agent| agent.name.as_str())
            .to_owned();
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
            let display = tool_display(call, agents.get(call.agent_id.as_str()).copied());
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
                    .map(str::to_owned),
                "agent.error" => text(&event.data, &["message", "error"])
                    .map(|text| format!("**分析遇到问题**\n\n{text}")),
                "agent.status"
                    if matches!(event.data["status"].as_str(), Some("failed" | "crashed")) =>
                {
                    text(&event.data, &["error"]).map(|text| format!("**分析遇到问题**\n\n{text}"))
                }
                "agent.completed" => {
                    text(&event.data, &["summary"]).map(|text| format!("**任务结论**\n\n{text}"))
                }
                "agents.stopped" => Some("此 Agent 分支已停止，已有证据和分析结果已保留。".into()),
                "run.budget_paused" => Some(match event.data["reason"].as_str() {
                    Some("operator paused from desktop" | "operator paused assessment") => {
                        "分析已暂停，进度已保存。".into()
                    }
                    Some(reason) => format!("分析已暂停：{reason}"),
                    None => "分析已暂停，进度已保存。".into(),
                }),
                "run.resumed" => Some("已恢复分析，继续处理未完成的工作。".into()),
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
            author: agents
                .get(mail.recipient.as_str())
                .map_or_else(|| "你".into(), |agent| format!("你 → {}", agent.name)),
            created_at: mail.created_at,
            content: vec![ChatPart::Text {
                text: mail.content.clone(),
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
        let messages = transcript(&events, &[call.clone()], &[], &[]);
        assert_eq!(messages.len(), 1);
        let ChatPart::ToolCall { args, is_error, .. } = &messages[0].content[0] else {
            panic!("tool missing")
        };
        assert!(*is_error);
        assert_eq!(args["display"]["state"], "failed");
        call.state = CallState::Completed;
        call.result = Some(json!({"matches":[1,2,3]}));
        assert_eq!(tool_display(&call, None).summary, "返回 3 项匹配");
    }
}
