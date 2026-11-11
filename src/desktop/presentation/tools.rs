//! Bounded factual tool views; raw results and evidence remain authoritative.
use super::*;

pub fn result_summary(value: &Value) -> DisplayText {
    summary(value, 0)
}
fn summary(value: &Value, depth: usize) -> DisplayText {
    if depth > 6 {
        return DisplayText::default();
    }
    match value {
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .map(|value| summary(&value, depth + 1))
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| excerpt(text, 600).into()),
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
                return excerpt(text, 600).into();
            }
            // These are technical result fields, not translated raw evidence.
            let facts = ["format", "architecture", "arch", "status", "exit_code"]
                .into_iter()
                .filter_map(|key| {
                    map.get(key)
                        .filter(|v| v.is_string() || v.is_number())
                        .map(|value| {
                            format!(
                                "{key}: {}",
                                value
                                    .as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| value.to_string())
                            )
                        })
                })
                .collect::<Vec<_>>();
            if !facts.is_empty() {
                return facts.join(" · ").into();
            }
            for key in ["matches", "functions", "strings", "findings", "results"] {
                if let Some(Value::Array(items)) = map.get(key) {
                    return UiMessage::new(format!("result-{key}"))
                        .arg("count", items.len())
                        .into();
                }
            }
            for key in ["result", "content", "output", "value", "analysis", "data"] {
                if let Some(value) = map.get(key) {
                    let text = summary(value, depth + 1);
                    if !text.is_empty() {
                        return text;
                    }
                }
            }
            DisplayText::default()
        }
        _ => DisplayText::default(),
    }
}
pub fn tool_display(
    call: &ToolCallRecord,
    agent: Option<&AgentSummary>,
    target_name: &str,
) -> ToolDisplay {
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
            "target",
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
    let payload = call.result.as_ref().map(payload);
    let target = payload
        .and_then(|value| value.get("function"))
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(|name| excerpt(name, 180))
        .or_else(|| {
            call.arguments["address"]
                .as_u64()
                .map(|address| format!("module+0x{address:x}"))
        })
        .unwrap_or_else(|| {
            if target.is_empty()
                && matches!(
                    call.name.as_str(),
                    "binary_inspect" | "stage_target" | "binary_strings"
                )
            {
                target_name.into()
            } else {
                target
            }
        });
    let purpose = text(&call.arguments, &["description", "objective", "task"])
        .map(|text| excerpt(text, 240))
        .unwrap_or_default();
    let summary = call
        .result
        .as_ref()
        .map(result_summary)
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| match state {
            "completed" | "failed" | "uncertain" | "paused" | "interrupted" => {
                UiMessage::new(format!("result-{state}")).into()
            }
            _ => DisplayText::default(),
        });
    let preview = if call.name == "function_decompile" && call.state == CallState::Completed {
        payload
            .and_then(|data| data["code"].as_str().map(|code| (data, code)))
            .zip(call.evidence_id.as_ref())
            .map(|((data, code), evidence)| {
                let text = code.lines().take(12).collect::<Vec<_>>().join("\n");
                let text = text.chars().take(1600).collect::<String>();
                CodePreview {
                    language: "c".into(),
                    truncated: data["truncated"].as_bool().unwrap_or(false)
                        || text.chars().count() < code.chars().count(),
                    text,
                    evidence_id: evidence.clone(),
                }
            })
            .filter(|preview| !preview.text.trim().is_empty())
    } else {
        None
    };
    let summary = if call.name == "function_decompile"
        && call.state == CallState::Completed
        && let Some(data) = payload
        && data["code"].is_string()
    {
        UiMessage::new("result-decompiled")
            .arg("name", target.clone())
            .arg(
                "chars",
                data["code_chars"].as_u64().unwrap_or_else(|| {
                    data["code"].as_str().unwrap_or_default().chars().count() as u64
                }),
            )
            .into()
    } else {
        summary
    };
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
        preview,
        job: None,
    }
}

// Follow known transport/analysis envelopes only; never walk arbitrary raw data.
fn payload(mut value: &Value) -> &Value {
    for _ in 0..8 {
        let next = ["value", "result", "analysis", "data"]
            .into_iter()
            .find_map(|key| value.get(key).filter(|item| item.is_object()));
        match next {
            Some(next) => value = next,
            None => break,
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, arguments: Value, result: Value) -> ToolCallRecord {
        ToolCallRecord {
            id: "call".into(),
            run_id: "run".into(),
            agent_id: "analyst".into(),
            name: name.into(),
            arguments,
            result: Some(json!({"value":{"result":result}})),
            state: CallState::Completed,
            side_effecting: false,
            evidence_id: Some("raw-evidence".into()),
            created_at: now(),
            updated_at: now(),
        }
    }

    #[test]
    fn real_numeric_addresses_and_ghidra_envelopes_render_without_model_narration() {
        let mut call = call(
            "function_decompile",
            json!({"address":4096}),
            json!({"analysis":{"data":{
            "function":{"name":"parse_record"},"code":"int parse_record(void) { return 0; }","code_chars":35,"truncated":false}}}),
        );
        let view = tool_display(&call, None, "parser.elf");
        assert_eq!(view.target, "parse_record");
        assert!(
            view.summary
                .render(crate::i18n::Locale::EnUs)
                .starts_with("Decompiled parse_record")
        );
        assert_eq!(view.preview.unwrap().evidence_id, "raw-evidence");
        call.result = None;
        assert_eq!(
            tool_display(&call, None, "parser.elf").target,
            "module+0x1000"
        );
    }

    #[test]
    fn preview_requires_successful_evidence_and_is_bounded_on_unicode_boundaries() {
        let code = "/* 中文 */\n".repeat(1000);
        let mut call = call(
            "function_decompile",
            json!({"address":4096}),
            json!({"analysis":{"data":{"code":code}}}),
        );
        let preview = tool_display(&call, None, "parser.elf").preview.unwrap();
        assert!(preview.truncated);
        assert!(preview.text.lines().count() <= 12);
        assert!(preview.text.chars().count() <= 1600);
        call.state = CallState::Failed;
        assert!(tool_display(&call, None, "parser.elf").preview.is_none());
        call.state = CallState::Completed;
        call.evidence_id = None;
        assert!(tool_display(&call, None, "parser.elf").preview.is_none());
    }

    #[test]
    fn empty_target_arguments_use_the_run_identity_and_nested_facts_are_preserved() {
        let call = call(
            "binary_inspect",
            json!({}),
            json!({"format":"ELF","architecture":"x86_64"}),
        );
        let view = tool_display(&call, None, "parser.elf");
        assert_eq!(view.target, "parser.elf");
        assert!(
            view.summary
                .render(crate::i18n::Locale::EnUs)
                .contains("x86_64")
        );
    }
}
