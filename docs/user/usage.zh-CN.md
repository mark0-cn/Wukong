<a id="user-guide"></a>

# 使用指南

[English](usage.md) · 简体中文

[返回项目首页](../../README.zh-CN.md) · [语言设置](languages.zh-CN.md)

本文补充桌面工作区、模型配置、任务恢复与协作约定。首次构建和初始化见 [快速开始](../../README.zh-CN.md#快速开始)。

<a id="contents"></a>

## 目录

- [桌面工作区](#桌面工作区)
- [配置与模型](#配置与模型)
- [任务管理与恢复](#任务管理与恢复)
- [预算与用量](#预算与用量)
- [Agent 角色与能力](#agent-角色与能力)
- [报告与复核](#报告与复核)

<a id="desktop-workspace"></a>

## 桌面工作区

桌面版基于 Tauri + React。直接双击 `wukong.exe`，或使用以下命令打开窗口，无需另开浏览器：

```sh
wukong
wukong gui
wukong gui RUN_ID
```

`gui RUN_ID` 直接选中已保存任务。未配置模型凭据时，仍可查看已有记录；开始分析需要可用的模型配置。

<a id="everyday-operation"></a>

### 日常操作

1. 点击“新建分析”，选择或拖入二进制，编辑分析任务后开始。
2. 从左侧任务列表搜索、切换任务；在会话中查看公开回复与工具操作，展开卡片查看原始参数和结果。
3. 打开“协作任务”面板选择 Agent，查看对应会话。任务概览提供任务总结，发现页可追溯证据。
4. 在输入框补充信息。给已完成的 Agent 安排新工作时，选择“追加任务”；暂停的分析需手动恢复。
5. 使用顶部操作暂停、恢复或导出报告，在协作面板停止指定分支。恢复对话框可调整累计预算上限。

任务预算限制累计 Token、费用、时长、Agent 数量和协作深度。

工作区采用任务列表、分析内容、协作详情三栏布局。拖动分栏边界可以调整宽度；键盘聚焦边界后也可用方向键调整，宽度保存在当前客户端。
较窄窗口默认收起协作详情，留出正文空间，仍可手动展开；手动选择的面板显示状态优先。
顶部“切换任务列表”和“协作任务”可分别收起两侧面板，“专注模式”同时收起两侧，再次点击恢复原有显示状态，未发送的消息草稿保留。
“更新”“工具环境”“题目环境”“工作区设置”集中在左侧底部；专注模式下先展开任务列表即可使用。
默认使用深灰与金色主题，蓝色表示执行中，绿色表示成功，黄色表示等待/警告，红色表示失败；状态同时显示图标和文字。

会话默认跟随最新进展；向上滚动查看历史会暂停跟随，点击“最新进展”返回。新任务输出语言按语言设置和任务内容解析，已有记录保持原文。

每个窗口同时驱动一个任务，并可浏览其他任务。关闭窗口时会先等待本窗口的分析暂停保存，以及报告导出等文件操作完成；关闭确认也会指出未发送草稿。

<a id="reading-and-navigation"></a>

### 阅读与导航

- 标题旁的铅笔可设置任务名称，留空恢复文件名。名称保存在当前设备的 WebView 本地存储，按工作区路径与任务 ID 区分，不修改任务指令、数据库、报告或目标文件，也不随迁移包转移。清理 WebView 数据会清除名称和布局偏好。
- 任务列表支持名称、路径和任务内容搜索，按日期分组；可筛选进行中、历史与暂停任务。分栏宽度及面板开关在本机保留。
- 顶部状态行展示当前操作；底部“模型与用量”展开模型、Token、费用和 Agent 数量。连接异常会同步改变指示点和文字。
- 成功工具操作默认紧凑展示。较大的工具组可展开全部记录，失败和未决操作始终可见。参数与原始结果在展开后读取。
- 输入框随内容增高，也可手动展开。“补充信息”不会隐式重开已结束任务；“追加任务”保持明确的 follow-up 语义，暂停任务仍需手动恢复。发送目标和接收代次仍绑定到当前展示的 Agent。
- 向上阅读时不自动跳到底部；“最新进展”仅在离开底部时显示，并记录新到达的消息数量。
- 发现详情优先展示验证结果和证据缺口，单份证据自动预览；多份证据通过列表选择。来源、SHA-256 和证据 ID 可复制。窄窗口通过“返回发现列表”切换，不同时挤放列表与详情。

<a id="collaboration-task-tree"></a>

### 协作任务树

右侧面板按 Agent 的真实父级关系显示树线，角色标签区分总控、分析和验证；状态颜色仅表示执行状态。分析和验证角色不会因为职责不同被自动排成上下级或先后顺序。

- 点击节点查看对应会话及底部详情；点击箭头只展开或折叠，不切换会话。
- 普通节点使用两行显示名称、角色、状态与范围。错误信息额外显示一行，完整任务、错误和总结留在详情中。
- 折叠节点显示整个子树的数量、运行、等待/暂停、异常及完成统计，不包含父节点自身；停止的子任务计入总数。折叠不会停止工作，也不会隐藏子树异常提示。
- 选中节点显示父子路径；路径中的名称可以跳回上级任务。选中节点被折叠隐藏时，其祖先显示“包含当前选中的任务”。
- 树形折叠状态按本机工作区路径和任务 ID 保存，切换任务或重启界面后保留。四层之外的节点通过“聚焦分支”继续查看，再用“返回上级”或“完整任务树”返回，避免深层缩进挤压名称。
- 聚焦任务树后，上下键移动焦点，左右键展开/折叠或进入子级/返回父级，Home/End 跳到首尾；Enter 或空格才选择节点。键盘焦点与当前会话选择相互独立。
- 若父级记录缺失或形成环，界面单独展示并标记异常分支，不修改原始记录，也不据此推断任务依赖。

<a id="editing-settings"></a>

### 修改设置

左侧“工作区设置”提供模型、API 地址、凭据环境变量名、预算、并发、执行后端和存储路径等表单字段。设置分为“语言与模型”“任务预算”“存储位置”“高级配置”。高级配置中的“TOML 源码”用于编辑包括角色模型、价格和 MCP 在内的完整配置。

- 切换分类和关闭设置保留未保存的配置及目录草稿；存储目录变更执行期间不能关闭设置，配置草稿未保存时不能切换目录。
- 保存前校验配置，保留原文件注释及未修改字段。
- 文件已被外部修改时拒绝覆盖，需重新加载后编辑。
- 可打开另一份 TOML，保存并应用后供新任务使用。
- 运行中的分析需先暂停，并等待正在进行的操作结束，再应用配置。
- 恢复已有任务沿用该任务保存的配置。
- `--state` 的临时覆盖仍然优先，不写回配置文件。

API key 内容不写入 TOML；填写环境变量名，而非密钥值。Windows 下优先读取进程环境变量；缺失时读取用户、系统环境变量，因此桌面启动也可使用已保存的凭据。仅在 PowerShell 中设置的 `$env:...` 只对该进程及其子进程生效。

<a id="windows-deployment-and-launch"></a>

### Windows 发布与启动

普通安装使用正式 Release 的 `-setup.exe`，安装与更新说明见[在线安装](online-install.zh-CN.md)。以下内容仅针对从源码构建后手工部署桌面程序。

若构建目录带有 Low 完整性标签，EXE 会继承该标签，启动后可能无法访问 Docker Desktop 的命名管道。可使用 [发布脚本](../../scripts/dev/publish-gui.ps1) 将程序复制到工作区之外的普通应用目录，并生成指向所选配置文件的快捷方式。

在仓库根目录运行，以下示例使用快速开始生成的 release 程序与根目录配置：

```powershell
.\scripts\dev\publish-gui.ps1 -Destination D:\wukong-bin\desktop-gui -BinaryPath .\target\release\wukong.exe -ConfigPath .\wukong.toml
```

此后使用发布目录中的 `Wukong.lnk` 启动。配置仍保存到指定的原始文件；脚本不修改工作区或 Docker 权限。重新构建后，关闭旧窗口并再次运行脚本即可更新。

<a id="configuration-and-models"></a>

## 配置与模型

完整字段见 [wukong.example.toml](../../wukong.example.toml)。相对文件路径按配置文件所在目录解析。

<a id="configuration-discovery"></a>

### 配置查找顺序

| 入口 | 查找顺序 |
| --- | --- |
| 显式指定 `--config PATH` | 始终优先使用指定文件 |
| 桌面入口 | 可执行文件同目录的 `wukong.toml` → 当前目录 → 用户数据目录中的默认配置 |
| CLI 命令 | 当前目录的 `wukong.toml` → 可执行文件同目录 |

桌面默认配置保存在 Windows `%LOCALAPPDATA%/WukongData/wukong.toml`，Linux 为 `$XDG_DATA_HOME/Wukong/wukong.toml`（默认 `~/.local/share/Wukong/wukong.toml`）；首次启动时按需创建。
任务数据与更新缓存可在“工作区设置 → 存储位置”切换，详情见[在线安装](online-install.zh-CN.md#更新)。

```sh
wukong --config ./wukong.toml --state ./saved-state gui
wukong --config ./wukong.toml analyze ./sample.elf --task "还原记录格式并验证字段边界"
```

<a id="model-services"></a>

### 模型服务

| `model.provider` | 接口 | 默认凭据环境变量 |
| --- | --- | --- |
| `openai` | Chat Completions | `OPENAI_API_KEY` |
| `openai_responses` | Responses | `OPENAI_API_KEY` |
| `anthropic` | Anthropic | `ANTHROPIC_API_KEY` |
| `ollama` | Ollama 原生 `/api/chat` | `OLLAMA_API_KEY`（可选） |

自定义 endpoint 通过 `model.base_url` 指定；`model.api_key_env` 可更改凭据环境变量名。Ollama 默认连接 `http://localhost:11434`；使用无凭据的本地服务时，不设置 `api_key_env`。若从示例配置切换到 Ollama，应删除该字段。

首次模型请求前会保存解析后的 endpoint，包括环境变量提供的默认值。恢复时使用任务保存的模型与后端配置。

<a id="role-models"></a>

### 角色模型

根 Agent 始终使用 `model.model`。`model.role_model` 为 analyst 和 verifier 指定共享的子 Agent 模型；`model.role_models` 可分别覆盖各角色的模型与单轮输出预算。它们使用同一 provider、endpoint 和凭据。

```toml
[model]
provider = "openai_responses"
model = "your-root-model"
api_key_env = "OPENAI_API_KEY"
max_tokens = 8192
role_model = "your-role-model"
role_max_tokens = 16384

[model.role_models]
verifier = { model = "your-verifier-model", max_tokens = 16384 }
```

角色模型选择优先级为：角色专属配置 → `role_model` → 根模型。`role_max_tokens` 为显式配置了角色模型的子 Agent 设置默认单轮输出上限；角色专属 `max_tokens` 优先。输出因 `finish_reason=Length` 截断时，可在服务支持的范围内提高相应上限。

<a id="execution-backends"></a>

### 执行后端

执行工具默认关闭。Docker、SSH / VM 部署见 [执行 worker](../../workers/runtime/README.zh-CN.md)，本地 Ghidra 配置见 [Ghidra 后端](../../workers/ghidra/README.zh-CN.md)。`doctor` 会报告实际可用能力；模型服务与外部引擎是否可用取决于实际环境。

`trusted_local` 直接在所在机器运行进程，仅适用于明确选择的可信测试样本。日常动态分析使用隔离 worker。

<a id="task-management-and-recovery"></a>

## 任务管理与恢复

以下示例中的 `RUN_ID`、`AGENT_ID`、`CALL_ID` 和 `EVIDENCE_ID` 均替换为实际 ID。

<a id="inspect-and-intervene"></a>

### 查看与干预

```sh
wukong runs
wukong status RUN_ID --events 30
wukong gui RUN_ID
wukong message AGENT_ID "请记录这条新的分析约束"
wukong message AGENT_ID "请继续调查该函数的边界条件" --kind follow_up
wukong stop AGENT_ID --reason "停止该分支"
```

普通消息补充信息，不会重新开启已完成任务。`--kind follow_up` 用于向已完成的 Agent 追加工作，保留其历史和累计预算；暂停状态仍需显式恢复。停止分支也会停止其子任务，已记录证据保留。

<a id="pause-and-resume"></a>

### 暂停与恢复

```sh
wukong pause RUN_ID
wukong resume RUN_ID --max-tokens 1000000
```

CLI 的 Ctrl+C 请求在持久化边界停止调度，保留状态以便恢复。`resume` 使用保存的配置与检查点，并可通过 `--max-tokens`、`--max-usd` 调整累计预算上限。活动控制端通过租约互斥。

原生 Rig 日志位于状态目录的 `rig/RUN_ID`。当前状态目录使用 schema 3，不提供旧数据库迁移；旧目录会返回版本提示，新任务应选择新的状态目录。

外部 jobs 和调试器需分别核对。持久化检查点不能恢复已经消失的调试进程；容器或 VM 丢失后，也不能凭检查点还原存活会话。

<a id="unresolved-external-calls"></a>

### 未决外部调用

有副作用且结果未知的第三方 MCP 调用保留未决状态，不会盲目重放。操作员复核实际结果并关联证据后，可执行对账：

```sh
wukong reconcile CALL_ID --result-file ./reviewed-result.json --evidence-id EVIDENCE_ID
```

<a id="budgets-and-usage"></a>

## 预算与用量

Token 预算按 provider 返回的用量累计。缺少必要用量时，系统保存响应并暂停，不会将未知消耗计成零。

已经发出的并行请求仍可能使观测用量超过阈值；取消或重发的请求也可能产生 provider 未回报的费用。配置的 USD 单价用于估算，并非账单保证。

使用费用预算前，在 `[pricing]` 中配置实际的每百万 Token 输入与输出价格。默认零价格关闭 USD 计价；`max_usd` 需要相应价格配置。完整预算字段见 [配置示例](../../wukong.example.toml)。

<a id="agent-roles-and-capabilities"></a>

## Agent 角色与能力

<a id="roles"></a>

### 角色分工

| 角色 | 职责 |
| --- | --- |
| `root` | 拆解、派单、协调证据与汇总交付 |
| `analyst` | 对目标执行分析，在任务范围内选择与切换方法 |
| `verifier` | 独立复核其他 Agent 的结论并关联实际验证证据 |

运行时只有以上三类角色。创建子任务时使用 `role="analyst"` 或 `role="verifier"`；通过 `skills=["recon", "reverse"]` 等选择专项能力。`hunter`、`fuzz`、`crash`、`exploit` 和 `reporter` 也是能力名称。角色指令自动安装，不通过 `load_skill` 切换身份。

<a id="collaboration-and-lifecycle"></a>

### 协作与生命周期

根 Agent 专注编排，实际目标操作由动态子 Agent 完成。分析笔记、待办、发现和覆盖记录在同一任务内共享；威胁模型仅在安全边界分析中按需使用。每个 Agent 保留独立会话，完整工具输出保存为哈希产物，模型接收有界结果及证据 ID。

| 工具 | 用途 |
| --- | --- |
| `task` | 创建异步子任务 |
| `wait` | 等待单个可达 Agent 的报告 |
| `wait_for_agents` | 按成功或终止条件收集报告 |
| `send_message_to_agent` | 发送普通信息，不提前结束等待或重开任务 |
| `follow_up_agent` | 给已完成的 Agent 追加工作，保留历史和累计预算 |
| `view_agent_graph` | 查看协作关系与待清理会话 |

同范围的新任务只与活动 Agent 去重；独立验证交给另一个 verifier，普通报告由 root 汇总。工具参数校验、结构化输出、上下文保留与压缩、重试分类及用量计价复用 Rig，MCP 协议复用 rmcp；证据、权限和 worker 会话清理由业务层验收。

子 Agent 完成前须关闭自己的调试与交互会话；根任务完成前须处理全部遗留会话。同任务的其他 Agent 可以关闭已终止 owner 遗留的会话，但不能向其发送命令。

<a id="custom-prompts"></a>

### 自定义提示词

提示词集中于 [`prompts/`](../../prompts)，公共规则位于 `base.md`：

| 目录 | 内容 |
| --- | --- |
| `roles/` | 角色指令与生命周期 |
| `capabilities/` | 专项能力 |
| `methods/` | 格式、类型、算法、状态机、二进制差异等方法 |
| `support/` | 工具与恢复指导 |
| `examples/` | 分析示例 |

[`src/prompts.rs`](../../src/prompts.rs) 统一注册内置名称、用途、正文和示例引用；模型仅获取能力目录与已选择的正文。目录分类不改变 skill 调用名。

可通过 `skills_dir/<name>.md` 覆盖内置正文或提供自定义方法，通过 `demos_dir/<name>.md` 提供示例。默认示例目录为 `prompts/examples`，目录缺失时跳过示例。内置正文嵌入程序，不依赖运行时读取仓库文件。

<a id="reports-and-review"></a>

## 报告与复核

```sh
wukong report RUN_ID --output ./reports/assessment-01
wukong verify-bundle ./reports/assessment-01
```

导出目录必须是新目录。报告包包含目标快照、原始证据、配置与复现元数据、JSON、Markdown、HTML 及 SHA256 清单。导出不执行目标；`verify-bundle` 校验文件与映射的完整性。校验和不认证发布者；原始证据不脱敏，配置导出会隐藏凭据字段，不是凭据备份。标题和状态按导出语言显示，已有正文不重写。文件清单和复现方法见 [报告格式](../reference/report-format.zh-CN.md)。

独立复核必须由不同 Agent 执行，并关联实际 worker 结果。静态可疑特征或单纯崩溃不会自动升级为已验证影响。

[`fixtures/`](../../fixtures/README.zh-CN.md) 提供本地评测构建与行为回归定义，区分样本真值、执行观察和 Agent 检测结论；行为回归通过不代表 Agent 分析质量已经通过评测。

<a id="custom-skill-metadata"></a>

### 自定义 Skill 元数据

`skills_dir` 中的自定义 `.md` 会自动出现在 Agent 的能力目录。可添加同名 `.json`，字段为 `description`、`applies_to`、`excludes`、`requires`、`deliverables`、`validation`、`version`；除说明和版本外均为字符串数组。目录标记有效正文的 SHA256 和来源，覆盖内置正文时使用覆盖后的哈希。元数据只指导路由，不授予工具权限；运行前仍须检查实际后端能力。每个元数据文件最多 16 KiB，目录最多 256 项。

<a id="shared-analysis-tools"></a>

### 共用分析工具

`function_instructions` 返回分页指令、所属基本块起点、分支／调用流和 fallthrough；所有地址携带模块 SHA256。Ghidra 查询可设置 `target_evidence_id` 使用本任务已有的另一个二进制证据作为基线，原任务目标保持不变。缺少基线必须显式报告，不能拿当前目标代替。

`differential_test` 接收本任务已登记的 worker `target` 和 `reference` 文件、`reference_python`、`inputs_base64` 和每例 1–10 秒时限；最多 64 个 stdin 输入，每个最多 64 KiB。它通过现有隔离任务生命周期运行，`job_artifacts` 导入 `differential-results.json`，保留输入、原始输出、退出码、两侧哈希和差异。超时、输出截断、非零退出归为 inconclusive。当前协议只比较 stdin；argv／状态会话需单独 harness。

<a id="pwn-method-heap-analysis"></a>

### Pwn 方法：堆分析

`heap_ptmalloc` 已加入内置能力目录。它按实际 allocator 身份、ABI 和版本解释分配／释放时间线，要求 chunk 快照、原语证据、反例和未知项。版本表只是上游检查点，不能替代发行版补丁或运行时观察。来源见 `docs/reference/pwn-knowledge.md`。

<a id="pinning-a-pwn-runtime"></a>

### 固定 pwn 运行环境

先用 `file_put` 登记用户提供的 libc 和 loader，再用 `runtime_profile` 读取目标与运行库 SHA256、ELF Build ID、架构。它默认只确认文件身份。传入自己拥有的 `session_id`（调试器）或 `interactive_session_id`（交互进程）可读取实际映射，并按哈希逐个标记 mapped_hash_confirmed；不能传任意 PID。进程已退出或模块无法读取时不推断匹配。

`runtime_prepare` 要求 target/libc/loader 三个已登记 ELF，创建内容寻址的独立副本，并返回 loader、library-path、target 的直接 argv。将其用于 `interactive_start` 或 `job_start`，不要拼接为 shell 字符串。原始文件不改写；不同 glibc 版本、附加依赖或 loader ABI 仍可能导致启动失败，需记录实际结果。`libc_identify` 可用同进程的多个泄漏比对用户提供的 ELF 候选集；省略候选集时只检查 worker 自身的 libc。匹配仍为 candidate_only，不代表确认映射；无匹配也不能直接认定目标使用了另一版 libc。参数、歧义和验证边界见 [Pwn 证据工具](../reference/pwn-tools.zh-CN.md#libc_identify)。

<a id="structured-crash-evidence"></a>

### 结构化崩溃证据

`crash_snapshot` 读取已停止的所属调试会话或已登记 core 文件；输出保留寄存器、
PC/SP、故障地址、栈、映射和原始调试回执。`cyclic_offset` 必须提供原始 pattern、
alphabet、n 和内存顺序的恰好 n 字节，避免默认 n 或寄存器截断造成假偏移。
`input_minimize` 默认用信号、模块哈希、PC 文件偏移及已解析帧区分崩溃；
缺少 GDB/模块身份时拒绝弱化基线。确需只按信号缩减时显式传
`allow_signal_only=true`，结果标记为弱判据。stdin 的原生复现走管道，调试复现
使用文件重定向；对输入通道敏感的目标需检查二者差异。每次快照可从 job_artifacts 获取。

<a id="load-pwn-methods-on-demand"></a>

### Pwn 方法按需加载

`pwn` 是共享证据与路由入口；`heap_ptmalloc` 处理分配器生命周期，
`rop_analysis` 处理 ABI、栈状态、gadget 语义及各路径前提，`format_string`
区分格式控制、参数访问、泄漏、写入和最终触发。按当前子任务加载，避免一次
灌入所有利用技巧。高级路径必须先证明前提，helper 生成 payload 不代表成功。

<a id="mitigation-evidence-layers"></a>

### 缓解机制证据分层

`mitigation_profile` 分开返回静态声明、worker 环境支持和所属进程的运行态观察；
`mitigation_analysis` 解释这些证据如何约束当前实验。支持 debugger 或 interactive
会话，不能传任意 PID。可选 `address` 仅查询该地址所在映射的权限。
CET ELF note 不等于运行时启用；SHSTK 按线程记录，缺字段保留未知。seccomp
mode 2 不推导 syscall 白名单。环境和映射变化需要重新观察。

工具行从实际记录生成文件名、模块相对地址及反编译函数名。可用的反编译返回值会直接展示带原始证据 ID 的 C 代码预览，最多 12 行/1600 字符；截断会明确标注。原始参数与结果仍按需展开，代码预览不替代独立验证。
