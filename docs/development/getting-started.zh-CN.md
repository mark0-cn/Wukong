<a id="build-and-configure-from-source"></a>

# 从源码构建与配置

[English](getting-started.md) · 简体中文

开发 Wukong，或运行尚无正式安装包版本的源码时使用本流程。持续维护的桌面发行目标为 Windows x64 和 Linux x64。正式安装包见[在线安装](../user/online-install.zh-CN.md)，代码边界和检查入口见[维护约定](maintenance.zh-CN.md)。

<a id="prerequisites"></a>

## 准备依赖

- [rust-toolchain.toml](../../rust-toolchain.toml) 固定的 Rust 工具链，当前为 `1.97.1`；`rustup` 会为本仓库选择它。Cargo 的 `rust-version` 是最低要求，不代表所有更新编译器均已验证。
- 满足 [package-lock.json](../../package-lock.json) 的 Node.js/npm：Node 22 分支使用 `22.22.2+`，Node 24 分支使用 `24.15.0+`。
- [Tauri 2 系统依赖](https://v2.tauri.app/start/prerequisites/)，包括 Windows MSVC C++ 构建工具和 Linux 所需 WebKitGTK/GTK 库。
- 动态分析需要支持 Linux 容器的 Docker，至少为镜像和临时层预留 25 GiB；源码工具链下载脚本需要 Python 3.12+。使用 Docker 工具时无需在宿主机安装 GDB/pwntools。
- 可用的模型服务和模型名；通过环境变量名称配置凭据。

当前发布流水线不提供 macOS 正式桌面安装包。首个动态后端面向 Linux x86_64 ELF，原生 DbgEng/LLDB 后端尚未实现。

<a id="clone-and-build-the-client"></a>

## 获取并构建客户端

```sh
git clone https://github.com/mark0-cn/Wukong.git
cd Wukong
npm ci
npm run desktop:build
```

这会生成前端资源和 Rust 桌面客户端。Windows 可执行文件为 `target/release/wukong.exe`，Linux 为 `target/release/wukong`；Tauri 安装文件位于 `target/release/bundle/`。桌面开发使用 `npm run desktop:dev`。在已提交且干净的工作区生成经过入口检查的平台原生安装包，使用 [cargo package-local](releases.zh-CN.md#build-native-installers-locally)。

下文以 `wukong` 表示程序：将其所在目录加入 PATH，或使用实际路径，例如 PowerShell 中的 `.\target\release\wukong.exe`、Linux shell 中的 `./target/release/wukong`。

仅使用 CLI 时可跳过 Node、前端和 Tauri 桌面依赖：

```sh
cargo build --release --no-default-features
```

该程序支持 CLI 命令，`gui` 入口不可用。

<a id="configure-the-model"></a>

## 配置模型

```sh
wukong init wukong.toml
```

`init` 拒绝覆盖已有文件。编辑生成配置的 `[model]`，保留其他内容：

```toml
[model]
provider = "openai_responses"
model = "your-model-name"
api_key_env = "OPENAI_API_KEY"
# 兼容服务可选 API 地址：
# base_url = "https://your-provider.example/v1"
```

将凭据设置到对应环境变量：

```powershell
# Windows PowerShell：仅对当前进程及子进程生效
$env:OPENAI_API_KEY = "your-provider-key"
```

```sh
# Linux shell
export OPENAI_API_KEY="your-provider-key"
```

进程级变量需要从同一 shell 启动程序。Windows 桌面启动也支持已保存的用户/系统环境变量。不要把密钥值写入 TOML。支持 `openai`（Chat Completions）、`openai_responses`、`anthropic`、`ollama`；无凭据本地 Ollama 可省略 `api_key_env`。详见[配置与角色模型](../user/usage.zh-CN.md#configuration-and-models)和[完整示例](../../wukong.example.toml)。

<a id="prepare-docker-tools"></a>

## 准备 Docker 工具

在线首次准备需要与客户端版本匹配的正式 Release 签名清单。尚未发布的源码版本无法从向导获取该清单；可在仓库根目录手动构建并启动工作环境：

```sh
python -m scripts.toolchain.fetch_worker_deps
docker buildx bake --load --set workbench.tags=wukong:0.1
wukong --config wukong.toml env up --image wukong:0.1
wukong --config wukong.toml env doctor
```

**先下载锁定附件，再构建**：`workers/Dockerfile` 挂载 Ghidra/JDK 压缩包并复制 GEF，来源是忽略的 `workers/deps/`，全新克隆不包含这些文件。下载器校验哈希，镜像构建再次校验。上述本地标签用于源码开发，与正式发行的签名镜像 digest 独立。

`env up` 返回生成的工作环境配置路径，通常是相对此配置的 `.wukong/workbench/wukong.toml`，其中包含选定的 worker/容器身份并保留模型设置。使用返回路径启动桌面，或在环境面板选择“使用工具环境”：

```sh
wukong --config .wukong/workbench/wukong.toml gui
```

自定义 `[workbench].root` 时使用实际返回路径。`env doctor` 会运行真实工具探针，包括 32/64 位执行、GDB 和 Ghidra；需要正常运行的 Docker，客户端构建成功本身不证明工具就绪。题目导入、网络和最终验证见 [Pwn 工作环境](../user/pwn-workbench.zh-CN.md)。

独立 [worker 指南](../../workers/runtime/README.zh-CN.md)介绍 Docker/SSH/VM 适配器。`scripts/dev/start-worker.ps1` 创建 `--network none` 的独立 worker，调用前同样需要下载依赖，适用于隔离的本地目标任务。受管理的题目服务及专用网络使用上面的工作环境流程。

<a id="inspect-and-start-analysis"></a>

## 检查文件并开始分析

```sh
wukong --config wukong.toml inspect ./sample.elf --strings
wukong --config .wukong/workbench/wukong.toml doctor
wukong --config .wukong/workbench/wukong.toml analyze ./sample.elf --task "Recover the input format with reviewable evidence"
```

`inspect` 读取二进制，不执行目标或调用模型。示例配置默认禁用执行，选择 worker 后才可使用。动态任务使用生成的 worker 配置，未知目标需要隔离；`trusted_local` 仅适用于可信开发夹具。桌面中选择“新建分析”、目标文件并描述目标即可。

<a id="development-checks"></a>

## 开发检查

桌面改动的 Rust 检查前先生成前端资源：

```sh
npm run build
npm run check
npm test
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

按 [AGENTS](../../AGENTS.zh-CN.md) 和[维护约定](maintenance.zh-CN.md)选择相关检查。浏览器交互测试使用模拟原生边界，真实 Docker、Ghidra 和安装器验收另有入口；仅 CLI 构建不能代替桌面特性验证。
