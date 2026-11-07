# Rust / Rig 二进制逆向 Agent 实现架构

本项目按 Strix 的模块职责与运行语义重建：根 agent 编排、动态子 agent、消息、独立会话、沙箱工具、skills、证据与独立验证。`strix-rig-binary-agent-plan.md` 是历史需求基线，`implementation-contract.md` 记录验收不变量。控制端使用 Rust，生产执行循环固定使用 Rig 0.46.0 的 ECS 内核及原生 SubagentsPlugin。

## 模块与职责

| 模块 | 职责 |
|---|---|
| `domain` | run/agent/message/event/tool/evidence/finding/verification/coverage 的持久化记录 |
| `store` | SQLite 事务、事件与邮箱、按内容哈希保存的原始产物 |
| `coordinator` | 业务父子树、任务去重、契约、状态转移、消息账本、依赖和完成门槛 |
| `agents`、`skills/` | 根 agent 与领域角色提示词、角色能力范围 |
| `llm` | provider 配置与原生连接、结构化工具摘要及摘要用量 hook |
| `runner` | 启动原生内核、租约、业务预算、事件与证据摘要 |
| `kernel` | Rig 执行与日志、子任务及消息协议的业务接入，模型与工具准入、原生会话投影 |
| `tools` | 类型化参数、权限/路径检查、工具调用记录、输出截断与证据引用 |
| `binary` | ELF/PE/Mach-O 画像、SHA256、字符串、Ghidra headless 后端 |
| `runtime`、`workers/runtime` | 隔离运行、持久化 worker、GDB/MI 会话、AFL++ 任务、崩溃重放与最小化 |
| `mcp` | rmcp 协议与 Rig 工具定义、命名空间、超时、环境变量及消息大小限制 |
| `report`、`interface` | 报告导出、CLI、TUI、人工查询与干预 |

## 一次任务如何运行

1. 导入目标并记录完整 SHA256、路径和配置，创建根 run 与根 agent。
2. 根 agent 调用原生 `task`，使用 description、prompt、role、scope 及可选 contract 等字段。业务层事务写入子任务后，Rig 异步创建子 agent 并返回。按角色、scope 去重；前驱未完成时只阻止子任务的模型请求，不阻塞创建者。
3. 每个 agent 有独立的原生 Conversation；AgentPlugin 驱动模型与工具效果，SessionLogPlugin 保存可恢复日志，EffectLogPlugin 保存效果记录。SQLite 保留业务账本与供 CLI/TUI 查询的会话投影。并发额度按在途模型请求计算，等待中的父任务不占模型名额。
4. SubagentsPlugin 负责创建、请求、报告与取消。`wait` 等待一个可达 agent，`wait_for_agents` 提供业务成功条件。完成通知按 generation 去重，并绑定一次原生 Report。普通信息不释放等待；已完成任务的新工作必须使用 `follow_up_agent`。
5. 工具 schema 和校验复用 Rig。原始结果写入证据目录，模型只获取有界摘要和证据 ID；摘要采用 AgentRunner 的类型化输出和 AgentHook 计量，身份及状态字段由宿主保留。通用分析信息写入共享笔记、待办与 coverage；threat model 仅在安全边界分析中按需建立。
6. 会话保留、工具调用配对及摘要替换复用 rig-memory，保留共享资料和原始会话的证据引用。继承上下文只复制已闭合的父对话，不为尚未返回的工具调用制造结果。重试采用 Rig 的错误分类与 Retry-After；用量计价采用 Rig Pricing，业务层继续执行预算暂停。
7. 已确认影响必须关联独立 verifier 的验证记录。`agent_finish` 与 `finish_scan` 先通过业务验收，包括契约、证据和会话清理，再由原生协议报告完成；普通文本答案不能绕过这些门槛。

## 状态与恢复

agent 状态包括 queued、running、waiting、budget_paused、completed、stopped、crashed、failed。完成/停止事件通知父任务。预算暂停保留检查点，恢复后继续调度。

活动任务内，通过 `follow_up_agent`（CLI `message --kind follow_up`）向 completed 子 agent 追加任务会保留其历史和累计用量，并开始新的 generation；工具调用与完成通知按 generation 区分。父任务已完成时一并唤醒，已停止或失败的分支需新建任务。显式前驱依赖与父子完成依赖合并检查，拒绝祖先依赖及间接循环。

子 agent 完成前须关闭自己持有的调试和交互会话，根任务完成前须处理全部会话。停止或失败的 owner 遗留会话可以由同任务的其他子 agent 关闭；该权限不允许发送调试命令或交互输入。

Rig 原生日志位于 `state_dir/rig/RUN_ID`，保存模型对话、效果及 provider 原生元数据，不包含模型客户端或凭据。首次模型请求前固定解析后的 provider/endpoint 配置，恢复使用保存值。角色输出预算逐请求设置，不覆盖模型自身的容量信息。密钥由配置指定的环境变量提供。本次实现面向新任务，不迁移旧版自定义 AgentRun 检查点。

业务工具调用标识由 agent、generation、已计量模型轮次与 provider tool-call ID 构成，不依赖恢复后会重新分配的 ECS 实体或效果 ID。中断后先重入业务调用账本，复用已完成结果或核对未决状态。原生效果及数据库变化唤醒执行循环；一秒心跳仅用于租约、时间预算及外部操作检查。

`load_skill` 将本地 skill 的正文、SHA256 与原始证据引用保存为 agent 的活动指令快照，在下一次新模型请求的系统提示词中生效。已冻结的请求保持原样；源文件后续变化不会悄悄改写已加载的快照。动态指令不经过工具结果摘要，压缩和恢复保留其内容。单个 skill 上限为 64 KiB；root 编排规则只允许 root 角色加载。

共享资料按需装配：每次新模型请求按作者、父任务、前驱、scope 和任务关键词选择最多 6 条笔记、待办或可选 threat model 的有界预览。选择属于启发式提示，不保证覆盖全部相关资料；提示词明确提供遗漏数量和检索入口。`context_search` 支持文本、kind、作者 scope、agent_id 和分页，`context_read` 按 Unicode 字符位置读取原始文档 JSON（含 amendments）。两者按工具输出字节限额自动缩小页面，并保留下一页位置，不经过模型摘要。所有读取限于当前 run；原始证据继续通过 `evidence_read` 读取。会话压缩只保存同样的有界预览、账本计数和完整会话证据引用，不重新注入全部文档或任务树。计划和约束仍来自任务契约，筛选不会替代验收。

外部进程与模型状态分别恢复。worker 的 debugger/fuzz/job handle 需要重新核对存活状态；保存了 pid 或 SQLite 状态不等于进程仍在运行。已完成的有副作用工具调用复用记录，无法确定结果的调用不能盲目重放。MCP 的 readOnlyHint 通过 Rig 的提示语义处理，不足以授权重放结果未知的远端调用。GDB 同一会话只允许 owner 顺序修改；并行实验创建独立进程。原生内核只注册配置允许的业务工具，不加载宿主 shell 或通用编码工具。

## 地址与静态分析

地址引用明确携带样本 SHA256，以及 file_offset、module_relative 或 runtime 地址空间。runtime 引用还需记录运行实例与 load_base。module_relative 从画像报告的 image_base 起算；文件偏移不能直接作为调试器虚拟地址。

Rust `object` 负责真实解析 ELF、PE 与 Mach-O 的架构、位宽、字节序、节、符号、依赖和导入。Mach-O `LC_MAIN` 的文件偏移先映射到模块地址，再输出 module-relative。输入、列表和字符串有上限；超限/解析错误明确报告。

保护机制检查给出 enabled、disabled、partial、unknown 和原始依据：ELF 使用 program headers/dynamic 标记；PE 使用 DLL characteristics；Mach-O 使用 header 标记。发现 canary 函数只证明对应符号存在，不证明所有函数都有保护；静态 ASLR 标记不证明运行时 OS 策略开启。无法确定时保持 unknown。

Ghidra 使用真实 headless 导入、自动分析与 Java bridge。functions 是函数管理器查询；decompile 使用 `DecompInterface`；xrefs 使用 reference manager。自动分析超时、无函数、反编译失败和哈希不符返回错误。完整反编译保存为独立 `.c` 产物。

项目缓存绑定目标 SHA256、Ghidra 版本/配置、launcher 与 bridge 内容哈希。首次导入完成后持久化项目，后续不同查询使用 `-process -noanalysis`。文件锁保护项目的单写者约束；每次请求保留参数、响应、stdout 与 stderr。外部/寄存器/栈地址保留 Ghidra 符号空间，不能伪装成 module-relative。

## 动态验证与评测边界

静态假设、崩溃复现、根因确认、利用原语验证和最终影响分别记录。crash replay 使用目标哈希、运行参数、输入和环境；最小化重新运行谓词，不能用输入长度或某个字符串替代实际复现。

`fixtures/` 包含符号/stripped 的本地漏洞、修复与正常样本。构建脚本生成 ELF，行为 harness 实际重放并保存哈希与退出结果；检测准确率由独立报告和显式复核标签计算，不从 crash 推断 agent 检测成功。详见 `fixtures/README.md`。

首版动态工具集中在 Linux x86_64 ELF。PE/Mach-O 静态画像可用，但本仓库未实现 Windows DbgEng 或 macOS LLDB 动态后端。本轮原生内核验证在 macOS 完成，Windows 控制端尚未验证。外部工具与模型 provider 的环境依赖需要实际配置，缺少依赖会返回明确错误；本地协议测试不等同于线上 provider 或外部引擎验收。

## Typed coordination messages

Messages carry kind, correlation/reply IDs, recipient generation, sender generation,
current task identity and evidence references. `send_message_to_agent` defaults to
information: it neither reopens completed work nor interrupts a wait. Questions,
replies, results and blocked notices can interrupt a waiter without erasing its
wait condition. Only explicit `follow_up_agent` reopens completed work.

`queued` means durably enqueued; `included` means accepted into the native
conversation/context and journal, not that a model request ran or acted on it.
`replied` records an explicit
correlated reply (not an assessment of correctness). Old-generation replies are
retained as `stale` and never injected as current work. `message_status` exposes
these states. In-flight model requests keep their original mailbox snapshot.
This increment uses a fresh schema 2 store; no old-database migration is provided.
Informational mail left on completed work becomes
stale when a new generation starts. CLI messages default to information; TUI
operator input is a question and does not implicitly reopen completed work.

The native `message` tool opens a request or sends a reply to a reachable active
agent. Use typed `send_message_to_agent` for informational updates and correlated
mail, and `follow_up_agent` for additional work after completion. Native wait
delivery and typed mailbox receipts are projected together; receipt inclusion
does not prove that the model processed or answered the message.

## Task contracts and results

`task.contract` optionally declares an objective, input evidence IDs,
constraints, required named deliverables and completion-condition labels. A task
without extra requirements uses its task text as the objective.
The contract is installed in the specialist prompt and survives checkpoints.
An active role/scope cannot silently be reused with a conflicting contract.

`agent_finish`/`finish_scan` accept `result`: outcome (success, inconclusive,
blocked, cancelled), answer, evidence IDs, named evidence artifacts, completed
condition labels, unknowns and next actions. Summary-only completion is a shorthand
for tasks without required artifacts or conditions. Success checks required artifact names, linked raw
evidence integrity and explicit coverage of declared conditions. These are
completeness/provenance checks, not proof that a model's conclusion is correct.
Non-success outcomes must identify unknowns or next actions and may omit missing
outputs. Cancelled work becomes stopped; blocked/inconclusive completion records
retain their distinct outcome for dependency evaluation.

Results are attached to the terminal notification and agent graph, and archived
per task/agent ID and generation in exported `snapshot.task_results`. A follow-up
clears the current result without deleting the archived previous generation.
The existing agent ID is the task identity in this increment; independent task
assignment/worker attempts remain a later extension.

## Dependency admission and waits

New children default to `dependency_policy=all_success`. `all_terminal` admits a
collector after every prerequisite ends, including failures. `any_success` admits
an alternative consumer after one succeeds and blocks only when all alternatives
end without success. A success requires completed execution plus a success task
result; inconclusive/blocked results do not qualify. Missing prerequisites are
errors, never assumed successes.

Before model scheduling, a transaction either releases the gate or records a
blocked result, fails the unstarted child, archives its result and notifies the
parent. Released models receive the predecessor results in their request context.
Failure propagates through dependent children without model calls. The
release event records producer generations/outcomes; once admitted, later producer
follow-ups do not revoke the consumer. New dependent work evaluates the latest
producer result. This is admission ordering, not automatic live dataflow: required
input evidence still belongs in the task contract.

`wait_for_agents.policy` defaults to all_terminal and also supports all_success
and any_success. Its return/wakeup includes a condition with state pending,
satisfied or blocked, separate from timeout. Both creation gates and runtime wait
edges join the parent-completion dependency graph; cyclic waits are rejected
transactionally. Wait parking and tool receipts commit together, so replay cannot
park the same turn a second time after a lost response.
