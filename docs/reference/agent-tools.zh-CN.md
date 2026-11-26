# Agent 工具接口

[English](agent-tools.md) · 简体中文

[生成的工具目录](tool-catalog.zh-CN.md) 描述已注册工具。模型看到的工具取决于角色、已选分组和实际观测的后端支持。工具选择和 Skills 不改变角色。原始调试器和 Windows Lab 操作对目录允许的角色默认开启。Lab 操作还要求 VM 与任务指定环境一致。

`tool_catalog` 分页报告分组和可用性。`tool_select` 替换下一次模型请求的可选分组。核心工具和资源观察／清理工具始终保留。初始选择包含静态分析与类型化调试。模糊测试、符号执行、原始调试器、Lab 和配置的 MCP 工具分别需选择 `fuzzing`、`symbolic`、`raw_debugger`、`windows_lab` 或 `external`；这只改变模型上下文。每次调用都会再次检查就绪状态。

只有 verifier 可以调用 `analysis_verify`、`poc_verify` 和 `verification_record`。业务层在认证及提升发现阶段时也会检查 verifier 角色。

## 执行与结果

`launch_prepare` 从 primary、bundle、已暂存文件／依赖或 Linux target/libc/loader 创建绑定哈希的不可变目录布局。在此设置 args、cwd、环境、输入模式和时限。目录包可用 `source.entry` 和 `source.members` 在上传前选择入口及成员子集。job_start、interactive_start 或 debug_start 复用返回的 `launch_spec`，省略 argv／target／环境／时限覆盖参数。schema 在分派给 worker 前拒绝冲突选项。准备记录身份，实际 worker 执行才构成验收证据。

`file_put` 必须且只能提供 text／base64／evidence 中一种来源。`debug_start` 分离启动与 dump 输入，`debug_control` 按动作要求相应字段，`crash_snapshot` 必须且只能提供 session 或 core。`artifact_validate.interpreter` 为 python3 或 bash。交互输入最多 64 KiB。后端声明 debug.events 时，`debug_inspect` 的 kind=events 提供有界事件快照；其他后端明确报告不可用。

`findings_list` 和 `coverage_list` 返回过滤后的摘要，每页最多 20 条，使用 after 继续 next_cursor。`evidence_search` 按产出者、目标／内容哈希、作业、来源或媒体类型检索本次运行证据。`resource_list` 只从控制器记录列出调用者拥有的作业和会话，包含最近观测状态；它不会接管未知 worker 资源或授予清理权限。

普通 `coverage_record` 按调用者及经过空白规范化、区分大小写的 scope/kind 更新或插入一行。root 还可通过 `supersedes` 提供最多 128 个不同的同 run 覆盖 ID，这些行的规范化 scope/kind 必须匹配，且 owner 必须已终止。root 的解决记录与旧行转为 `superseded` 在同一事务中提交。被取代的行保留 ID、备注和证据供审计，但不阻塞完成，也不是当前结论；被反驳的推断必须在 root 解决记录中更正。非 root 调用、活跃 owner、跨 run 引用和 scope/kind 不匹配均被拒绝。

`resource_read` 返回有界输出分块、证据 ID 和 next_offset。at_snapshot_end 表示当前观察中没有更多字节；eof 还要求资源已终止。后端可能只在完成后公布输出流。流缺失会明确报错，不会当作空输出。证据内容仍可通过 evidence_read 读取，不依赖 worker 资源。

`differential_test` 用 1–8 个本次运行的输入证据比较两个真实准备的 LaunchSpec，每个输入最多 64 KiB，两份清单均须记录 1–10 秒时限。明确选择 exact_output、exit_status 或 stdout_json 及预期退出码。作业顺序运行，以支持单资源 worker。保留清单、输入、作业状态和输出证据。超时、输出缺失／截断、非预期退出或无效 JSON 均为 inconclusive。聚合调用预算为 300 秒；未完成作业仍保留所属关系，可通过资源清单发现。

`trace_run` 使用不可变 LaunchSpec 和同 run 的单个输入，以普通持久 job 启动声明式函数 entry/return 或调用点追踪。`trace_compare` 接收两组调用者所有的 `trace/run.json` + `trace/events.jsonl` 证据，离线比较规范化观察，不连接 worker。只有完整、可比、已 settled、证据就绪且非空的 trace 才能返回 match/mismatch。schema 和 Linux/ABI 限制见[动态函数追踪与离线差分](../user/dynamic-trace.zh-CN.md)。
