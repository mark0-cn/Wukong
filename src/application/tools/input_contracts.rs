//! Cross-field constraints belong to the same schemas used for selection and admission.
use super::*;

fn exclusive(fields: &[&str]) -> Value {
    json!({"oneOf": fields.iter().map(|field| json!({"required":[field],
        "not":{"anyOf":fields.iter().filter(|other| *other != field)
            .map(|other|json!({"required":[other]})).collect::<Vec<_>>()}})).collect::<Vec<_>>()})
}

pub(super) fn schema(name: &str, mut schema: Value) -> Value {
    let constraint = match name {
        "resource_read" => json!({"oneOf":[
            {"properties":{"kind":{"const":"job"},"stream":{"enum":["stdout","stderr"]}}},
            {"properties":{"kind":{"enum":["debugger","interactive"]},"stream":{"const":"transcript"}}}
        ]}),
        "file_put" => exclusive(&["text", "data_base64", "evidence_id"]),
        "crash_snapshot" => exclusive(&["session_id", "core"]),
        "job_start" | "interactive_start" => json!({"oneOf":[
            {"required":["argv"],"properties":{"argv":{"minItems":1}},"not":{"required":["launch_spec"]}},
            {"required":["launch_spec"],"not":{"anyOf":[{"required":["argv"]},{"required":["env"]},{"required":["cwd"]},{"required":["timeout_secs"]},{"required":["executable_inputs"]}]}}
        ]}),
        "debug_start" => json!({"oneOf":[
            {"required":["target"],"properties":{"mode":{"const":"launch"}},"not":{"anyOf":[{"required":["launch_spec"]},{"required":["dump"]}]}},
            {"required":["launch_spec"],"properties":{"mode":{"const":"launch"}},"not":{"anyOf":[{"required":["target"]},{"required":["args"]},{"required":["dump"]}]}},
            {"required":["mode","target","dump"],"properties":{"mode":{"const":"dump"}},"not":{"anyOf":[{"required":["launch_spec"]},{"required":["args"]},{"required":["input"]}]}}
        ]}),
        "debug_control" => json!({"oneOf":[
            {"properties":{"action":{"const":"breakpoint_add"}},"allOf":[exclusive(&["address","symbol"])],"not":{"required":["breakpoint_id"]}},
            {"properties":{"action":{"enum":["breakpoint_remove","breakpoint_enable","breakpoint_disable"]}},"required":["breakpoint_id"],"not":{"anyOf":[{"required":["address"]},{"required":["symbol"]}]}},
            {"properties":{"action":{"enum":["run","continue","interrupt","step","next","finish"]}},"not":{"anyOf":[{"required":["address"]},{"required":["symbol"]},{"required":["breakpoint_id"]}]}}
        ]}),
        "debug_inspect" => json!({"allOf":[
            {"if":{"properties":{"kind":{"const":"memory"}}},"then":{"required":["address"]}},
            {"if":{"not":{"properties":{"kind":{"const":"events"}}}},"then":{"not":{"required":["offset"]}}}
        ]}),
        "todo_update" => json!({"anyOf":[{"required":["body"]},{"required":["status"]}]}),
        _ => return schema,
    };
    schema["allOf"] = json!([constraint]);
    schema
}

#[cfg(test)]
mod tests {
    use super::*;
    fn valid(name: &str, args: Value) -> bool {
        let spec = builtins()
            .iter()
            .find(|s| s.definition.name.as_str() == name)
            .unwrap();
        validated_arguments(args, &spec.definition.parameters).is_ok()
    }
    #[test]
    fn rejects_ambiguous_or_incomplete_execution_before_dispatch() {
        for (name, args) in [
            (
                "file_put",
                json!({"path":"files/x","text":"x","data_base64":"eA=="}),
            ),
            ("file_put", json!({"path":"files/x"})),
            (
                "job_start",
                json!({"launch_spec":"files/l.json","argv":["files/x"]}),
            ),
            ("interactive_start", json!({"argv":[]})),
            ("crash_snapshot", json!({"session_id":"s","core":"files/c"})),
            ("debug_start", json!({"mode":"dump","target":"files/x"})),
            (
                "debug_control",
                json!({"session_id":"s","action":"breakpoint_add"}),
            ),
            (
                "debug_control",
                json!({"session_id":"s","action":"continue","symbol":"main"}),
            ),
            ("todo_update", json!({"id":"t"})),
            ("agent_finish", json!({"summary":"old shorthand"})),
            (
                "resource_read",
                json!({"kind":"job","resource_id":"j","stream":"transcript"}),
            ),
            (
                "resource_read",
                json!({"kind":"interactive","resource_id":"s","stream":"stdout"}),
            ),
            (
                "lab_restore",
                json!({"vm":"old","lab_id":"old","generation":1}),
            ),
        ] {
            assert!(!valid(name, args), "{name}");
        }
        assert!(valid(
            "interactive_start",
            json!({"launch_spec":"files/l.json"})
        ));
        assert!(valid(
            "debug_control",
            json!({"session_id":"s","action":"breakpoint_add","symbol":"main"})
        ));
        assert!(valid("file_put", json!({"path":"files/x","text":""})));
    }
}
