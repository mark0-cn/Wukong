# 内置工具目录

[English](tool-catalog.md) · 简体中文

由 `wukong --locale zh-CN tool-catalog` 从工具注册表生成。不要手动编辑。

原生协调工具 `task`、`message`、`wait` 由 Rig 提供；MCP 工具取决于配置。下表列出内置业务工具的角色边界及后端要求（保留技术标识）。实际模型列表还会根据环境预检过滤；执行仍校验目标、证据、会话归属和当前后端能力。中文说明仅用于文档，不改变模型协议。

| 工具 | 可见角色 | 有副作用 | 后端要求 | 用途 |
|---|---|---|---|---|
| `agent_finish` | analyst / verifier | 是 | controller | 以结构化结果结束子任务，覆盖契约、证据和缺口。summary 仅用于无必需产物的任务；提供 result 时忽略 summary，以 result.answer 为准。活动后代必须先结束。 |
| `analysis_verify` | verifier | 是 | controller | 仅 verifier：记录逐项声明检查、限制及验证者自己的证据。supported 只覆盖所列检查；复现检查必须关联精确产物和带状态证据的验证者作业。不认证整份报告，不提升安全影响。记录为不可变共享笔记。 validator_integration 检查绑定真实 --verify 任务与共用采集/判定路径的独立代码审查。自动解析自有任务原始状态证据；错误按索引标明无效检查。 |
| `angr_solve` | analyst / verifier | 是 | angr.solve | 对 stdin 校验路径进行有界符号输入还原。提供模块相对成功/失败地址与输入长度，返回候选输入和探索统计的后台 job。简单可逆变换优先直接求逆，大状态空间使用切片或明确约束；必须在真实目标重放候选字节。 |
| `artifact_stage` | analyst / verifier | 是 | file.upload.begin + file.upload.write + file.upload.commit | 通过有界可恢复传输，将同一 run 中通过哈希校验的证据制品传入分配的环境，返回 worker 文件和原始证据 ID。 |
| `artifact_validate` | verifier | 是 | Linux self_check_v1 + py_run + file.put + job.status + job.artifacts | 仅 verifier 可用：对 self_check_v1 Python 或 Shell 脚本独立测试正常、异常退出、信号、超时、缺失、截断和矛盾观测。绑定生产者内容证据、verifier 成功执行同一目标 --verify 的基线任务及导入的 baseline.json。启动可信有界驱动任务；观察结果后才能声明验收通过。覆盖模拟判定，不证明目标行为或采集路径连接。 |
| `binary_inspect` | analyst / verifier | 否 | controller | 检查本 run 的精确目标、架构、节、导入、符号和缓解机制。 |
| `binary_strings` | analyst / verifier | 否 | controller | 分页搜索可打印字符串，附文件偏移地址引用。 |
| `bundle_evidence` | analyst / verifier | 否 | controller | 把一个已校验 bundle 成员导入为不可变证据，以便用 target_evidence_id 检查或反编译。路径相对于被分析目录。 |
| `bundle_manifest` | analyst / verifier | 否 | controller | 返回目录 bundle 清单：自动哈希的成员、推断的 executable/library/data 类型、PE 导入和跨成员导入边。无需用户标注角色；entry 是推断出的启动候选。 |
| `bundle_stage` | analyst / verifier | 是 | file.upload.begin + file.upload.write + file.upload.commit + file.put | 把全部 bundle 成员 staging 到同一个 worker 目录，创建以该目录为 cwd 的 LaunchSpec，并返回 launch_spec 路径和成员身份。随后用 launch_spec 且不带 argv/timeout_secs/cwd/env 调用 job_start，让推断入口带着同目录 DLL 运行。timeout_secs（默认 300）写入 LaunchSpec；最多 64 个成员。 |
| `context_read` | 全部 | 否 | controller | 按字符范围读取同 run 的确切共享文档 JSON，含 amendments。kind/id 来自 context_search；沿 content.next_offset 读取所需完整内容。 |
| `context_search` | 全部 | 否 | controller | 按文本、kind、作者 scope 或 agent_id 搜索同 run 的笔记、待办和可选威胁模型。返回有界预览和 next_offset；用 context_read 读取完整正文及 amendments。 |
| `coverage_list` | 全部 | 否 | controller | 列出本次分析的全部覆盖记录。 |
| `coverage_record` | 全部 | 是 | controller | 记录检查范围、结果及支持证据。needs_follow_up 阻止 root 完成。 |
| `coverage_update` | 全部 | 是 | controller | 更新覆盖记录的结果、笔记和证据。 |
| `crash_replay` | analyst / verifier | 是 | crash.replay | 在干净目录启动持久化崩溃复现 job。 |
| `crash_snapshot` | analyst / verifier | 否 | core.snapshot | 从自己拥有且已停止的调试会话获取寄存器、PC/SP、故障地址、指令、栈和模块身份，或检查同 run 的 core。强签名要求实际模块哈希；未知身份保持未解决。 |
| `cyclic_offset` | analyst / verifier | 否 | cyclic.offset | 在同 run 的 cyclic pattern 中找唯一偏移。必须提供原始 alphabet/n 及内存顺序恰好 n 字节；拒绝不一致生成参数，不猜字节序或隐式截断寄存器。 |
| `debug_close` | analyst / verifier | 是 | debug.close | 关闭自己的调试会话，或清理同 run 中 owner 已终止的会话。 |
| `debug_command` | analyst / verifier | 是 | debug.command | 在所属会话执行一条通过策略校验的 GDB/MI 命令。隔离会话接受不含函数调用的简单表达式；优先使用结构化检查/控制。稳定请求 ID 防止盲目重放。 |
| `debug_control` | analyst / verifier | 是 | debug.control | 使用类型化的断点和执行动作控制自己拥有的实时调试器。Dump 会话拒绝执行控制；稳定请求 ID 防止盲目重放。 |
| `debug_inspect` | analyst / verifier | 否 | debug.inspect | 在自己拥有的调试会话中，有界检查寄存器、栈、内存、模块、线程、指令、异常或执行状态；完整引擎记录保留为证据。 |
| `debug_start` | analyst / verifier | 是 | debug.start | 对暂存目标启动自己拥有的调试会话，或检查单独暂存的 Dump。引擎选择不会回退到其他平台。 |
| `differential_test` | analyst / verifier | 是 | job.start | 在隔离后台 job 中，以有界 stdin 向量比较暂存目标/参考实现。原始输出、哈希、差异保留产物；超时/截断是 inconclusive，不是匹配。 |
| `emulate_run` | analyst / verifier | 否 | emu.run | 从起始地址用 Unicorn 有界模拟暂存目标：映射 ELF 段和栈，运行到 count 条指令或 PC 到 until 地址，返回寄存器及所需内存 data_base64。目标不原生执行；可用于 VM/手工代码（解释器循环、x87/FPU）而无需自写 Unicorn harness。init_regs 设置前提寄存器，memory 注入字节；watchdog 限制失控循环，未知指令错误包含地址。 |
| `environment_doctor` | 全部 | 否 | explicit probe | 探测已登记环境的实际平台和工具；子任务只能探测分配给自己的环境。 |
| `environment_list` | 全部 | 否 | controller | 列出已登记的分析环境，不建立连接；配置存在不代表环境已就绪。 |
| `evidence_read` | 全部 | 否 | controller | 读取完整性已核对证据的有界字节范围；非 UTF-8 使用 base64。 |
| `exec_command` | analyst / verifier | 是 | exec.run | 在 worker 同步执行一条有界 shell 命令，返回 stdout/stderr/退出码。每次使用新临时目录；target 接收 stage_target 路径，并通过 $TARGET 暴露解析后文件（例如 "printf '<payload>' \| $TARGET"）。适合快速检查和 stdin PoC；长任务用 job_start 并重定向日志。files/... 只在直接 argv 项解析，不在 shell 字符串中解析；状态根使用 $WUKONG_STATE_DIR。 |
| `file_get` | analyst / verifier | 否 | file.stat + file.read | 取回同 run 已声明的 worker 文件，并将原始字节保存为证据。 |
| `file_put` | analyst / verifier | 是 | file.put | 通过 base64 字节（data_base64）、UTF-8 文本（text，脚本首选，无需 base64 编码）或已有证据创建不可变工作区文件。evidence_id 必须是内容证据 ID（工具结果的 content_evidence_id），不能使用工具结果信封，否则写入的是信封 JSON。需作为程序执行时使用 executable=true。path_policy=versioned 接受 solve.py 等逻辑文件名并存储内容哈希版本；复用返回的 files/... 路径。小改动使用 file_revise。 |
| `file_revise` | analyst / verifier | 是 | file.put | 从原始 UTF-8 内容证据和 1–64 个按顺序唯一匹配的文本替换创建不可变修订。name 是逻辑文件名；后续复用返回的哈希绑定工作区路径和 content_evidence_id。绝不覆盖原件。小改动优先使用局部修订，避免重传整份脚本。 |
| `finding_file` | 全部 | 是 | controller | 提交有证据的疑似或已复现发现。验证影响需要独立不可变 verifier 记录；critical/high 要求 reproduced 或更高阶段。 |
| `finding_get` | 全部 | 否 | controller | 读取发现及其独立验证记录。 |
| `finding_update` | 全部 | 是 | controller | 更新已有发现；提升影响必须有独立执行证据。 |
| `findings_list` | 全部 | 否 | controller | 列出本分析的发现及记录阶段。 |
| `finish_scan` | root | 是 | controller | 仅 root：子任务和覆盖项处理完毕后结束分析。 |
| `follow_up_agent` | 全部 | 是 | controller | 显式追加工作，以新 generation 重开已完成任务并保留历史；用 recipient_generation 拒绝过时代次的派单。 |
| `function_decompile` | analyst / verifier | 否 | controller Ghidra or ghidra_query | Ghidra：使用绑定目标的 module_relative AddressRef（推荐）或兼容的字节偏移反编译。复制 binary_inspect 返回的地址；链接时 VA 必须减去 image_base。文件偏移、运行时地址和页号不是模块偏移。 |
| `function_instructions` | analyst / verifier | 否 | controller Ghidra or ghidra_query | Ghidra：使用绑定目标的 module_relative AddressRef 或兼容的模块相对字节偏移列出函数指令；不接受链接时 VA、文件偏移或页号。使用 offset/limit 分页。 |
| `functions_search` | analyst / verifier | 否 | controller Ghidra or ghidra_query | Ghidra：分页搜索已分析函数。 |
| `fuzz_start` | analyst / verifier | 是 | fuzz.start | 暂存目标和非空种子后启动 AFL++ QEMU fuzz。 |
| `fuzz_status` | analyst / verifier | 否 | job.status | 观察已绑定 AFL++ job 的真实统计和崩溃文件。 |
| `input_minimize` | analyst / verifier | 是 | input.minimize | 用新观察的模块哈希、PC 偏移及栈帧签名，经 worker 调试器缩减崩溃输入（Linux 为 GDB，Windows 为经 WinDbg MCP 的 DbgEng）。allow_signal_only 显式允许较弱信号等价；轮询 job_status 并收集 job_artifacts。 |
| `interactive_close` | analyst / verifier | 是 | interact.close | 关闭所属交互会话及进程树，也可清理同 run 中 owner 已终止的会话。 |
| `interactive_send` | analyst / verifier | 是 | interact.write | 向所属交互会话 stdin 写字节并返回新增输出；两次调用间会话保持存活。 |
| `interactive_start` | analyst / verifier | 是 | interact.start | 启动本 Agent 所属的持久化交互进程（REPL），返回 session_id；worker 路径只在直接 argv 项解析。 |
| `job_artifacts` | analyst / verifier | 否 | job.artifacts | 列出并导入同 run job 的原始产物。 |
| `job_cancel` | analyst / verifier | 是 | job.cancel | 请求取消所属的同 run job。 |
| `job_start` | analyst / verifier | 是 | job.start | 在配置的 worker 启动持久进程任务，立即返回 job_id。使用 argv，或已暂存的 launch_spec（记录目标、文件、参数、cwd、环境和输入模式）；input 选择已暂存数据文件。有 launch_spec 时省略 argv/env/cwd 覆盖并保留其超时。原始 argv 与 executable_inputs 可使用 {target}；files/... 引用只解析直接参数，不解析 shell 字符串。隔离 Linux 中 argv[0] 和显式 executable_inputs 获得可执行副本，其余文件参数只读。结果包含 cwd、输入哈希与权限。可通过环境变量 WUKONG_STATE_DIR 和 WUKONG_JOB_DIR 读取路径。 |
| `job_status` | analyst / verifier | 否 | job.status | 观察同 run job，刷新持久状态并导入原始观察。 |
| `lab_install` | analyst / verifier | 是 | controller | 通过 lab 管理通道运行包中哈希固定的 install.ps1。需要配置的密码环境变量；写入不可变安装回执。 |
| `lab_prepare` | analyst / verifier | 是 | controller | 准备已登记的 Windows lab：生成规格中经哈希校验的包、校验安装计划、按需创建基线并拍实验快照。返回含 lab_id 和 generation 的租约；后续目标工具使用该租约。 |
| `lab_prune` | analyst / verifier | 是 | controller | 删除未使用的 lab 前缀快照，保留基线和活动实验快照。逐个报告失败，绝不触碰无关快照。 |
| `lab_restore` | analyst / verifier | 是 | controller | 把已登记 lab 恢复到基线，按需启动并清理实验快照。核对未知 run/安装结果；需要活动 lab_id 与 generation。 |
| `lab_run` | analyst / verifier | 是 | controller | 通过已登记 Windows worker 运行实验已校验包中的一个产物；可选 stdin 来自有界证据。写入不可变 run 回执，传输失败标记为 unknown；恢复基线来核对。 |
| `lab_status` | analyst / verifier | 否 | controller | 检查已登记的 Windows lab VM：宿主可达性、快照、阶段以及最近一次 run/安装。只读，不修改 worker 或宿主。 |
| `libc_identify` | analyst / verifier | 否 | libc.identify | 使用同进程符号泄漏比较最多 16 份同 run ELF libc 候选；省略时只检查 worker libc。保留哈希、Build ID、架构、不一致泄漏和歧义。候选不确认映射身份，无匹配仅描述搜索集合。 |
| `load_skill` | 全部 | 是 | controller | 激活内置或自定义能力/方法。root/analyst/verifier 角色指令自动安装，不能在此加载。初始与动态能力合计最多五个名称。精确正文/哈希持久化，下一新模型边界生效，替代同名初始正文并跨压缩/恢复保留。加载不改变角色、权限或完成条件。 |
| `message_status` | 全部 | 否 | controller | 查看收发消息、投递状态和关联回复。 |
| `mitigation_profile` | analyst / verifier | 否 | runtime.profile | 区分二进制声明、worker 支持和所属进程/线程的保护观察；可选 address 检查其真实映射。 |
| `note_save` | 全部 | 是 | controller | 保存关联本任务的共享笔记。 |
| `notes_list` | 全部 | 否 | controller | 列出本分析共享笔记。 |
| `poc_verify` | verifier | 是 | poc.verify | 在新试验目录独立重放发现，使用 argv/stdin 或宿主准备 launch_spec 的有界网络计划。网络每次新 TCP 连接，捕获/修补 ASLR 值并哈希原始 transcript；不重置服务，部署身份仍未确认。原生 Windows worker 仅支持 argv/stdin 重放；成功以 worker 观察为准。 |
| `py_run` | analyst / verifier | 是 | py.run | 一步在 worker 运行 Python。小脚本直接用 source 文本；大脚本先用 file_put 的 text 上传，再把返回路径作为 source_file（source/source_file 只能二选一）。worker 先做语法检查并直接返回编译错误，再创建后台 job（用 job_status 读输出）。可选 argv 对应 sys.argv[1..]；可选 target 暂存路径追加为最后参数，在隔离 job 中作为只读可执行副本，用 sys.argv[-1] 读取。 |
| `runtime_doctor` | 全部 | 否 | explicit probe | 检查已配置 worker 的真实平台、版本及能力。 |
| `runtime_prepare` | analyst / verifier | 是 | runtime.prepare | 为提供的 target/libc/loader 创建不可变执行副本并返回直接 argv。三份 ELF 架构须一致。不执行或修改原件；真实映射另行确认。 |
| `runtime_profile` | analyst / verifier | 否 | runtime.profile | 检查同 run target/libc/loader 的 SHA256、ELF Build ID 和架构；可选观察所属存活调试会话真实映射。文件候选不代表确认运行时。 |
| `seccomp_analyze` | analyst / verifier | 否 | seccomp.analyze | 解析同 run 原始 classic-BPF seccomp 产物的指令、分支和动作；可具体探测架构/syscall/参数/IP。产物不证明已安装策略，探测不等于完整 allowlist。 |
| `send_message_to_agent` | 全部 | 是 | controller | 入队 information、question、reply、result 或 blocked 通知。information 不打断等待，只有 follow_up_agent 重开已完成任务；queued/included 不证明已处理。 |
| `stage_target` | analyst / verifier | 是 | file.upload.begin + file.upload.write + file.upload.commit | 将 SHA256 已核对的本 run 原始目标复制到 worker 不可变文件区。 |
| `stop_agent` | 全部 | 是 | controller | 停止自己或后代并记录原因。 |
| `think` | 全部 | 是 | controller | 记录本 Agent 的简短可审计草稿。原文留在工具历史并可能由子任务继承；不是私密通道或节省上下文的机制。共享结论使用 note_save。 |
| `threat_model_amend` | 全部 | 是 | controller | 向共享威胁模型追加持久修订。 |
| `threat_model_get` | 全部 | 否 | controller | 读取共享威胁模型文档。 |
| `threat_model_save` | 全部 | 是 | controller | 创建稳定文档 ID 的共享威胁模型。 |
| `todo_save` | 全部 | 是 | controller | 创建共享待办，默认 pending。 |
| `todo_update` | 全部 | 是 | controller | 更新共享待办正文或状态。 |
| `todos_list` | 全部 | 否 | controller | 列出共享待办。 |
| `verification_record` | verifier | 是 | job.status + job.artifacts | 仅 verifier：导入完成的 poc_verify job、原始证据和真实尝试计数。崩溃只验证崩溃行为。 |
| `view_agent_graph` | 全部 | 否 | controller | 查询紧凑分页的代理状态及完整 ID（默认每页 8 条，使用 next_offset 翻页）。指定 agent_id 获取单个代理完整任务、契约和结果。同一 run 内接受至少 8 字符的唯一前缀；后续协调使用返回的完整 ID。 |
| `wait_for_agents` | 全部 | 是 | controller | 按 all_terminal（默认）、all_success 或 any_success 等待选定 Agent，返回 satisfied/blocked 或超时；空列表表示子任务。 |
| `xrefs_query` | analyst / verifier | 否 | controller Ghidra or ghidra_query | Ghidra：查询绑定目标的 module_relative AddressRef 或兼容字节偏移的引用关系。复制 binary_inspect 地址；链接时 VA 减去 image_base，不能将文件偏移或页号解释为模块偏移。 |
