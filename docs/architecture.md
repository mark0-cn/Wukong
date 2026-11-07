# Rust / Rig 二进制逆向 Agent 实现架构

本项目按 Strix 的模块职责与运行语义重建：根 agent 调度、动态子 agent、消息、独立会话、沙箱工具、skills、证据与独立验证。`strix-rig-binary-agent-plan.md` 是需求基线，`implementation-contract.md` 记录验收不变量。控制端使用 Rust，模型循环固定使用 Rig 0.46.0。

## 模块与职责

| 模块 | 职责 |
|---|---|
| `domain` | run/agent/message/event/tool/evidence/finding/verification/coverage 的持久化记录 |
| `store` | SQLite 事务、事件与邮箱、按内容哈希保存的原始产物 |
| `coordinator` | 父子树、动态任务去重、状态转移、消息与等待唤醒、预算和完成门槛 |
| `agents`、`skills/` | 根 agent 与领域角色提示词、继承上下文、角色能力范围 |
| `llm` | provider 配置、Rig AgentRun、准备模型请求、工具结果回放、压缩与版本检查 |
| `runner` | 调度可运行 agent，驱动模型效果与工具效果，保存恢复点 |
| `tools` | 类型化参数、权限/路径检查、工具调用记录、输出截断与证据引用 |
| `binary` | ELF/PE/Mach-O 画像、SHA256、字符串、Ghidra headless 后端 |
| `runtime`、`workers/runtime` | 隔离运行、持久化 worker、GDB/MI 会话、AFL++ 任务、崩溃重放与最小化 |
| `mcp` | 第三方 MCP 工具扩展 |
| `report`、`interface` | 报告导出、CLI、TUI、人工查询与干预 |

## 一次任务如何运行

1. 导入目标并记录完整 SHA256、路径和配置，创建根 run 与根 agent。
2. 根 agent 调用 `create_agent`，事务写入子任务后立即返回。子任务按角色、scope 去重，并受数量、并发、深度、时间和 token 预算约束。
3. 每个 agent 有独立的 Rig `AgentRun` 检查点。模型请求与工具执行由控制端驱动，执行边界保存到 SQLite。
4. 子 agent 通过持久化邮箱协作；等待时释放执行机会。子任务完成消息有去重键，父任务从消息或等待事件继续。
5. 工具原始结果写入证据目录，模型只获取有界摘要和证据 ID。通用分析信息写入共享笔记、待办与 coverage；threat model 仅在安全边界分析中按需建立。压缩会话时保留这些共享资料及原始检查点的证据引用，协调消息进入可压缩的会话历史。
6. 已确认影响必须关联独立 verifier 的验证记录。根任务完成前，活动后代和明确需要跟进的覆盖项必须得到处理。

## 状态与恢复

agent 状态包括 queued、running、waiting、budget_paused、completed、stopped、crashed、failed。完成/停止事件通知父任务。预算暂停保留检查点，恢复后继续调度。

活动任务内，通过 `follow_up_agent`（CLI `message --kind follow_up`）向 completed 子 agent 追加任务会保留其历史和累计用量，并开始新的 generation；工具调用与完成通知按 generation 区分。父任务已完成时一并唤醒，已停止或失败的分支需新建任务。显式前驱依赖与父子完成依赖合并检查，拒绝祖先依赖及间接循环。

子 agent 完成前须关闭自己持有的调试和交互会话，根任务完成前须处理全部会话。停止或失败的 owner 遗留会话可以由同任务的其他子 agent 关闭；该权限不允许发送调试命令或交互输入。

Rig 检查点序列化模型对话和 provider 原生状态，不包含模型客户端或凭据。检查点带 Rig 版本；跨版本恢复必须显式迁移。密钥由配置指定的环境变量提供。上下文压缩记录原始用量和压缩元数据，证据文件独立保留。

`load_skill` 将本地 skill 的正文、SHA256 与原始证据引用保存为 agent 的活动指令快照，在下一次新模型请求的系统提示词中生效。已冻结的请求保持原样；源文件后续变化不会悄悄改写已加载的快照。动态指令不经过工具结果摘要，压缩和恢复保留其内容。单个 skill 上限为 64 KiB；root 编排规则只允许 root 角色加载。

外部进程与模型状态分别恢复。worker 的 debugger/fuzz/job handle 需要重新核对存活状态；保存了 pid 或 SQLite 状态不等于进程仍在运行。已完成的有副作用工具调用复用记录，无法确定结果的调用不能盲目重放。GDB 同一会话只允许 owner 顺序修改；并行实验创建独立进程。

## 地址与静态分析

地址引用明确携带样本 SHA256，以及 file_offset、module_relative 或 runtime 地址空间。runtime 引用还需记录运行实例与 load_base。module_relative 从画像报告的 image_base 起算；文件偏移不能直接作为调试器虚拟地址。

Rust `object` 负责真实解析 ELF、PE 与 Mach-O 的架构、位宽、字节序、节、符号、依赖和导入。Mach-O `LC_MAIN` 的文件偏移先映射到模块地址，再输出 module-relative。输入、列表和字符串有上限；超限/解析错误明确报告。

保护机制检查给出 enabled、disabled、partial、unknown 和原始依据：ELF 使用 program headers/dynamic 标记；PE 使用 DLL characteristics；Mach-O 使用 header 标记。发现 canary 函数只证明对应符号存在，不证明所有函数都有保护；静态 ASLR 标记不证明运行时 OS 策略开启。无法确定时保持 unknown。

Ghidra 使用真实 headless 导入、自动分析与 Java bridge。functions 是函数管理器查询；decompile 使用 `DecompInterface`；xrefs 使用 reference manager。自动分析超时、无函数、反编译失败和哈希不符返回错误。完整反编译保存为独立 `.c` 产物。

项目缓存绑定目标 SHA256、Ghidra 版本/配置、launcher 与 bridge 内容哈希。首次导入完成后持久化项目，后续不同查询使用 `-process -noanalysis`。文件锁保护项目的单写者约束；每次请求保留参数、响应、stdout 与 stderr。外部/寄存器/栈地址保留 Ghidra 符号空间，不能伪装成 module-relative。

## 动态验证与评测边界

静态假设、崩溃复现、根因确认、利用原语验证和最终影响分别记录。crash replay 使用目标哈希、运行参数、输入和环境；最小化重新运行谓词，不能用输入长度或某个字符串替代实际复现。

`fixtures/` 包含符号/stripped 的本地漏洞、修复与正常样本。构建脚本生成 ELF，行为 harness 实际重放并保存哈希与退出结果；检测准确率由独立报告和显式复核标签计算，不从 crash 推断 agent 检测成功。详见 `fixtures/README.md`。

首版动态工具集中在 Linux x86_64 ELF。控制端可以运行于 Windows；PE/Mach-O 静态画像可用，但本仓库未实现 Windows DbgEng 或 macOS LLDB 动态后端。外部工具与模型 provider 的环境依赖需要实际配置，缺少依赖会返回明确错误。

## Typed coordination messages

Messages carry kind, correlation/reply IDs, recipient generation, sender generation,
current task identity and evidence references. `send_message_to_agent` defaults to
information: it neither reopens completed work nor interrupts a wait. Questions,
replies, results and blocked notices can interrupt a waiter without erasing its
wait condition. Only explicit `follow_up_agent` reopens completed work.

`queued` means durably enqueued; `included` means installed in a checkpointed
provider request, not that the model has acted. `replied` records an explicit
correlated reply (not an assessment of correctness). Old-generation replies are
retained as `stale` and never injected as current work. `message_status` exposes
these states. Frozen model requests keep their original mailbox snapshot.
This increment uses a fresh schema 2 store; no old-database migration is provided.
Informational mail left on completed work becomes
stale when a new generation starts. CLI messages default to information; TUI
operator input is a question and does not implicitly reopen completed work.

## Task contracts and results

`create_agent.contract` optionally declares an objective, input evidence IDs,
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
