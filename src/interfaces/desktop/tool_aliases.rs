//! Native Rig tools and legacy event names, outside the current builtin catalog.
pub(super) fn title(name: &str) -> Option<&'static str> {
    match name {
        "task" => Some("tool-task"),
        "message" => Some("tool-message"),
        "wait" => Some("tool-wait"),
        "tool_search" => Some("tool-tool-search"),
        "tools_load" => Some("tool-tools-load"),
        "functions_list" => Some("tool-functions-list"),
        "ghidra_decompile" => Some("tool-ghidra-decompile"),
        "write_stdin" => Some("tool-write-stdin"),
        "file_list" => Some("tool-file-list"),
        _ => None,
    }
}
