//! Human-readable projections of durable activity; raw evidence remains available.
use crate::domain::{AgentSnapshot, Event, ToolCallRecord};
use serde_json::Value;

pub(crate) fn public_replies(agent: &AgentSnapshot) -> Vec<String> {
    use rig::completion::{AssistantContent, Message};
    let Some(checkpoint) = &agent.checkpoint else {
        return vec![];
    };
    let Ok(conversation) =
        serde_json::from_value::<rig_core::conversation::Conversation>(checkpoint.clone())
    else {
        return vec![];
    };
    conversation
        .messages()
        .iter()
        .flat_map(|message| match &**message {
            Message::Assistant(assistant) => assistant
                .content
                .iter()
                .filter_map(|part| match part {
                    AssistantContent::Text(text) if !text.text.trim().is_empty() => {
                        Some(text.text.clone())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect()
}

pub(crate) fn technical(event: &Event) -> bool {
    matches!(
        event.kind.as_str(),
        "model.usage" | "model.usage_aux" | "model.stream_progress" | "agent.status"
    ) && event.data.get("error").and_then(Value::as_str).is_none()
}
fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}
fn short(text: &str) -> String {
    let mut chars = text.chars();
    let mut output: String = chars.by_ref().take(600).collect();
    if chars.next().is_some() {
        output.push_str("…（展开详情查看完整内容）");
    }
    output
}
fn readable_reason(reason: &str) -> String {
    match reason {
        "operator paused from desktop" => "你点击了暂停，当前进度已保存。".into(),
        "operator paused assessment" => "操作者请求暂停，当前进度已保存。".into(),
        _ => reason.to_owned(),
    }
}
pub(crate) fn tool_name(name: &str) -> &str {
    match name {
        "notes_list" => "阅读分析笔记",
        "todos_list" => "查看待办任务",
        "stage_target" => "准备分析样本",
        "binary_inspect" => "检查文件格式与安全属性",
        "binary_strings" => "提取并检索字符串",
        "runtime_doctor" => "检查分析环境",
        "job_start" => "启动后台分析任务",
        "job_status" => "查看后台任务进度",
        "evidence_read" => "读取分析证据",
        "load_skill" => "加载专项分析方法",
        "tool_search" => "查找分析工具",
        "tools_load" => "准备分析工具",
        "task" | "spawn_agent" => "分派子任务",
        "wait" | "wait_for_agents" => "等待协作结果",
        "message" | "send_message_to_agent" => "发送协作消息",
        "think" => "记录分析笔记",
        "context_write" | "note_write" => "保存分析笔记",
        "context_read" => "阅读共享分析记录",
        "context_search" => "检索共享分析记录",
        "agent_finish" => "提交子任务结论",
        "finish_scan" => "提交分析报告",
        "ghidra_decompile" => "反编译函数",
        "findings_list" => "查看已有发现",
        "coverage_list" => "检查分析覆盖情况",
        "view_agent_graph" => "查看协作任务进度",
        _ => name,
    }
}

pub(crate) fn describe(event: &Event, call: Option<&ToolCallRecord>) -> (String, String) {
    let data = &event.data;
    let name = tool_name(field(data, "name"));
    let mut detail = String::new();
    let title = match event.kind.as_str() {
        "tool.started" => format!("正在{name}"),
        "tool.finished" => format!(
            "{}：{name}",
            if field(data, "state") == "failed" {
                "执行失败"
            } else {
                "已返回结果"
            }
        ),
        "run.budget_paused" => {
            detail = readable_reason(field(data, "reason"));
            "分析已暂停".into()
        }
        "controller.started" => "开始调度分析任务".into(),
        "controller.finished" => {
            detail = match field(data, "status") {
                "budget_paused" => "已保存进度，点击“恢复”可继续。",
                "completed" => "分析已完成，可查看总结或导出报告。",
                "failed" => "分析失败，请查看下方错误详情。",
                _ => "本轮调度已结束。",
            }
            .into();
            "分析引擎已结束本轮工作".into()
        }
        "agent.created" | "agent.spawned" => {
            detail = field(data, "task").to_owned();
            "已创建协作 Agent".into()
        }
        "agent.error" => {
            detail = field(data, "message").to_owned();
            "Agent 遇到错误".into()
        }
        "agent.status" => {
            detail = field(data, "error").to_owned();
            "Agent 状态更新".into()
        }
        "model.text" => {
            detail = field(data, "text").to_owned();
            "Agent 思考".into()
        }
        "run.resumed" => "已恢复分析".into(),
        "context.compacted" => "已整理历史上下文并保留证据".into(),
        other => {
            detail = field(data, "reason").to_owned();
            format!("任务记录 · {other}")
        }
    };
    if let Some(call) = call {
        for key in [
            "description",
            "objective",
            "task",
            "query",
            "pattern",
            "path",
            "job_id",
            "evidence_id",
        ] {
            let text = field(&call.arguments, key);
            if !text.is_empty() {
                detail = short(text);
                break;
            }
        }
        if let Some(argv) = call.arguments.get("argv").and_then(Value::as_array) {
            detail = format!(
                "执行命令：{}",
                short(
                    &argv
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            );
        }
        if event.kind == "tool.finished"
            && let Some(result) = &call.result
        {
            for key in ["summary", "answer", "message", "error", "stdout"] {
                let text = field(result, key);
                if !text.is_empty() {
                    if !detail.is_empty() {
                        detail.push('\n');
                    }
                    detail.push_str(&short(text));
                    break;
                }
            }
        }
    }
    (title, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::now;
    #[test]
    fn manual_pause_is_not_reported_as_exhausted_budget() {
        let event = Event {
            seq: 1,
            run_id: "run".into(),
            agent_id: None,
            kind: "run.budget_paused".into(),
            data: serde_json::json!({"reason":"operator paused from desktop"}),
            created_at: now(),
        };
        let (title, detail) = describe(&event, None);
        assert_eq!(title, "分析已暂停");
        assert!(detail.contains("你点击了暂停"));
    }
    #[test]
    fn failed_tools_are_not_presented_as_successful() {
        let event = Event {
            seq: 1,
            run_id: "run".into(),
            agent_id: None,
            kind: "tool.finished".into(),
            data: serde_json::json!({"name":"binary_strings","state":"failed"}),
            created_at: now(),
        };
        assert_eq!(describe(&event, None).0, "执行失败：提取并检索字符串");
        assert!(!technical(&event));
    }
}
