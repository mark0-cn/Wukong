//! Native Rig tools and legacy event names, outside the current builtin catalog.
pub(super) fn title(name: &str) -> Option<&'static str> {
    match name {
        "task" => Some("分派分析子任务"),
        "message" => Some("发送协作消息"),
        "wait" => Some("等待子任务结果"),
        "tool_search" => Some("查找分析工具"),
        "tools_load" => Some("准备分析工具"),
        "functions_list" => Some("查找程序函数"),
        "ghidra_decompile" => Some("反编译函数"),
        "write_stdin" => Some("与运行中的进程交互"),
        "file_list" => Some("列出工作区文件"),
        _ => None,
    }
}
