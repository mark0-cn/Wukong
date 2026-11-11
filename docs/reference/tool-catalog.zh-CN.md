# 内置工具目录

[English](tool-catalog.md) · 简体中文

由 `wukong --locale zh-CN tool-catalog` 从工具注册表生成。不要手动编辑。

原生协调工具 `task`、`message`、`wait` 由 Rig 提供；MCP 工具取决于配置。下表列出模型可见的内置业务工具及生命周期可见性，具体执行仍校验目标、证据、会话归属和后端能力。中文说明仅用于文档，不改变模型协议。

| 工具 | 可见角色 | 有副作用 | 用途 |
|---|---|---|---|
| `agent_finish` | analyst / verifier | 是 | 以结构化结果结束子任务，覆盖契约、证据和缺口。summary 仅用于无必需产物的任务；提供 result 时忽略 summary，以 result.answer 为准。活动后代必须先结束。 |
| `analysis_verify` | analyst / verifier | 是 | 仅 verifier：记录独立通用分析判定，包含期望/观察、范围、反例及验证者自己的证据。不提升安全影响；记录不可变并出现在共享笔记中。 |
| `angr_solve` | analyst / verifier | 是 | 对 stdin 校验路径进行有界符号输入还原。提供模块相对成功/失败地址与输入长度，返回候选输入和探索统计的后台 job。简单可逆变换优先直接求逆，大状态空间使用切片或明确约束；必须在真实目标重放候选字节。 |
| `binary_inspect` | analyst / verifier | 否 | 检查本 run 的精确目标、架构、节、导入、符号和缓解机制。 |
| `binary_strings` | analyst / verifier | 否 | 分页搜索可打印字符串，附文件偏移地址引用。 |
| `context_read` | 全部 | 否 | 按字符范围读取同 run 的确切共享文档 JSON，含 amendments。kind/id 来自 context_search；沿 content.next_offset 读取所需完整内容。 |
| `context_search` | 全部 | 否 | 按文本、kind、作者 scope 或 agent_id 搜索同 run 的笔记、待办和可选威胁模型。返回有界预览和 next_offset；用 context_read 读取完整正文及 amendments。 |
| `coverage_list` | 全部 | 否 | 列出本次分析的全部覆盖记录。 |
| `coverage_record` | 全部 | 是 | 记录检查范围、结果及支持证据。needs_follow_up 阻止 root 完成。 |
| `coverage_update` | 全部 | 是 | 更新覆盖记录的结果、笔记和证据。 |
| `crash_replay` | analyst / verifier | 是 | 在干净目录启动持久化崩溃复现 job。 |
| `crash_snapshot` | analyst / verifier | 否 | 从自己拥有且已停止的调试会话获取寄存器、PC/SP、故障地址、指令、栈和模块身份，或检查同 run 的 core。强签名要求实际模块哈希；未知身份保持未解决。 |
| `cyclic_offset` | analyst / verifier | 否 | 在同 run 的 cyclic pattern 中找唯一偏移。必须提供原始 alphabet/n 及内存顺序恰好 n 字节；拒绝不一致生成参数，不猜字节序或隐式截断寄存器。 |
| `debug_close` | analyst / verifier | 是 | 关闭自己的调试会话，或清理同 run 中 owner 已终止的会话。 |
| `debug_command` | analyst / verifier | 是 | 在所属会话执行恰好一条 GDB/MI 命令；稳定请求 ID 防止盲目重放。 |
| `debug_start` | analyst / verifier | 是 | 对暂存目标启动自己拥有的 GDB/MI 会话。 |
| `differential_test` | analyst / verifier | 是 | 在隔离后台 job 中，以有界 stdin 向量比较暂存目标/参考实现。原始输出、哈希、差异保留产物；超时/截断是 inconclusive，不是匹配。 |
| `emulate_run` | analyst / verifier | 否 | 从起始地址用 Unicorn 有界模拟暂存目标：映射 ELF 段和栈，运行到 count 条指令或 PC 到 until 地址，返回寄存器及所需内存 data_base64。目标不原生执行；可用于 VM/手工代码（解释器循环、x87/FPU）而无需自写 Unicorn harness。init_regs 设置前提寄存器，memory 注入字节；watchdog 限制失控循环，未知指令错误包含地址。 |
| `evidence_read` | 全部 | 否 | 读取完整性已核对证据的有界字节范围；非 UTF-8 使用 base64。 |
| `exec_command` | analyst / verifier | 是 | 在 worker 同步执行一条有界 shell 命令，返回 stdout/stderr/退出码。每次使用新临时目录；target 接收 stage_target 路径，并通过 $TARGET 暴露解析后文件（例如 "printf '<payload>' \| $TARGET"）。适合快速检查和 stdin PoC；长任务用 job_start 并重定向日志。files/... 只在直接 argv 项解析，不在 shell 字符串中解析；状态根使用 $WUKONG_STATE_DIR。 |
| `file_get` | analyst / verifier | 否 | 取回同 run 已声明的 worker 文件，并将原始字节保存为证据。 |
| `file_put` | analyst / verifier | 是 | 从 data_base64、UTF-8 text（脚本优先，无需 base64）或已有证据创建不可变 worker 文件。evidence_id 必须是所需内容记录（工具结果 content_evidence_id），而非工具结果信封 ID；传信封会写入信封 JSON 而非目标字节。要执行的程序设置 executable=true。 |
| `finding_file` | 全部 | 是 | 提交有证据的疑似或已复现发现。验证影响需要独立不可变 verifier 记录；critical/high 要求 reproduced 或更高阶段。 |
| `finding_get` | 全部 | 否 | 读取发现及其独立验证记录。 |
| `finding_update` | 全部 | 是 | 更新已有发现；提升影响必须有独立执行证据。 |
| `findings_list` | 全部 | 否 | 列出本分析的发现及记录阶段。 |
| `finish_scan` | root | 是 | 仅 root：子任务和覆盖项处理完毕后结束分析。 |
| `follow_up_agent` | 全部 | 是 | 显式追加工作，以新 generation 重开已完成任务并保留历史；用 recipient_generation 拒绝过时代次的派单。 |
| `function_decompile` | analyst / verifier | 否 | Ghidra：按模块相对偏移反编译函数。 |
| `function_instructions` | analyst / verifier | 否 | Ghidra：分页函数指令及包含调用的控制流边；地址为模块相对且绑定目标 SHA256。可选 target_evidence_id 选择同 run 的基线二进制。 |
| `functions_search` | analyst / verifier | 否 | Ghidra：分页搜索已分析函数。 |
| `fuzz_start` | analyst / verifier | 是 | 暂存目标和非空种子后启动 AFL++ QEMU fuzz。 |
| `fuzz_status` | analyst / verifier | 否 | 观察已绑定 AFL++ job 的真实统计和崩溃文件。 |
| `input_minimize` | analyst / verifier | 是 | 用新观察的模块哈希、PC 偏移及栈帧签名缩减崩溃输入，要求 GDB。allow_signal_only 显式允许较弱信号等价；轮询 job_status 并收集 job_artifacts。 |
| `interactive_close` | analyst / verifier | 是 | 关闭所属交互会话及进程树，也可清理同 run 中 owner 已终止的会话。 |
| `interactive_send` | analyst / verifier | 是 | 向所属交互会话 stdin 写字节并返回新增输出；两次调用间会话保持存活。 |
| `interactive_start` | analyst / verifier | 是 | 启动本 Agent 所属的持久化交互进程（REPL），返回 session_id；worker 路径只在直接 argv 项解析。 |
| `job_artifacts` | analyst / verifier | 否 | 列出并导入同 run job 的原始产物。 |
| `job_cancel` | analyst / verifier | 是 | 请求取消所属的同 run job。 |
| `job_start` | analyst / verifier | 是 | 在配置 worker 启动持久进程，立即返回 job_id。argv 可用 {target} 占位，harness 解析并在隔离 job 暂存；files/... 只在直接 argv 项解析，不在 shell 字符串中解析。job 可读取 WUKONG_STATE_DIR、WUKONG_JOB_DIR。 |
| `job_status` | analyst / verifier | 否 | 观察同 run job，刷新持久状态并导入原始观察。 |
| `libc_identify` | analyst / verifier | 否 | 使用同进程符号泄漏比较最多 16 份同 run ELF libc 候选；省略时只检查 worker libc。保留哈希、Build ID、架构、不一致泄漏和歧义。候选不确认映射身份，无匹配仅描述搜索集合。 |
| `load_skill` | 全部 | 是 | 激活内置或自定义能力/方法。root/analyst/verifier 角色指令自动安装，不能在此加载。初始与动态能力合计最多五个名称。精确正文/哈希持久化，下一新模型边界生效，替代同名初始正文并跨压缩/恢复保留。加载不改变角色、权限或完成条件。 |
| `message_status` | 全部 | 否 | 查看收发消息、投递状态和关联回复。 |
| `mitigation_profile` | analyst / verifier | 否 | 区分二进制声明、worker 支持和所属进程/线程的保护观察；可选 address 检查其真实映射。 |
| `note_save` | 全部 | 是 | 保存关联本任务的共享笔记。 |
| `notes_list` | 全部 | 否 | 列出本分析共享笔记。 |
| `poc_verify` | analyst / verifier | 是 | 在新试验目录独立重放发现，使用 argv/stdin 或宿主准备 launch_spec 的有界网络计划。网络每次新 TCP 连接，捕获/修补 ASLR 值并哈希原始 transcript；不重置服务，部署身份仍未确认。成功以 worker 观察为准。 |
| `py_run` | analyst / verifier | 是 | 一步在 worker 运行 Python 源文，无需 base64/file_put。先语法检查并直接返回编译错误，再创建后台 job（job_status 读输出）。可选 argv 对应 sys.argv[1..]；可选 target 暂存路径附加为最后参数，在隔离 job 自动暂存，用 sys.argv[-1] 读取。多行脚本优先于 file_put+job_start。 |
| `runtime_doctor` | 全部 | 否 | 检查已配置 worker 的真实平台、版本及能力。 |
| `runtime_prepare` | analyst / verifier | 是 | 为提供的 target/libc/loader 创建不可变执行副本并返回直接 argv。三份 ELF 架构须一致。不执行或修改原件；真实映射另行确认。 |
| `runtime_profile` | analyst / verifier | 否 | 检查同 run target/libc/loader 的 SHA256、ELF Build ID 和架构；可选观察所属存活调试会话真实映射。文件候选不代表确认运行时。 |
| `seccomp_analyze` | analyst / verifier | 否 | 解析同 run 原始 classic-BPF seccomp 产物的指令、分支和动作；可具体探测架构/syscall/参数/IP。产物不证明已安装策略，探测不等于完整 allowlist。 |
| `send_message_to_agent` | 全部 | 是 | 入队 information、question、reply、result 或 blocked 通知。information 不打断等待，只有 follow_up_agent 重开已完成任务；queued/included 不证明已处理。 |
| `stage_target` | analyst / verifier | 是 | 将 SHA256 已核对的本 run 原始目标复制到 worker 不可变文件区。 |
| `stop_agent` | 全部 | 是 | 停止自己或后代并记录原因。 |
| `think` | 全部 | 是 | 记录本 Agent 的简短可审计草稿。原文留在工具历史并可能由子任务继承；不是私密通道或节省上下文的机制。共享结论使用 note_save。 |
| `threat_model_amend` | 全部 | 是 | 向共享威胁模型追加持久修订。 |
| `threat_model_get` | 全部 | 否 | 读取共享威胁模型文档。 |
| `threat_model_save` | 全部 | 是 | 创建稳定文档 ID 的共享威胁模型。 |
| `todo_save` | 全部 | 是 | 创建共享待办，默认 pending。 |
| `todo_update` | 全部 | 是 | 更新共享待办正文或状态。 |
| `todos_list` | 全部 | 否 | 列出共享待办。 |
| `verification_record` | analyst / verifier | 是 | 仅 verifier：导入完成的 poc_verify job、原始证据和真实尝试计数。崩溃只验证崩溃行为。 |
| `view_agent_graph` | 全部 | 否 | 列出本分析 Agent 树、scope、状态及结果。 |
| `wait_for_agents` | 全部 | 是 | 按 all_terminal（默认）、all_success 或 any_success 等待选定 Agent，返回 satisfied/blocked 或超时；空列表表示子任务。 |
| `xrefs_query` | analyst / verifier | 否 | Ghidra：查询模块相对偏移的入向/出向引用。 |
