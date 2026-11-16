# Agent 工具接口

[English](agent-tools.md) · 简体中文

[生成的工具目录](tool-catalog.zh-CN.md) 描述已注册工具。模型看到的工具取决于角色、明确的任务授权、已选分组和实际观测的后端支持。工具选择不能授予权限。`contract.tool_grants` 接受 `raw_debugger` 和 `windows_lab`；root 可以向任务授予这些权限，子任务只能继续委派自己已有的权限。Skills 不授予权限。Lab 操作还要求 VM 与任务指定环境一致。

`tool_catalog` 分页报告分组和可用性。`tool_select` 替换下一次模型请求的可选分组。核心工具和资源观察／清理工具始终保留。初始选择包含静态分析与类型化调试。模糊测试、符号执行和配置的 MCP 工具需选择 `fuzzing`、`symbolic` 或 `external`；原始调试器和 Lab 分组还需要对应任务授权。每次调用都会再次检查就绪状态。

## 接口迁移

旧名称和相互竞争的输入形式均被拒绝，不提供别名。

| 移除的接口 | 当前接口 |
|---|---|
| `fuzz_status` | `job_status` |
| `runtime_doctor` | `environment_doctor`；environment_id 默认使用任务指定环境 |
| 原生 `message` / `wait` | `send_message_to_agent` / `wait_for_agents`；保留原生 `task` |
| `notes_list`、`todos_list`、`threat_model_get` | `context_search` 后使用 `context_read` |
| `think` | 用 `note_save` 保存简短共享结论 |
| 模型 `lab_prune` | 只保留操作员 CLI |
| `bundle_stage`、`runtime_prepare` | `launch_prepare`，source.kind 为 bundle 或 linux_runtime |
| `target_evidence_id` | 所有静态工具统一使用 `target_ref={kind:"evidence",evidence_id:"..."}` |
| 静态地址裸整数 | 绑定目标的 module_relative AddressRef |
| `artifact_stage.artifact_id` | `evidence_id` |
| 完成参数 `summary` | 结构化 result 中的 `result.answer` |
| Lab vm/lab_id/generation 输入组合 | `lab_prepare` 返回的所属不透明 `lab_ref` |
| `interactive_send.data_base64` | 带类型的 `source`：text、base64 或 evidence |

只有 verifier 可以调用 `analysis_verify`、`poc_verify` 和 `verification_record`。业务层在认证及提升发现阶段时也会检查 verifier 角色。

## 执行与结果

`launch_prepare` 从 primary、bundle、已暂存文件／依赖或 Linux target/libc/loader 创建绑定哈希的不可变目录布局。在此设置 args、cwd、环境、输入模式和时限。job_start、interactive_start 或 debug_start 复用返回的 `launch_spec`，省略 argv／target／环境／时限覆盖参数。schema 在分派给 worker 前拒绝冲突选项。准备记录身份，实际 worker 执行才构成验收证据。

`file_put` 必须且只能提供 text／base64／evidence 中一种来源。`debug_start` 分离启动与 dump 输入，`debug_control` 按动作要求相应字段，`crash_snapshot` 必须且只能提供 session 或 core。`artifact_validate.interpreter` 为 python3 或 bash。交互输入最多 64 KiB。后端声明 debug.events 时，`debug_inspect` 的 kind=events 提供有界事件快照；其他后端明确报告不可用。

`findings_list` 和 `coverage_list` 返回过滤后的摘要，每页最多 20 条，使用 after 继续 next_cursor。`evidence_search` 按产出者、目标／内容哈希、作业、来源或媒体类型检索本次运行证据。`resource_list` 只从控制器记录列出调用者拥有的作业和会话，包含最近观测状态；它不会接管未知 worker 资源或授予清理权限。

`resource_read` 返回有界输出分块、证据 ID 和 next_offset。at_snapshot_end 表示当前观察中没有更多字节；eof 还要求资源已终止。后端可能只在完成后公布输出流。流缺失会明确报错，不会当作空输出。证据内容仍可通过 evidence_read 读取，不依赖 worker 资源。

`differential_test` 用 1–8 个本次运行的输入证据比较两个真实准备的 LaunchSpec，每个输入最多 64 KiB，两份清单均须记录 1–10 秒时限。明确选择 exact_output、exit_status 或 stdout_json 及预期退出码。作业顺序运行，以支持单资源 worker。保留清单、输入、作业状态和输出证据。超时、输出缺失／截断、非预期退出或无效 JSON 均为 inconclusive。聚合调用预算为 300 秒；未完成作业仍保留所属关系，可通过资源清单发现。
