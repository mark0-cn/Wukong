<a id="从源码构建与配置"></a>

# Build and configure from source

English · [简体中文](getting-started.zh-CN.md)

Use this path to develop Wukong or run a checkout whose version has no published installer. Windows x64 and Linux x64 are the maintained desktop distribution targets. See [installation](../user/online-install.md) for published installers and [maintenance](maintenance.md) for code boundaries and checks.

<a id="准备依赖"></a>

## Prerequisites

- The exact Rust toolchain in [rust-toolchain.toml](../../rust-toolchain.toml), currently `1.97.1`; `rustup` selects it for this checkout. Cargo's `rust-version` is the minimum, not a promise that every newer compiler has been validated.
- Node.js/npm satisfying [package-lock.json](../../package-lock.json). On Node 22 use `22.22.2+`; on Node 24 use `24.15.0+`.
- [Tauri 2 system prerequisites](https://v2.tauri.app/start/prerequisites/), including MSVC C++ build tools on Windows and the required WebKitGTK/GTK libraries on Linux.
- For dynamic analysis: Docker with Linux containers, at least 25 GiB free for images/temporary layers, and Python 3.12+ to run source toolchain download scripts. Host GDB/pwntools installations are not needed with Docker tools.
- A supported model service and model name; configure credentials through environment variable names.

macOS has no official desktop installer in this release pipeline. The first dynamic backend targets Linux x86_64 ELF; native DbgEng/LLDB backends are not implemented.

<a id="获取并构建客户端"></a>

## Clone and build the client

```sh
git clone https://github.com/mark0-cn/Wukong.git
cd Wukong
npm ci
npm run desktop:build
```

This builds frontend assets and the Rust desktop client. The executable is `target/release/wukong.exe` on Windows or `target/release/wukong` on Linux. Tauri installer output is in `target/release/bundle/`. Use `npm run desktop:dev` for desktop development. To produce a checked, platform-native local installer, use [cargo package-local](releases.md#build-native-installers-locally) from a committed, clean checkout.

The remaining commands use `wukong`. Add its executable directory to PATH or use its actual path: for example, `.\target\release\wukong.exe` in PowerShell or `./target/release/wukong` in a Linux shell.

For CLI-only work, skip Node/frontend/Tauri desktop dependencies:

```sh
cargo build --release --no-default-features
```

That executable supports CLI commands; the `gui` entry is unavailable.

<a id="配置模型"></a>

## Configure the model

```sh
wukong init wukong.toml
```

`init` refuses to overwrite an existing file. Edit `[model]` in the generated configuration, retaining the rest:

```toml
[model]
provider = "openai_responses"
model = "your-model-name"
api_key_env = "OPENAI_API_KEY"
# Optional API URL for a compatible service:
# base_url = "https://your-provider.example/v1"
```

Set the credential in the corresponding environment variable:

```powershell
# Windows PowerShell: applies to this process and its children
$env:OPENAI_API_KEY = "your-provider-key"
```

```sh
# Linux shell
export OPENAI_API_KEY="your-provider-key"
```

Launch from that shell when using a process-local variable. Windows desktop launches also support saved user/system environment variables. Do not put the key value in TOML. Providers include `openai` (Chat Completions), `openai_responses`, `anthropic` and `ollama`; credential-free local Ollama can omit `api_key_env`. See [configuration and role models](../user/usage.md#configuration-and-models) and the [complete example](../../wukong.example.toml).

<a id="准备-docker-工具"></a>

## Prepare Docker tools

Online first-run setup needs the signed manifest from the official Release matching the client version. An unpublished source version cannot obtain that manifest through the setup wizard. For that checkout, build tools and start the workbench manually from the repository root:

```sh
python -m scripts.toolchain.fetch_worker_deps
docker buildx bake --load --set workbench.tags=wukong:0.1
wukong --config wukong.toml env up --image wukong:0.1
wukong --config wukong.toml env doctor
```

Download the locked attachments **before** building: `workers/Dockerfile` mounts Ghidra/JDK archives and copies GEF from ignored `workers/deps/`. A fresh clone does not include them. The downloader verifies checksums; the image build verifies them again. The local tag above is a source-development tag, independent of official signed release digests.

`env up` returns the generated workbench configuration path, normally `.wukong/workbench/wukong.toml` relative to this configuration. It includes the selected worker/container identity and preserves your model settings. Use that returned path to launch the desktop, or choose **Use tool environment** in the environment panel:

```sh
wukong --config .wukong/workbench/wukong.toml gui
```

Use the returned path if `[workbench].root` was customized. `env doctor` runs actual tool probes, including 32/64-bit execution, GDB and Ghidra. It requires a working Docker daemon; a successful client build alone does not establish tool readiness. See [Pwn workbench](../user/pwn-workbench.md) for challenge import, networking and final verification.

The standalone [worker guide](../../workers/runtime/README.md) covers Docker/SSH/VM adapters. `scripts/dev/start-worker.ps1` creates a separate worker with `--network none`; download dependencies before invoking it. It is a deployment option for isolated local target work. Use the workbench flow above for managed challenge services and their dedicated networks.

<a id="检查文件并开始分析"></a>

## Inspect and start analysis

```sh
wukong --config wukong.toml inspect ./sample.elf --strings
wukong --config .wukong/workbench/wukong.toml doctor
wukong --config .wukong/workbench/wukong.toml analyze ./sample.elf --task "Recover the input format with reviewable evidence"
```

`inspect` reads the binary without running it or calling a model. The example configuration leaves execution disabled until a worker is selected. Dynamic work uses the generated worker configuration; unknown targets require isolation. `trusted_local` is restricted to trusted development fixtures. In the desktop, use **New analysis**, select the target and describe the objective.

<a id="开发检查"></a>

## Development checks

For desktop changes, generate frontend assets before Rust checks:

```sh
npm run build
npm run check
npm test
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

Select checks by impact using [AGENTS](../../AGENTS.md) and [maintenance](maintenance.md). Browser interaction tests use a simulated native boundary; actual Docker, Ghidra and installer acceptance have separate entry points. A CLI-only build does not replace desktop-feature verification.
