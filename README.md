<p align="center">
  <img src="icons/icon.png" width="88" alt="Wukong 标志">
</p>

<h1 align="center">Wukong</h1>

<p align="center">简体中文 · <a href="README.en.md">English</a></p>

<p align="center">
  <strong>面向二进制逆向与分析的多 Agent 工作区</strong><br>
  行为理解 · 格式与算法还原 · 动态调试 · 独立验证
</p>

<p align="center">
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/Rust-1.97.1%2B-DEA584?logo=rust" alt="Rust 1.97.1+"></a>
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/Rig-0.46.0-7EE0C3" alt="Rig 0.46.0"></a>
  <a href="tauri.conf.json"><img src="https://img.shields.io/badge/Tauri-2-24C8D8?logo=tauri" alt="Tauri 2"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue" alt="License: Apache-2.0"></a>
</p>

<p align="center">
  <a href="#快速开始">快速开始</a> ·
  <a href="docs/user/usage.md">使用指南</a> ·
  <a href="#文档">文档导航</a> ·
  <a href="https://github.com/mark0-cn/Wukong/issues">反馈问题</a>
</p>

Wukong（悟空）使用 Rust 与 Rig 构建分析核心，通过 Tauri + React 提供桌面工作区，同时保留完整 CLI。描述分析目标后，根 Agent 拆解任务，由分析与验证 Agent 协作完成调查，并交付可追溯的结论、原始证据和报告。

## 目录

- [项目特性](#项目特性)
- [快速开始](#快速开始)
- [使用方式](#使用方式)
- [配置执行环境](#配置执行环境)
- [文档](#文档)
- [项目结构](#项目结构)
- [参与贡献](#参与贡献)
- [致谢](#致谢)
- [许可证](#许可证)

## 项目特性

| 能力 | 说明 |
| --- | --- |
| 二进制分析 | 直接解析 ELF、PE、Mach-O 画像与字符串；通过 Ghidra 查询函数、反编译和交叉引用 |
| 多 Agent 协作 | 根 Agent 编排任务，分析 Agent 调查问题，独立验证 Agent 复核结论；共享笔记、待办和证据 |
| 动态分析 | 隔离 worker 提供 GDB/MI、持久化进程、交互会话、AFL++ QEMU、angr、崩溃重放与输入最小化 |
| 桌面与命令行 | 桌面查看会话、协作任务、发现与证据；CLI 支持启动、干预、恢复和导出 |
| 多语言 | 中英文界面、CLI 帮助和可读报告；新任务输出语言独立设置，见[语言设置](docs/user/languages.md) |
| 持久化与预算 | SQLite 状态、检查点与 Rig 会话日志；支持暂停恢复、并发控制、Token 与费用预算 |
| 证据交付 | 导出 JSON、Markdown、HTML 报告、样本快照、原始证据及 SHA256 清单，并校验报告包完整性 |

> 当前动态分析后端面向 **Linux x86_64 ELF**。PE、Mach-O 支持静态画像；Windows DbgEng 与 macOS LLDB 动态后端尚未实现。

## 快速开始

普通用户从 [GitHub Releases](https://github.com/mark0-cn/Wukong/releases) 下载 Windows `-setup.exe` 或 Linux `.AppImage`，打开后通过环境向导准备 Docker 工具。后续客户端与镜像更新在“更新”中完成，详见[在线安装](docs/user/online-install.md)。以下为源码构建步骤。

### 1. 准备环境

- **Rust 1.97.1+**。
- **Node.js 22.12+ 与 npm**，用于桌面前端构建。
- 对应操作系统的 [Tauri 2 系统依赖](https://v2.tauri.app/start/prerequisites/)。
- 调用模型所需的服务、模型名和凭据；动态分析另需 [执行 worker](#配置执行环境)。

### 2. 获取源码并构建

```sh
git clone https://github.com/mark0-cn/Wukong.git
cd Wukong
npm ci
npm run desktop:build
```

桌面构建会先生成前端资源，再编译 Rust 程序。可执行文件位于：

| 平台 | 路径 |
| --- | --- |
| Windows | `target/release/wukong.exe` |
| Linux / macOS | `target/release/wukong` |

安装包输出到 `target/release/bundle/`。下文使用 `wukong` 表示该可执行文件：可将其所在目录加入 `PATH`，或替换为完整路径，例如 PowerShell 中的 `.\target\release\wukong.exe`。

<details>
<summary>仅构建 CLI</summary>

不需要桌面界面时，可以跳过 Node.js 与前端构建：

```sh
cargo build --release --no-default-features
```

此构建支持 CLI 命令，`gui` 入口不可用。

</details>

### 3. 配置模型

```sh
wukong init
```

编辑生成的 `wukong.toml`，设置 `[model]` 中的模型与服务信息，保留其他配置。`init` 不会覆盖已有文件；完整字段见 [配置示例](wukong.example.toml)。

```toml
[model]
provider = "openai_responses"
model = "your-model-name"
api_key_env = "OPENAI_API_KEY"
# 使用兼容服务时指定 API 地址：
# base_url = "https://your-provider.example/v1"
```

将密钥设置到对应的环境变量中：

```powershell
# Windows PowerShell
$env:OPENAI_API_KEY = "your-provider-key"
```

```sh
# Linux / macOS
export OPENAI_API_KEY="your-provider-key"
```

支持 `openai`（Chat Completions）、`openai_responses`、`anthropic` 和 `ollama`。凭据通过环境变量读取，不写入配置或检查点；本地 Ollama 可使用无凭据配置。角色模型与配置查找规则见 [使用指南](docs/user/usage.md#配置与模型)。

### 4. 检查环境并开始分析

```sh
wukong doctor
wukong inspect ./sample.exe --strings
wukong gui
```

`doctor` 检查配置与已安装能力；`inspect` 直接读取二进制，不执行样本，也不调用模型。在桌面工作区选择“新建分析”，选择样本并描述任务即可开始。

## 使用方式

### 桌面工作区

双击 `wukong.exe` 或运行 `wukong gui` 打开工作区，无需另开浏览器。

- 搜索任务记录，选择或拖入目标，编辑分析任务。
- 查看 Agent 会话、工具操作、协作任务、发现与证据。
- 向选中的 Agent 补充信息或安排后续任务。
- 暂停、调整预算后恢复、停止分支，以及导出报告与证据包。
- 在“工作区设置”中编辑模型、预算与后端，或切换到 TOML 源码编辑完整配置。

详细操作与 Windows 发布说明见 [桌面工作区指南](docs/user/usage.md#桌面工作区)。

### 命令行

```sh
# 启动分析并导出到一个新目录
wukong analyze ./sample.elf --task "还原输入格式与关键算法，给出可复核的证据" --output ./reports/analysis-01

# 查看任务与最近事件
wukong runs
wukong status RUN_ID --events 30

# 暂停与恢复
wukong pause RUN_ID
wukong resume RUN_ID --max-tokens 1000000

# 导出与校验报告包
wukong report RUN_ID --output ./reports/analysis-02
wukong verify-bundle ./reports/analysis-02
```

`RUN_ID` 为任务 ID。完整选项可通过 `wukong --help` 或 `wukong <command> --help` 查看；消息、分支停止与恢复约定见 [任务管理指南](docs/user/usage.md#任务管理与恢复)。

## 配置执行环境

执行工具默认关闭。启用 Docker 或 SSH worker 后，可使用调试、模糊测试、崩溃重放和独立验证。Windows Docker Desktop 可在仓库根目录运行：

```powershell
.\scripts\dev\start-worker.ps1
```

脚本构建 worker 镜像，创建独立容器与持久化卷。随后修改 `wukong.toml` 中的 `[runtime]`：

```toml
[runtime]
backend = "docker"
docker_container = "wukong-worker"
remote_bridge_path = "/opt/wukong/runtime/bridge.py"
remote_state_dir = "/state"
```

运行 `wukong doctor` 确认实际可用能力。模型凭据留在控制端；`trusted_local` 会直接在本机运行进程，仅适用于可信测试样本。

- [执行 worker](workers/runtime/README.md)：Docker、SSH / VM 部署、协议与验收。
- [Ghidra 后端](workers/ghidra/README.md)：本地 Ghidra / JDK 配置、查询与缓存。

## 文档

完整分类见 [文档索引](docs/README.md)。

| 文档 | 内容 |
| --- | --- |
| [使用指南](docs/user/usage.md) | 桌面操作、模型配置、任务恢复、预算与 Agent 协作 |
| [配置示例](wukong.example.toml) | 模型、角色、预算、worker 与工具配置 |
| [工具目录](docs/reference/tool-catalog.md) | 从工具注册表生成的完整工具与参数说明 |
| [实现架构](docs/design/architecture.md) | 模块边界、执行循环、状态与协作语义 |
| [架构方案](docs/history/strix-rig-binary-agent-plan.md) / [实现契约](docs/design/implementation-contract.md) | 需求基线与验收约定 |
| [报告格式](docs/reference/report-format.md) | 报告包结构、证据、完整性校验与复现 |
| [维护约定](docs/development/maintenance.md) | 开发检查、工具文档生成与性能基准 |
| [评测夹具](fixtures/README.md) / [验收记录](docs/history/validation/2026-11-06/README.md) | 本地行为回归、独立复核与已有验证证据 |

## 项目结构

```text
Wukong/
├── src/                    # Rust 分析核心、CLI 与桌面宿主
├── ui/                     # React + TypeScript 桌面界面
├── prompts/                # 角色、能力、方法与示例提示词
├── workers/                # Python 执行 worker 与 Ghidra 桥接
├── templates/              # 报告模板
├── tests/                  # Rust 集成测试、自动化测试与真实环境验收
├── fixtures/               # 可信回归样本、任务与人工真值
├── evals/                  # 行为评测、评分器与评测测试
├── scripts/                # CI、发布、工具链和开发脚本
├── packaging/              # 随在线包分发的安装与回滚脚本
├── docs/                   # 使用、架构与验收文档
└── wukong.example.toml    # 配置示例
```

## 参与贡献

欢迎通过 [Issue](https://github.com/mark0-cn/Wukong/issues) 报告问题、讨论功能，或提交 [Pull Request](https://github.com/mark0-cn/Wukong/pulls)。问题报告请包含复现步骤、操作系统、构建方式与相关日志。

桌面开发模式：

```sh
npm ci
npm run desktop:dev
```

提交代码改动前运行相关检查。Rust 检查使用默认桌面特性时，先生成前端资源：

```sh
npm run build
npm run check
npm test
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

worker 测试、环境依赖和性能检查见 [维护约定](docs/development/maintenance.md)；评测时应区分样本行为回归与 Agent 分析质量，独立复核须关联实际证据。

## 致谢

- [Strix](https://github.com/usestrix/strix)：参考其模块职责与协调语义，基线 commit 为 `06015506625a78b981ae0b34be40239fc1b389be`。
- [Rig](https://github.com/0xPlaygrounds/rig)：模型连接、ECS 执行循环、会话日志与异步子 Agent 支持，当前固定使用 `0.46.0`。

## 许可证

本项目采用 [Apache License 2.0](LICENSE)。来源与第三方声明见 [NOTICE](NOTICE)；外部分析引擎遵循各自的上游许可证。

## Pwn 工作环境

发行版通过 GitHub Release 提供 Windows 在线安装包，安装器从公开 Docker Hub 仓库 `hlmark0/wukong` 拉取固定 digest 的工具镜像。见 [在线安装与升级](docs/user/online-install.md)和 [CI/tag 发布流程](docs/development/releases.md)。

依赖更新、上游工具版本检查、不可变 Release 与运行摘要见 [GitHub 维护功能](docs/development/github-maintenance.md)。

工具 Docker 与题目 Docker 由宿主机统一管理，每题独立网络。使用 `env up/doctor`、`challenge import/prepare/build/up` 和离线迁移命令；桌面左下角提供“工具环境”和“题目环境”入口。详见 [Pwn 工作环境](docs/user/pwn-workbench.md)。
