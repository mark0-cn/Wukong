<a id="rust--rig-binary-analysis-agent-architecture"></a>

# Rust / Rig 二进制逆向 Agent 实现架构

[English](architecture.md) · 简体中文

本项目按 Strix 的模块职责与运行语义重建：根 agent 编排、动态子 agent、消息、独立会话、沙箱工具、skills、证据与独立验证。[原始方案](../history/strix-rig-binary-agent-plan.zh-CN.md)是历史需求基线，[实现契约](implementation-contract.zh-CN.md)记录验收不变量。控制端使用 Rust，生产执行循环固定使用 Rig 0.46.0 的 ECS 内核及原生 SubagentsPlugin。

<a id="modules-and-responsibilities"></a>

## 模块与职责

| 模块 | 职责 |
|---|---|
| `domain` | run/agent/message/event/tool/evidence/finding/verification/coverage 的持久化记录 |
| `store` | SQLite 事务、事件与邮箱、按内容哈希保存的原始产物 |
| `coordinator` | 共享事务入口；`runs`、`tasks`、`messages`、`waits`、`completion` 分别管理运行、子任务、消息、依赖等待与完成 |
| `agents`、`prompts/` | 角色与能力提示词装配；`src/prompts.rs` 统一维护内置目录 |
| `llm` | provider 配置与原生连接、结构化工具摘要及摘要用量 hook |
| `runner`、`bootstrap` | 运行生命周期与具体依赖装配 |
| `kernel_host`、`model_catalog`、`usage` | 内核可调用的业务接口、角色模型选择、用量及预算记账 |
| `kernel` | Rig 执行与日志、子任务及消息协议的业务接入，模型与工具准入、原生会话投影 |
| `tools` | 单一 `catalog` 注册；`args/`、`handlers/` 按职责分组，服务负责权限、资源和证据；`verification` 统一独立验证准入与认证 |
| `binary` | 地址身份及 SHA256；`profile` 解析 ELF/PE/Mach-O，`strings` 提取有界字符串，`ghidra` 管理 headless 后端及缓存/进程所有权 |
| `runtime`、`workers/runtime` | 隔离运行、持久化 worker、GDB/MI 会话、AFL++ 任务、崩溃重放与最小化 |
| `mcp` | rmcp 协议与 Rig 工具定义、命名空间、超时、环境变量及消息大小限制 |
| `report`、`interface` | 报告导出、CLI、人工查询与干预 |
| `desktop`、`ui/src` | Tauri 命令与工作区服务、展示数据适配、React 桌面界面及生命周期 |

<a id="a-runs-lifecycle"></a>

## 一次任务如何运行

1. 导入目标并记录完整 SHA256、路径和配置，创建根 run 与根 agent。
2. 根 agent 调用原生 `task`，使用 description、prompt、role、scope 及可选 contract 等字段。业务层事务写入子任务后，Rig 异步创建子 agent 并返回。按角色、scope 去重；前驱未完成时只阻止子任务的模型请求，不阻塞创建者。
3. 每个 agent 有独立的原生 Conversation；AgentPlugin 驱动模型与工具效果，SessionLogPlugin 保存可恢复日志，EffectLogPlugin 保存效果记录。SQLite 保留业务账本与供 CLI/GUI 查询的会话投影。并发额度按在途模型请求计算，等待中的父任务不占模型名额。
4. SubagentsPlugin 负责创建、请求、报告与取消。`wait` 等待一个可达 agent，`wait_for_agents` 提供业务成功条件。完成通知按 generation 去重，并绑定一次原生 Report。普通信息不释放等待；已完成任务的新工作必须使用 `follow_up_agent`。
5. 工具 schema 和校验复用 Rig。原始结果写入证据目录，模型只获取有界摘要和证据 ID；摘要采用 AgentRunner 的类型化输出和 AgentHook 计量，身份及状态字段由宿主保留。通用分析信息写入共享笔记、待办与 coverage；threat model 仅在安全边界分析中按需建立。
6. 会话保留、工具调用配对及摘要替换复用 rig-memory，保留共享资料和原始会话的证据引用。继承上下文只复制已闭合的父对话，不为尚未返回的工具调用制造结果。重试采用 Rig 的错误分类与 Retry-After；用量计价采用 Rig Pricing，业务层继续执行预算暂停。
7. 已确认影响必须关联独立 verifier 的验证记录。`agent_finish` 与 `finish_scan` 先通过业务验收，包括契约、证据和会话清理，再由原生协议报告完成；普通文本答案不能绕过这些门槛。

<a id="state-and-recovery"></a>

## 状态与恢复

`TaskGraph`、`DependencyRules`、`CompletionRules` 和 `MessageRules` 是不依赖存储及执行框架的纯领域规则。协调应用服务在同一事务中加载事实、调用规则、提交任务和邮箱变化；数据库查询与 generation 归档留在事务会话实现中。

agent 状态包括 queued、running、waiting、budget_paused、completed、stopped、crashed、failed。完成/停止事件通知父任务。预算暂停保留检查点，恢复后继续调度。

活动任务内，通过 `follow_up_agent`（CLI `message --kind follow_up`）向 completed 子 agent 追加任务会保留其历史和累计用量，并开始新的 generation；工具调用与完成通知按 generation 区分。父任务已完成时一并唤醒，已停止或失败的分支需新建任务。显式前驱依赖与父子完成依赖合并检查，拒绝祖先依赖及间接循环。

子 agent 完成前须关闭自己持有的调试和交互会话，根任务完成前须处理全部会话。停止或失败的 owner 遗留会话可以由同任务的其他子 agent 关闭；该权限不允许发送调试命令或交互输入。

Rig 原生日志位于 `state_dir/rig/RUN_ID`，保存模型对话、效果及 provider 原生元数据，不包含模型客户端或凭据。首次模型请求前固定解析后的 provider/endpoint 配置，恢复使用保存值。角色输出预算逐请求设置，不覆盖模型自身的容量信息。密钥由配置指定的环境变量提供。本次实现面向新任务，使用 schema 4 状态目录，不提供旧数据库或旧版自定义 AgentRun 检查点迁移。

`DurableEffects` 统一工具调用准入、稳定标识、事务回执、worker 成功回执和恢复决策。业务调用标识由 agent、generation、已计量模型轮次与 provider tool-call ID 构成，不依赖恢复后会重新分配的 ECS 实体或效果 ID。业务事务与回执一起提交；worker 回复在控制器后处理前持久化，若回复丢失则用同一请求标识查询 worker 的持久化操作账本。未决 MCP 写操作保留给操作员对账，不自动重放。操作员对账也使用带 epoch 的租约隔离。原生效果及数据库变化唤醒执行循环。

`KernelHost` 以具体业务操作连接 Rig 适配器，内核不访问 Store、Coordinator、ToolService 或完整配置。`ModelCatalog` 管理已解析的角色模型，`UsageService` 原子更新用量及预算状态，`bootstrap` 统一创建具体客户端和应用服务。

`RunSupervisor` 独立管理控制端租约、时间预算和后台 worker 观察，Rig 帧不等待 worker RPC。租约带持久化递增版本；运行中的 runner、coordinator 和工具共享带该版本的存储句柄，每次业务写事务在提交修改前验证 owner、版本和有效期。租约失效后旧控制端停止执行，其迟到结果不能覆盖接管者。监督任务随运行结束或取消清理，释放租约时也核对版本。

CLI 和 GUI 通过 `AssessmentService` 执行启动、恢复、暂停、停止及操作员消息。`PreparedAssessment` 保留已解析的运行配置与事件订阅，在执行完成或错误后关闭控制端工具连接；前端不自行编排租约或构造恢复配置。恢复准备不修改任务，预算修改在监督器取得正式租约后提交。

`FilesService`、`JobService`、`SessionsService` 和 `AnalysisService` 各自持有局部配置与所需依赖，worker 传输和稳定请求回执集中在 `WorkerGateway`。控制端使用 `JobRecord`、`SessionRecord`、`WorkerFileRecord` 和 `RunScoped` 检查归属，原始 worker 扩展载荷保留为 JSON。

`JobService` 统一自动监控和手动状态查询：检查任务归属、解析类型化 `JobStatus`、导入终止状态原始证据及产物后，发布可供模型读取的通知。同一任务的观察串行化，更新时合并最新账本，避免覆盖已导入产物或通知状态。`lost` 是终止状态，退出码和验证标志来自 `result`。任务通知与完成标识在同一事务写入持久化邮箱，沿用会话检查点与消息确认的原子提交；重启后未确认消息继续投递，旧 generation 的迟到任务结果标记为 stale。worker 错误通过 RPC `error.data.kind` 携带类别，业务会话和恢复代码按错误类别对账。

运行时角色只有 `root`、`analyst`、`verifier`。root 负责拆解、派单、证据协调和最终交付；analyst 负责目标分析；verifier 独立检查其他实例的结论。子任务的 `role` 只接受 analyst 或 verifier。模型、预算和工具权限由角色决定，`skills` 不授予新权限。

`recon`、`reverse`、`hunter`、`fuzz`、`crash`、`exploit`、`reporter` 是能力，`formats`、`types`、`algorithms`、`state_machine`、`binary_diff` 等提供具体方法。同一 analyst 可以在自己的问题范围内连续使用不同方法；不会仅因切换能力就创建后继任务。同范围的活跃 analyst 共用去重入口，独立 verifier 可以另建同范围的复核任务。hunter 可以从限定的研究范围开始发现并检验假设，无需预先给定漏洞假设。普通报告由 root 汇总，需要大型报告产物时才委派带 reporter 的 analyst。

角色指令由控制器自动安装，不能作为附加 skill 加载。`load_skill` 将本地能力的正文、SHA256 与原始证据引用保存为 agent 的活动指令快照，在下一次新模型请求的系统提示词中生效。已冻结的请求保持原样；源文件后续变化不会悄悄改写已加载的快照。同名活动快照替代初始 skill 的正文，避免两份版本并存。动态指令不经过工具结果摘要，压缩和恢复保留其内容。单个 skill 上限为 64 KiB；初始和动态能力合计最多 5 个不同名称。能力只补充方法、证据和产物，任务结束由主角色统一管理；恢复成功后继续原任务。

提示词集中在 `prompts/`：`base.md` 保存通用规则，`roles/` 保存身份与生命周期，`capabilities/` 保存专项能力，`methods/` 保存具体技术，`support/` 保存工具与恢复指导，`examples/` 保存有完整示范价值的案例。`src/prompts.rs` 注册名称、类别、用途、内置正文与可选示例引用；提示词中的能力发现目录、内置加载、错误提示和角色 skill 限制均从这份注册表派生。发现目录只包含名称和用途，正文及示例仍按角色和选中的 skill 装配。仅重复正文的 fuzz/crash 短示例已移除。

调用名称保持 `reverse`、`hunter` 等，不包含目录。`skills_dir` 继续通过平铺的 `<name>.md` 覆盖内置正文或提供自定义方法；`demos_dir` 默认指向 `prompts/examples`，也可指定平铺的自定义示例目录。目录缺失时跳过示例，内置正文已嵌入程序，不依赖运行时读取仓库文件。

共享资料按需装配：每次新模型请求按作者、父任务、前驱、scope 和任务关键词选择最多 6 条笔记、待办或可选 threat model 的有界预览。选择属于启发式提示，不保证覆盖全部相关资料；提示词明确提供遗漏数量和检索入口。`context_search` 支持文本、kind、作者 scope、agent_id 和分页，`context_read` 按 Unicode 字符位置读取原始文档 JSON（含 amendments）。两者按工具输出字节限额自动缩小页面，并保留下一页位置，不经过模型摘要。所有读取限于当前 run；原始证据继续通过 `evidence_read` 读取。会话压缩只保存同样的有界预览、账本计数和完整会话证据引用，不重新注入全部文档或任务树。计划和约束仍来自任务契约，筛选不会替代验收。

内置工具由 `src/tools/catalog.rs` 统一注册，模型从第一轮获得角色可见的完整目录；原生协调工具为 `task`、`message`、`wait`。root 使用 `finish_scan`，子 agent 使用 `agent_finish`；目标身份、证据、会话 owner 和独立复核仍由实际操作校验。工具定义直接用于执行、参数校验和[生成的工具目录](../reference/tool-catalog.zh-CN.md)，不保存工具发现或加载选择。MCP 定义在连接后合并，实际能力仍需探测。运行 `wukong tool-catalog` 重新生成 Markdown，`wukong --json tool-catalog` 导出机器可读 schema。

内置方法的来源、相关工具和机制夹具由 `prompts/coverage.json` 关联到现有 `SkillManifest`，通过 `wukong skill-catalog` 生成[覆盖清单](../reference/skill-coverage.zh-CN.md)。这些维护信息不改变角色权限、工具可用性或完成门槛。

Python worker 的入口、传输、操作账本、任务管理、独立监督、调试、交互、文件和命令执行由独立组件负责。`Runtime` 仅装配和路由，操作账本通过回调执行及核对会话存活，不依赖具体运行时。

具名[分析环境](cross-platform-environments.zh-CN.md)按需连接并与子任务不可变绑定。资源身份包含 worker；共享制品经过控制器哈希校验存储，以有界、可恢复的方式传输。

外部进程与模型状态分别恢复。worker 的 debugger/fuzz/job handle 需要重新核对存活状态；保存了 pid 或 SQLite 状态不等于进程仍在运行。已完成的有副作用工具调用复用记录，无法确定结果的调用不能盲目重放。MCP 的 readOnlyHint 通过 Rig 的提示语义处理，不足以授权重放结果未知的远端调用。GDB 同一会话只允许 owner 顺序修改；并行实验创建独立进程。原生内核只注册配置允许的业务工具，不加载宿主 shell 或通用编码工具。

<a id="desktop-reads-and-persistence-boundaries"></a>

## 桌面读取与持久化边界

`queries` 提供 `WorkspaceQueries`、`AssessmentQueries` 和 `ReportQueries`，返回不依赖 GUI 的摘要、增量游标、会话快照和报告快照。SQL 实现位于 `store/workspace` 与 `store/report_queries`；CLI 与 GUI 共用查询模型，聊天解析和布局缓存只保留在 GUI 适配层。

`desktop/workspace` 通过 `store/desktop_queries` 在同一快照读取轻量任务/Agent 摘要。响应修订签名结合 epoch、`entity_revisions`、事件序号、查询范围和操作员消息；未变化时不重复发送工作区数据，也能观察独立 CLI 进程的修改。活动历史默认保留最新 500 条，可按需扩展到 10000 条，工具调用仅加载活动窗口引用的记录。前端 `useWorkspace` 丢弃旧请求结果，切换任务或存储目录后重新读取。

业务写入通过 `StateSession` 事务会话提交，业务层不接收 SQLite 连接。`StoredRecord` 在存储实现中绑定实体类型、存储位置、身份和会话读取策略；业务使用 `load`、`records`、`save_record`，文档及会话使用有类型的类别接口，任务结果归档也通过具体业务方法完成。消息替换、旧 generation 消息归档和未读计数封装在存储实现内；所有回执与业务更新共享同一次租约校验和 IMMEDIATE 事务。

SQLite 使用 WAL、独立只读连接和单写入连接。普通查询以读事务观察一致快照，业务状态更新继续使用 IMMEDIATE 事务；索引覆盖按任务、Agent、事件种类和序号读取事件的路径。`entities` 中的 Agent JSON 只保存元数据，`agent_conversations` 独立保存 checkpoint、继承历史和会话修订号。预算与状态更新不读取或改写会话；完整读取接口在继承、follow-up、报告和恢复检查时组合两者，Rig 原生日志继续承担执行恢复。

类型边界与此对应：`AgentMetadata` 不含 checkpoint 或继承历史，元数据读取和写入接口使用该具体类型；`AgentConversation` 表示可选会话载荷，`AgentSnapshot` 在继承、恢复、报告和聊天展示时显式组合两者。完整快照使用平铺 JSON 结构，业务热路径使用元数据类型。

`desktop/presentation` 把公开进度、操作员消息和工具结果组合为展示 DTO，前端 `Conversation` 使用 assistant-ui 和虚拟列表呈现聊天。命令名、参数与返回值由 `ui/src/commands.ts` 的映射约束，`bridge` 统一调用；该类型检查不代替 Rust 权限和 epoch 校验。`useDesktopLifecycle` 管理关闭确认、原生文件拖入和键盘监听，`useStartupUpdates` 管理启动更新检查。样式入口保留固定导入顺序，颜色、分栏和组件样式分别维护。性能回归和模块边界见[维护约定](../development/maintenance.zh-CN.md)。

<a id="addresses-and-static-analysis"></a>

## 地址与静态分析

地址引用明确携带样本 SHA256，以及 file_offset、module_relative 或 runtime 地址空间。runtime 引用还需记录运行实例与 load_base。module_relative 从画像报告的 image_base 起算；文件偏移不能直接作为调试器虚拟地址。

Rust `object` 负责真实解析 ELF、PE 与 Mach-O 的架构、位宽、字节序、节、符号、依赖和导入。Mach-O `LC_MAIN` 的文件偏移先映射到模块地址，再输出 module-relative。输入、列表和字符串有上限；超限/解析错误明确报告。

保护机制检查给出 enabled、disabled、partial、unknown 和原始依据：ELF 使用 program headers/dynamic 标记；PE 使用 DLL characteristics；Mach-O 使用 header 标记。发现 canary 函数只证明对应符号存在，不证明所有函数都有保护；静态 ASLR 标记不证明运行时 OS 策略开启。无法确定时保持 unknown。

Ghidra 使用真实 headless 导入、自动分析与 Java bridge。functions 是函数管理器查询；decompile 使用 `DecompInterface`；xrefs 使用 reference manager。自动分析超时、无函数、反编译失败和哈希不符返回错误。完整反编译保存为独立 `.c` 产物。

项目缓存绑定目标 SHA256、Ghidra 版本/配置、launcher 与 bridge 内容哈希。首次导入完成后持久化项目，后续不同查询使用 `-process -noanalysis`。文件锁保护项目的单写者约束；每次请求保留参数、响应、stdout 与 stderr。外部/寄存器/栈地址保留 Ghidra 符号空间，不能伪装成 module-relative。

<a id="dynamic-verification-and-evaluation-limits"></a>

## 动态验证与评测边界

`tools/verification` 集中处理验证准入和证据认证，`jobs` 负责通用任务查询与产物导入。`verification_protocol` 对 worker 身份、计数、网络绑定和逐次观测建立类型化视图；原始 JSON 继续完整保存为证据，类型合格不等于影响成立。网络绑定的扩展字段参与身份比较，不丢弃未解析的身份限制。

静态假设、崩溃复现、根因确认、利用原语验证和最终影响分别记录。crash replay 使用目标哈希、运行参数、输入和环境；最小化重新运行谓词，不能用输入长度或某个字符串替代实际复现。

`fixtures/` 包含符号/stripped 的本地漏洞、修复与正常样本。构建脚本生成 ELF，行为 harness 实际重放并保存哈希与退出结果；检测准确率由独立报告和显式复核标签计算，不从 crash 推断 agent 检测成功。详见 `fixtures/README.md`。

首版动态工具集中在 Linux x86_64 ELF。PE/Mach-O 静态画像可用，但本仓库未实现 Windows DbgEng 或 macOS LLDB 动态后端。当前平台验收以 CI 和[桌面更新验证](../development/desktop-updates.zh-CN.md)为准；早期原生内核检查不证明当前 Windows Docker Desktop 覆盖。外部工具与模型 provider 的环境依赖需要实际配置，缺少依赖会返回明确错误；本地协议测试不等同于线上 provider 或外部引擎验收。

<a id="typed-coordination-messages"></a>

## 类型化协调消息

消息携带 kind、correlation/reply ID、收发 generation、当前任务身份和证据引用。send_message_to_agent 默认 information：不重开任务、不打断等待。question、reply、result、blocked 可唤醒等待者但不清除等待条件。只有显式 follow_up_agent 重开已完成任务。

queued 表示持久入队；included 表示进入原生会话/上下文及日志，不表示模型已执行或处理。replied 表示显式关联回复，不判断正确性。旧代次回复保留为 stale，不注入当前工作。message_status 展示状态；在途请求保留原邮箱快照。此增量使用新 schema 4，不迁移旧数据库。已完成任务遗留 information 在新代次开始时变 stale。CLI/GUI 默认 information，显式 follow-up 才重开。

原生 message 向可达活动 Agent 发请求或回复。信息/关联邮件使用类型化 send_message_to_agent，完成后的新工作用 follow_up_agent。原生等待投递与类型化回执一起投影；included 不证明模型已处理或回答。

<a id="task-contracts-and-results"></a>

## 任务契约和结果

task.contract 可声明 objective、输入 evidence ID、约束、命名产物及完成条件标签；无额外要求时任务文本作为目标。契约进入专员提示并跨检查点保留。活动 role/scope 不能悄悄复用于冲突契约。

agent_finish/finish_scan 接受 result：outcome（success/inconclusive/blocked/cancelled）、answer、evidence ID、命名证据产物、完成标签、unknowns、next actions。无必需产物/条件时可用 summary 简写。success 检查产物名、原始证据完整性和声明条件覆盖；这是完整性/来源检查，不证明模型结论正确。非成功需说明未知或下一步，可缺失输出。cancelled 转 stopped；blocked/inconclusive 保留独立 outcome 供依赖判断。

结果附在终态通知/Agent 图，按 task/agent ID 和 generation 归档到导出 snapshot.task_results。follow-up 清当前结果但不删上一代归档。当前 Agent ID 就是任务身份；独立任务分派/worker attempts 是后续扩展。

<a id="dependency-admission-and-waits"></a>

## 依赖准入与等待

新子任务默认 dependency_policy=all_success。all_terminal 在所有前提结束（含失败）后放行收集者；any_success 在一个成功后放行，仅当全部结束且无成功才阻止。成功要求 completed 加 success 结果，inconclusive/blocked 不算。缺失前提是错误，不视为成功。

模型调度前事务放行或记录 blocked、使未启动子任务失败、归档并通知父任务。放行请求收到前驱结果。失败无模型调用地传播。放行事件记录生产者代次/outcome；放行后前驱 follow-up 不撤销消费者，新依赖评估最新结果。这是准入顺序，不是自动实时数据流；所需证据仍须写契约。

wait_for_agents.policy 默认 all_terminal，也支持 all_success/any_success。返回/唤醒含 pending/satisfied/blocked 条件，与 timeout 分离。创建门槛和运行等待均加入父子完成图，事务拒绝循环。等待停驻与工具回执同时提交，丢响应后重放不能让同一轮再次停驻。

桌面对话由已持久化的调用、worker 后台任务和有界公开模型文本投影生成。`progress` 复用现有事务和版本通知保存公开快照；kernel 读取 Rig 的 `Partial.text`，经 `KernelHost` 委托持久化。流式执行与重试仍由 Rig 管理。消息身份由 Agent、代次和已计量轮次组成，不使用 ECS 实体 ID；旧代次、旧轮次不能覆盖新消息。开始与结束事件引用同一消息，中间快照覆盖该记录。租约过期参与桌面版本计算，并将未完成文本标记为中断。公开文本仅用于展示，不能解除完成或证据门槛。
