<p align="center">
  <img src="icons/icon.png" width="88" alt="Wukong logo">
</p>

<h1 align="center">Wukong</h1>

<p align="center">English · <a href="README.zh-CN.md">简体中文</a></p>

<p align="center">
  <strong>A multi-agent workspace for binary reverse engineering and analysis</strong><br>
  Behavior analysis · Format and algorithm recovery · Dynamic debugging · Independent verification
</p>

<p align="center">
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/Rust-1.97.1%2B-DEA584?logo=rust" alt="Rust 1.97.1+"></a>
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/Rig-0.46.0-7EE0C3" alt="Rig 0.46.0"></a>
  <a href="tauri.conf.json"><img src="https://img.shields.io/badge/Tauri-2-24C8D8?logo=tauri" alt="Tauri 2"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue" alt="License: Apache-2.0"></a>
</p>

<p align="center">
  <a href="#quick-start">Quick start</a> ·
  <a href="docs/user/usage.md">User guide</a> ·
  <a href="#documentation">Documentation</a> ·
  <a href="https://github.com/mark0-cn/Wukong/issues">Report an issue</a>
</p>

Wukong (悟空) combines a Rust/Rig analysis core with a Tauri/React desktop workspace and a complete CLI. Describe your objective; a root Agent breaks it into tasks, and analysis and verification Agents investigate together, delivering traceable conclusions, original evidence and reports.

<a id="目录"></a>

## Contents

- [Features](#features)
- [Quick start](#quick-start)
- [Usage](#usage)
- [Execution environment](#execution-environment)
- [Documentation](#documentation)
- [Repository layout](#repository-layout)
- [Contributing](#contributing)
- [Acknowledgments](#acknowledgments)
- [License](#license)

<a id="项目特性"></a>

## Features

| Capability | Description |
| --- | --- |
| Binary analysis | Read ELF, PE and Mach-O profiles and strings directly; query functions, decompilation and cross-references through Ghidra |
| Agent collaboration | Root orchestration, analyst investigation and independent verifier review; shared notes, checklists and evidence |
| Dynamic analysis | Isolated workers provide GDB/MI, persistent processes, interactive sessions, AFL++ QEMU, angr, crash replay and input minimization |
| Desktop and CLI | Inspect conversations, tasks, findings and evidence; start, intervene, resume and export from the CLI |
| Languages | English and Chinese UI, CLI help and readable reports; independent output preferences for new tasks; see [language settings](docs/user/languages.md) |
| Persistence and budgets | SQLite state, checkpoints and Rig conversation logs; pause/resume, concurrency, token and cost limits |
| Evidence delivery | JSON, Markdown and HTML reports, sample snapshots, original evidence and SHA256 manifests with bundle integrity checks |

> Dynamic analysis currently targets **Linux x86_64 ELF**. PE and Mach-O support static profiling; Windows DbgEng and macOS LLDB dynamic backends are not implemented.

<a id="快速开始"></a>

## Quick start

Download the Windows `-setup.exe` or Linux `.AppImage` from [GitHub Releases](https://github.com/mark0-cn/Wukong/releases). Prepare Docker tools in the first-run wizard; manage subsequent client and image upgrades in **Updates**. See [online installation](docs/user/online-install.md). The steps below build from source.

<a id="1-准备环境"></a>

### 1. Prerequisites

- **Rust 1.97.1+**.
- **Node.js satisfying the locked dependencies**, with npm. For the CI Node 22 line, use **22.22.2 or later**; Node 24 requires **24.15.0 or later**.
- [Tauri 2 system prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.
- A model service, model name and credentials; dynamic analysis also requires an [execution worker](#execution-environment).

<a id="2-获取源码并构建"></a>

### 2. Clone and build

```sh
git clone https://github.com/mark0-cn/Wukong.git
cd Wukong
npm ci
npm run desktop:build
```

The desktop build generates frontend assets, then compiles Rust. Executable locations:

| Platform | Path |
| --- | --- |
| Windows | `target/release/wukong.exe` |
| Linux / macOS | `target/release/wukong` |

Installer output is in `target/release/bundle/`. Commands below use `wukong`: add its directory to `PATH` or use the full path, such as `.\target\release\wukong.exe` in PowerShell.

<details>
<summary>Build only the CLI</summary>

If you do not need the desktop, skip Node.js and the frontend build:

```sh
cargo build --release --no-default-features
```

This build supports CLI commands but not `gui`.

</details>

<a id="3-配置模型"></a>

### 3. Configure a model

```sh
wukong init
```

Edit `[model]` in the generated `wukong.toml`, retaining other configuration. `init` never overwrites an existing file. See the [configuration example](wukong.example.toml) for all fields.

```toml
[model]
provider = "openai_responses"
model = "your-model-name"
api_key_env = "OPENAI_API_KEY"
# Set the API URL for a compatible service:
# base_url = "https://your-provider.example/v1"
```

Set the credential in the corresponding environment variable:

```powershell
# Windows PowerShell
$env:OPENAI_API_KEY = "your-provider-key"
```

```sh
# Linux / macOS
export OPENAI_API_KEY="your-provider-key"
```

Supported providers are `openai` (Chat Completions), `openai_responses`, `anthropic` and `ollama`. Credentials are read from environment variables, not stored in configuration or checkpoints; local Ollama supports credential-free configuration. See [configuration and models](docs/user/usage.md#configuration-and-models) for role models and discovery rules.

<a id="4-检查环境并开始分析"></a>

### 4. Check and analyze

```sh
wukong doctor
wukong inspect ./sample.exe --strings
wukong gui
```

`doctor` checks configuration and installed capabilities. `inspect` reads a binary without executing it or calling a model. Select **New analysis**, choose a sample and describe your task.

<a id="使用方式"></a>

## Usage

<a id="桌面工作区"></a>

### Desktop workspace

Double-click `wukong.exe` or run `wukong gui`; no separate browser is needed.

- Search task history, select or drop a target, and edit the analysis request.
- Inspect Agent conversations, tool operations, collaboration, findings and evidence.
- Send information or assign follow-up work to the selected Agent.
- Pause, adjust budgets and resume; stop branches; export reports and evidence bundles.
- Edit models, budgets and backends in **Workspace settings**, or use the TOML editor for the complete configuration.

See the [desktop guide](docs/user/usage.md#desktop-workspace) for operation and Windows deployment details.

<a id="命令行"></a>

### Command line

```sh
# Start analysis and export to a new directory
wukong analyze ./sample.elf --task "Recover the input format and key algorithms with reviewable evidence" --output ./reports/analysis-01

# Inspect tasks and recent events
wukong runs
wukong status RUN_ID --events 30

# Pause and resume
wukong pause RUN_ID
wukong resume RUN_ID --max-tokens 1000000

# Export and verify a report bundle
wukong report RUN_ID --output ./reports/analysis-02
wukong verify-bundle ./reports/analysis-02
```

Replace `RUN_ID` with the task ID. Use `wukong --help` or `wukong <command> --help` for options. See [task management and recovery](docs/user/usage.md#task-management-and-recovery) for messaging, stopping branches and resuming.

<a id="配置执行环境"></a>

## Execution environment

Execution tools are disabled by default. Enable a Docker or SSH worker for debugging, fuzzing, crash replay and independent verification. With Windows Docker Desktop, run from the repository root:

```powershell
.\scripts\dev\start-worker.ps1
```

The script builds a worker image and creates an isolated container and persistent volume. Update `[runtime]` in `wukong.toml`:

```toml
[runtime]
backend = "docker"
docker_container = "wukong-worker"
remote_bridge_path = "/opt/wukong/runtime/bridge.py"
remote_state_dir = "/state"
```

Run `wukong doctor` to confirm actual capabilities. Model credentials stay on the control host. `trusted_local` executes processes directly on the host and is only for trusted test samples.

- [Execution worker](workers/runtime/README.md): Docker, SSH/VM deployment, protocol and acceptance.
- [Ghidra backend](workers/ghidra/README.md): local Ghidra/JDK configuration, queries and caching.

<a id="文档"></a>

## Documentation

See the full [documentation index](docs/README.md).

| Document | Contents |
| --- | --- |
| [User guide](docs/user/usage.md) | Desktop, models, recovery, budgets and Agent collaboration |
| [Configuration example](wukong.example.toml) | Models, roles, budgets, workers and tools |
| [Tool catalog](docs/reference/tool-catalog.md) | Registry-generated tools and descriptions |
| [Architecture](docs/design/architecture.md) | Modules, execution loop, state and coordination |
| [Original design](docs/history/strix-rig-binary-agent-plan.md) / [implementation contract](docs/design/implementation-contract.md) | Requirements baseline and acceptance rules |
| [Report format](docs/reference/report-format.md) | Bundle structure, evidence, integrity and reproduction |
| [Maintenance](docs/development/maintenance.md) | Development checks, generated documentation and benchmarks |
| [Fixtures](fixtures/README.md) / [validation records](docs/history/validation/2026-11-06/README.md) | Behavior regressions, independent review and recorded evidence |

<a id="项目结构"></a>

## Repository layout

```text
Wukong/
├── src/                    # Rust analysis core, CLI and desktop host
├── ui/                     # React + TypeScript desktop UI
├── prompts/                # Roles, capabilities, methods and examples
├── workers/                # Python execution worker and Ghidra bridge
├── templates/              # Report templates
├── tests/                  # Rust, automation and real-environment acceptance
├── fixtures/               # Trusted samples, tasks and human ground truth
├── evals/                  # Behavior evaluation, scorers and evaluation tests
├── scripts/                # CI, release, toolchain and development scripts
├── packaging/              # Online-package installation and recovery scripts
├── docs/                   # User, architecture and acceptance documentation
└── wukong.example.toml     # Configuration example
```

<a id="参与贡献"></a>

## Contributing

Use [Issues](https://github.com/mark0-cn/Wukong/issues) for bugs or feature discussions, or submit a [Pull Request](https://github.com/mark0-cn/Wukong/pulls). Include reproduction steps, OS, build method and relevant logs.

Desktop development:

```sh
npm ci
npm run desktop:dev
```

Run checks relevant to your change. Build frontend assets before Rust checks using default desktop features:

```sh
npm run build
npm run check
npm test
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

See [maintenance](docs/development/maintenance.md) for worker tests, dependencies and benchmarks. Distinguish sample behavior regression from Agent analysis quality; independent review must reference actual evidence. Browser tests use a simulated native boundary; real container and installer acceptance are separate.

<a id="致谢"></a>

## Acknowledgments

- [Strix](https://github.com/usestrix/strix): module responsibilities and coordination semantics, baseline commit `06015506625a78b981ae0b34be40239fc1b389be`.
- [Rig](https://github.com/0xPlaygrounds/rig): model connections, ECS execution, conversation logs and asynchronous child Agents; pinned to `0.46.0`.

<a id="许可证"></a>

## License

Licensed under [Apache License 2.0](LICENSE). See [NOTICE](NOTICE) for provenance and third-party notices. External analysis engines retain their upstream licenses.

<a id="pwn-工作环境"></a>

## Pwn workbench

GitHub Releases provide Windows and Linux online installers. First setup downloads the digest-pinned tool image from public Docker Hub repository `hlmark0/wukong`. See [installation and updates](docs/user/online-install.md) and [CI/tag releases](docs/development/releases.md).

See [GitHub maintenance](docs/development/github-maintenance.md) for dependency updates, upstream tool checks, immutable releases and run summaries.

The host manages sibling tool and challenge containers, with a dedicated network per challenge. Use `env up/doctor`, `challenge import/prepare/build/up` and offline migration commands, or the desktop tool/challenge environment entries. Tools connect through `challenge:port`; their `localhost` refers to themselves. Debugging may require extracted runtime files or a dedicated debug container. Final verification uses the original challenge environment. See [Pwn workbench](docs/user/pwn-workbench.md).

Language changes share the same installation, update manifest, version and image digest; they do not change evidence, credentials or challenge data.
