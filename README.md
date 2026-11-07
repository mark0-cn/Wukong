# re-agent

基于 Strix 协作架构、使用 Rust 和 Rig 0.46.0 实现的二进制逆向与分析 agent，支持行为理解、格式与算法还原、调试，以及按需进行漏洞验证。首版动态分析后端面向 Linux x86_64 ELF。ELF、PE、Mach-O 的静态画像由 Rust 直接解析。

需求基线在 [架构方案](docs/strix-rig-binary-agent-plan.md)，实现与验收约定在 [实现契约](docs/implementation-contract.md)。

## 构建

需要 Rust 1.97.1 或更新版本：

```powershell
cargo build --release
cargo test --all-targets
```

Windows 可执行文件位于 `target/release/re-agent.exe`；Linux/macOS 为 `target/release/re-agent`。核心使用 Rig ECS 原生执行循环、会话日志和异步子 agent；模型接口测试通过本地 HTTP 服务验证实际 provider 协议。本轮验证在 macOS 上完成，Windows 控制端尚未验证。

## 开始使用

```powershell
.\target\release\re-agent.exe init
.\target\release\re-agent.exe doctor
.\target\release\re-agent.exe inspect .\sample.exe --strings
```

`inspect` 不执行样本，也不调用模型。`init` 写入配置，已有文件不会被覆盖。配置可参考 [re-agent.example.toml](re-agent.example.toml)；相对文件路径按配置文件所在目录解析。

选择模型、endpoint 和凭据环境变量后启动分析，例如：

```powershell
$env:OPENAI_API_KEY = "your-provider-key"
.\target\release\re-agent.exe --config .\re-agent.toml analyze .\sample.elf --task "分析输入边界，调查具体问题，独立复核并报告证据" --output .\reports\assessment-01
```

支持 `openai`（Chat Completions）、`openai_responses`、`anthropic` 和 `ollama`。凭据通过环境变量注入，不写入配置或检查点。自定义 endpoint 通过 `model.base_url` 指定。`ollama` 默认本地 endpoint，可不配置凭据。根 agent 始终使用 `model.model`；设置 `model.role_model` 后，各角色（recon/reverse/crash/exploit 等）子 agent 改用该模型名（同一 provider/endpoint/凭据），例如 `role_model = "deepseek-flash"` 搭配根模型的 `"deepseek-v4-pro"`。啰嗦的角色模型可用 `model.role_max_tokens` 单独调大单轮输出预算，避免 `finish_reason=Length` 截断。

## 配置 Linux worker

执行工具默认关闭。启用 Docker 或 SSH worker 后，可以使用调试、持久化进程、AFL++ QEMU、崩溃重放、输入最小化和独立验证。

Windows Docker Desktop 可使用：

```powershell
.\scripts\start-worker.ps1
```

该脚本构建 `workers/Dockerfile` 并创建独立 worker 容器和持久化卷。模型凭据留在控制端。详细配置、协议和调试能力见 [worker 文档](workers/runtime/README.md)。Ghidra 本地后端见 [Ghidra 文档](workers/ghidra/README.md)。

```toml
[runtime]
backend = "docker"
docker_container = "re-agent-worker"
remote_bridge_path = "/opt/re-agent/runtime/bridge.py"
remote_state_dir = "/state"
```

`trusted_local` 仅适合明确选择的可信测试样本；该后端直接在所在机器运行进程。日常分析使用隔离 worker。

## 管理与恢复

```powershell
re-agent runs
re-agent status RUN_ID --events 30
re-agent tui RUN_ID
re-agent message AGENT_ID "请记录这条新的分析约束"
re-agent pause RUN_ID
re-agent resume RUN_ID --max-tokens 1000000
re-agent stop AGENT_ID --reason "停止该分支"
re-agent report RUN_ID --output .\reports\assessment-01
re-agent verify-bundle .\reports\assessment-01
re-agent reconcile CALL_ID --result-file .\reviewed-result.json --evidence-id EVIDENCE_ID
```

省略 `--config` 时，会使用当前目录的 `re-agent.toml`（如果存在）。`--state` 可选择已有状态目录。首次模型请求前会保存解析后的 endpoint，包括环境变量提供的默认值。`resume` 使用任务保存的模型与后端配置；原生 Rig 日志保存在状态目录的 `rig/RUN_ID`。活动控制端通过租约互斥。本次实现面向新任务，不迁移旧版自定义 AgentRun 检查点。

Ctrl+C 请求在持久化边界停止调度，状态保留以便恢复。外部 jobs 和调试器分别核对：持久化检查点不能恢复已经消失的调试进程。有副作用且结果未知的第三方 MCP 调用保留未决状态，不能盲目重放。

预算按 provider 返回的用量累计。缺少必要用量时，系统保存响应并暂停，避免把未知消耗计成零。已经发出的并行请求仍可能使观测用量超过阈值；取消或重发的请求也可能产生 provider 未回报的费用。配置的 USD 单价用于估算，并非账单保证。

## 结构

| 模块 | 职责 |
|---|---|
| `kernel` | Rig 原生执行、子任务、消息与日志的业务适配 |
| `coordinator` / `runner` | 任务契约、依赖、消息账本、预算与完成门槛 |
| `llm` / `agents` / `skills` | provider 连接、结构化摘要、角色和上下文 |
| `store` / `domain` | SQLite 事务、检查点、事件、内容哈希产物 |
| `tools` / `mcp` | 工具 schema、角色能力、调用去重、第三方工具 |
| `binary` / `workers/ghidra` | 二进制画像、字符串、函数、反编译、交叉引用 |
| `runtime` / `workers/runtime` | GDB/MI、进程/fuzz jobs、符号执行（angr）、交互会话、崩溃重放与独立复核 |
| `report` / `interface` | CLI、TUI、JSON/Markdown/HTML 报告、证据包 |

根 agent 专注编排，实际目标操作由动态子 agent 完成。分析笔记、待办、发现和覆盖记录在同一任务内共享；威胁模型仅在安全边界分析中按需使用。每个 agent 保留独立会话。活动任务内可向已完成的子 agent 追加问题，保留其历史和累计预算；同范围的新任务只与活动 agent 去重。完整工具输出保存为哈希产物，模型只接收有界结果及证据 ID。

使用 `task` 创建异步子任务，`wait` 等待单个可达 agent 的报告，`wait_for_agents` 按成功或终止条件收集结果。普通信息使用 `send_message_to_agent`，不会提前结束等待或重开任务；已完成任务的新工作使用 `follow_up_agent`。工具参数校验、结构化输出、上下文保留与压缩、重试分类及用量计价复用 Rig，MCP 协议复用 rmcp；证据、权限和 worker 会话清理仍由业务层验收。

子 agent 完成前须关闭自己的调试和交互会话；根任务完成前须处理全部遗留会话。`view_agent_graph` 会列出待清理会话，其他子 agent 可以关闭已终止 owner 遗留的会话，但不能向它发送命令。

## 报告与评测

报告包包含目标快照、原始证据、配置和复现元数据、JSON、Markdown、HTML 及 SHA256 清单。导出不执行目标，完整性检查通过 `verify-bundle` 运行。详见 [报告格式](docs/report-format.md)。

`fixtures/` 提供本地评测构建与行为回归定义；它区分样本真值、执行观察和 agent 检测结论。独立复核必须来自不同 agent，并关联实际 worker 结果。静态可疑特征或单纯崩溃不会自动升级为已验证影响。

首版动态后端以 Linux ELF 为范围。Windows DbgEng 与 macOS LLDB 动态后端属于后续扩展。模型 provider 的实时可用性和外部引擎依赖取决于实际配置，缺少能力会返回明确错误。

## 来源

参照 [Strix](https://github.com/usestrix/strix) commit `06015506625a78b981ae0b34be40239fc1b389be` 的模块职责与协调语义；Rust 实现使用 [Rig v0.46.0](https://github.com/0xPlaygrounds/rig/releases/tag/v0.46.0)。
