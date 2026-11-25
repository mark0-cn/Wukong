# 原生 Windows Worker 组件

[English](README.md) · 简体中文

Windows 执行要求专用、可丢弃的目标 VM。接受目标影响整个 guest（包括 worker）；宿主访问、共享目录／设备及对外网络由虚拟化层限制。控制器／模型凭据与签名私钥留在 guest 外，原始证据保留在控制器，不可信执行后重置或丢弃 VM。worker 不创建 VM、不配置虚拟化层策略，也不自动管理快照。

签名部署记录要求必填 `"execution_boundary": "disposable_vm"`。签名记录 schema、签名 CLI、离线介质准备、安装器及服务均执行此约束，拒绝缺失、null 或不支持的边界。按专用、可丢弃 VM 全新部署，不提供迁移或旧部署记录兼容。

启动分为部署身份检查、资源恢复、依赖自检、guest 控制自检及通信就绪阶段。各阶段发布开始和成功／失败事件，记录耗时和稳定错误码。受保护的 `metadata/startup-diagnostics.json` 使用 schema 2，原子替换、上限 16 KiB，绑定安装身份、已验证且可取得的包／启动身份及新的启动尝试身份。`last-service-error.txt` 同步刷新为同次尝试（包括等待／就绪状态）。固定摘要不复制任意异常文本。安装失败携带当前诊断身份与错误码；hosted 验收分别验证完整报告和安装结果。

项目不再订阅或匹配 WFP 事件，启动不再要求回环阻断证据。AppContainer／令牌、ACL、进程归属、清理及调试器检查仍作为 guest 内部控制。每次启动的新 schema-5 资格回执采用 `scope: "disposable_vm_guest_controls"`。报告和 `doctor` 声明 `execution_boundary: "disposable_vm"`、`external_isolation_verified: false`；`doctor.guest_controls_verified` 表示本次启动检查结果。这些检查不能认证虚拟化层隔离，也不能保证 guest 被攻陷后的证据真实性。Windows 内部仍可能使用系统 WFP 能力；此修改不关闭系统防火墙。

密封的启动调试器夹具遇到异常停止时，受保护的宿主日志保留有界诊断：架构、预期／实际地址、异常标志及首帧身份。各选定字符串限制为 192 个标识符字符，不复制任意 MCP 输出或目标内容。结构化启动报告仍使用固定阶段错误码和摘要。

hosted 服务重启验收在重启前后核对有界报告及匹配的 `startup_ready` 文本摘要，并要求新的启动尝试身份。摘要文件存在本身不代表错误；身份过期、启动等待中或运行故障摘要仍会使验收失败。

密封夹具允许继续一次实时、首次机会、可继续的原生断点，且异常地址、首帧地址及当前停止地址必须一致，首帧归属于 `ntdll`；x86 另外允许一次 WOW64 加载器通知。离线导出符号可能将原生加载器断点归属到附近的导出函数，因此资格检查不要求特定系统函数名或零符号偏移。重复停止、地址不符、其他模块／故障及保存的崩溃上下文仍会被拒绝。最终仍必须命中夹具自身由 PDB 解析的断点，并验证寄存器和栈一致。普通目标不使用此放行逻辑。参见微软的[初始断点说明](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/initial-breakpoint)。

## 职责与请求链路

原生 Rust Worker 是 Windows 上唯一的执行管理入口。托管 SSH 调用固定 relay，经命名管道认证已登记的 SCM 服务。服务负责启动、资格检查、恢复和关闭，`native/rpc/worker.rs` 分发公开 API。不再存在常驻 Python 元数据 Worker、内层 Python 租约、回环 Worker 端点或反向 broker-health 通道。

| 组件 | 职责 |
| --- | --- |
| 控制器 / Rig | Agent 调度、角色、持久化业务请求、证据与独立验证 |
| `workers/protocol` | 共用身份、租约、错误和容量类型；公开 JSON-RPC 语义 |
| `native/core/` | 所有权身份及不可变资源类型，不依赖 RPC、存储或 OS API |
| `native/host/` | 安装、SCM 生命周期、已验证运行上下文及启动资格检查 |
| `native/rpc/` | JSON-RPC 注册、传输、启动身份绑定及控制器／运行隔离 |
| `native/resources/` | 单一资源管理器拥有准入、操作票据、账本及恢复 |
| `native/executors/` | 进程／会话计划、受控执行观察及结果适配；不修改账本 |
| `native/artifacts/` | 租约范围内的不可变输入、运行／资源范围内的输出及受限保护文件 IO |
| `native/isolation/` | Windows Token、AppContainer、Job、ACL 及进程启动 |
| 外部 WinDbg MCP / Python | 在受控资源内执行调试或有界分析，不独立管理 Worker |

RPC 在调用管理器前，将已注册处理器解析为资源操作；资源和制品模块不查询 RPC 注册表。`executors/results.rs` 校验并发布进程/重放观察，管理器独占清理和账本状态转换的提交。Shell 和分析适配器负责各自的响应投影。共享协议提供有类型的结果/清理观察；原生账本保留更严格的 OS 清理证明。

公开传输继续使用 JSON-RPC 2.0。控制器使用 `jsonrpsee`，共用 Rust 类型使用 Serde。内部会话消息采用 Prost 生成的 protobuf 和 Tokio 帧处理，外部调试器使用 `rmcp`。原始调试回复保留为带哈希的证据，不重新实现一套调试协议。Linux 保留现有 Python Worker。

`ping`、`doctor` 由原生层直接回答。只有签名包检查、遗留资源恢复、离线依赖检查和本次服务启动的新 guest 控制／调试器资格检查全部完成，服务才开放已注册的执行能力。`doctor.readiness` 说明当前状态。历史资格回执不能替代新启动检查。注册表只声明可调用的原生执行或隔离分析方法；Windows 不声明 Linux GDB 命令、原始调试命令、AFL/QEMU、core 文件快照和运行时准备能力。崩溃判定使用所属 DbgEng 会话及 `crash_replay`/`input_minimize`；`poc_verify` 仅重放 argv/stdin，映射模块内容保持未确认，因为内核报告的路径不是内容哈希。

## 租约、容量与持久化状态

原生 Worker 管理一份 120 秒的运行/控制器租约。控制器通过独立传输每 15 秒续租。受保护的 `metadata/lease-epochs/` 在发放令牌前保留单调递增 epoch，使用独占写入句柄，上限为 100,000 个 epoch。续租保持同一租约标识。访问缓存副作用前检查身份和租约。过期会请求取消，但不能证明资源已清理；活动请求/资源阻止接管和释放。

`doctor.capacity.max_active_resources` 当前为 **1**，由作业、持久会话和分析辅助进程共享。SQLite 唯一约束强制执行此容量。容量被占用时，Worker 在接纳新副作用之前返回 `resource_busy`。控制器通过 Tokio FIFO 准入互斥锁和有界重试等待；等待期间仍可观测、关闭、取消和续租。启动另一资源前必须关闭活动会话。传输失败和未知结果不会自动重放。控制器业务账本和 Worker 资源账本保持各自职责。

资源账本使用 schema **2**，拒绝旧数据库，不迁移或删除。协议 v3 应使用新的 Worker 安装实例，并保留旧实例与证据。显式生命周期状态保证进程、会话和恢复操作不能同时占用唯一槽位。输入准备在资源锁外运行，并持有独立的输入锁；准入随后重新核对租约和容量。管理器预留操作、签发票据，在锁外启动自有 Tokio 任务，再按原票据提交观察结果。启动、进程退出及恢复等待的归属不依赖发起 RPC 是否仍在等待。查询和取消可观察尚未完成的启动或失败的清理；失败不会释放账本中的资源占用。

上传只在受保护的原生存储中落盘一次。`file.put` 最大 32 MiB；续传使用 1 MiB 块，最大 16 GiB。上传声明、内容 SHA256、可执行策略和发布回执绑定 Worker 身份、运行、控制器和租约 epoch。重复块必须与原始字节一致；状态报告原生持久化偏移。读取和执行都要求完整回执。工具需要输入时才复制到其受控临时目录；普通账户的文件回执不能授权 System 读取。原始附件保持不变。

## 执行与分析

`job.*`、`debug.*`、`interact.*`、`exec.run`、`py.run`、重放、最小化和验证共用原生资源生命周期及账本。重复请求先解析原始意图，再访问后继输入。清理失败继续保留所有权；除非原始证据能证明结果，中断执行保持未知。制品访问限制在运行范围内，并保留大小/SHA256 身份。资源结果描述隔离方式；只有 `doctor` 报告当前服务就绪状态。

`runtime.profile` 直接读取受保护原生存储中已准入内容的身份，不占用执行槽。`debug.profile` 和 `interact.profile` 仅通过会话持有的进程句柄观察状态，保留原始证据并返回有界预览。无法取得的观察仍为未知；映射路径不能证明文件哈希。静态二进制声明仍由控制端负责。

`seccomp.analyze` 通过一次性 AppContainer Python 进程调用包内分析代码。辅助进程只使用已准入输入，期限为 120 秒，输出上限为 8 MiB，并保留执行回执和 stdout 哈希。它不构造 Python worker runtime，不拥有租约、守护进程、会话或第二套作业账本。

当前离线包没有提供 pwntools、pyelftools 和 Unicorn，因此不注册或声明循环模式偏移、libc 识别及模拟执行能力。增加能力必须同时具备离线依赖和可调用路径验收。

### 原生 Python 源码作业

`py.run` 使用固定 `-I -B -u -X utf8` 参数启动包内 CPython。源码必须是非空 UTF-8，最大 256 KiB 且不含 NUL，作为封存的不可执行输入存储。最多允许 64 个 argv 和一个可选的已接纳目标，超时为 10–3600 秒，输出为 1 KiB–8 MiB。沿用 16 进程/1 GiB Job 限制。源码不会在 System 服务中执行。Python 语法错误产生失败作业和原始 stderr；重试保留原始源码绑定的执行意图。

## 离线交付与验证

安装界面和无人值守部署都由原生安装器执行经审核的安装。安装器编排受保护状态、SSH 配置、服务控制管理器激活和公开连接回执发布。界面负责确认与回执导出。

在 x64 MSVC 环境构建。模型凭据和签名私钥只保留在控制器。安装/卸载验收只在专用 VM 或隔离 runner 上执行，不覆盖开发者安装。

CI 在没有控制器密钥的情况下暂存准确的载荷和原生二进制文件；操作员使用控制器密钥目录对可下载模板执行 finalize：

```powershell
python -m scripts.worker.toolchain --cache target/windows-offline-cache --download
python -m scripts.worker.build_native
python -m scripts.worker.media template --cache target/windows-offline-cache --native target/windows-worker-native --output target/windows-worker-media-template --version VERSION
python -m scripts.worker.media check --template target/windows-worker-media-template --report target/windows-worker-media-check.json
python -m scripts.worker.media pack --template target/windows-worker-media-template --version VERSION --output target/windows-worker-media
python -m scripts.worker.media finalize --template ABSOLUTE_TEMPLATE_DIRECTORY --keys ABSOLUTE_CONTROLLER_KEY_DIRECTORY --spec enrollment-spec.json --output ABSOLUTE_MEDIA_DIRECTORY --generate-ssh-key ABSOLUTE_SSH_PRIVATE_PATH
cargo fmt --manifest-path workers/windows/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path workers/windows/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path workers/windows/Cargo.toml --all-targets
```

`template` 暂存 `payload/`、`binaries/{wukong-worker.exe,wukong-package.exe}`、未签名的 `manifest-draft.json`、`native-build.json` 与 `media-template.json`；随后 `check` 在临时目录用临时密钥对该模板执行 finalize，并运行 `WukongWorkerSetup.exe verify-enrollment`，因此 CI 无需提权或安装即可验证密封、登记与介质装配。发布构建会发布确定性归档 `wukong-windows-worker-media-<version>.zip`；请按 `SHA256SUMS` 校验并在密钥目录之外解包。

`finalize` 要求使用位于模板和输出之外的绝对控制器密钥目录。登记规格（`schema: 1`）提供 32 位小写十六进制 `installation_id`、由 1–64 个 ASCII 字母/数字/下划线/连字符组成的 `environment_id`、`generation`（默认 1）、IPv4 `management_peer`（拒绝环回、未指定、组播和广播地址）、`ssh_port`、可选的 `execution_boundary`（仅允许 `disposable_vm`），以及规范的 `ssh_public_key {algorithm, key}` 或 `--generate-ssh-key` 二者之一。SSH 密钥由包内 `ssh-keygen.exe` 生成；私钥写到介质之外，只有公钥进入签名登记记录。finalize 会密封已暂存载荷、签署登记、装配公开介质（`WukongWorkerSetup.exe`、`setup.json`、`payload/`、`enrollment/`、可选 `recovery/`）并写入带哈希的 `media.json` 回执。若密钥目录或任何私钥文件名将出现在模板或输出内，命令会失败关闭。绝不要把控制器密钥目录、SSH 私钥或模型凭据复制进介质或客体。

任意 Windows 主机都可在不提权的情况下预检最终公开介质：`WukongWorkerSetup.exe verify-enrollment <payload> <enrollment> <controller-public-key-hex> <controller-version>`。安装使用提权的 `WukongWorkerSetup.exe install-reviewed setup.json <controller-key-hex> <sha256-of-enrollment.json>`，且只应在专用可丢弃 VM 上执行。

当 `install-reviewed` 发现有效的现有部署时，只允许通过经过审查的离线 SYSTEM 开机路径升级。为了认证该现有安装，此过渡路径可接受具有完整签名和哈希验证、运行协议低于或等于安装器当前协议的旧包；高于当前协议的未知包仍会被拒绝。新候选包以及每次正常服务／资源激活仍必须精确匹配当前协议。签名候选必须具有更高的语义包版本和登记代际，并保持安装、控制器信任方以及 SSH 对端/端口/客户端密钥绑定不变。安装器暂存并校验签名包，然后更新受保护的登记/绑定/部署记录、固定 SSH 中继、现有安装级防火墙规则和现有自有服务。父目录与被替换记录分别按各自显式 ACL 策略校验，绑定文件的较窄策略绝不会复用到代码根目录。防火墙校验同时接受裸登记地址和 Windows Firewall 返回的等价单主机子网掩码形式，程序、端口、方向、动作、配置文件和启用状态仍逐项精确检查。它不会接管外部服务/规则，也不会向模型驱动的执行暴露升级 RPC。准入后的变更一旦中断，本次尝试即失败：必须恢复旧 VM 基线，不能在原位置继续部分原生升级。

### 安装回执与重新保存

离线安装成功后，会在受保护的 `C:\ProgramData\WukongWorkers\<installation_id>\connection.json` 写入公开回执（schema 1，`receipt_version: 2`）。内容包括安装/环境身份、SSH 用户与端口、SSH 主机公钥及其指纹，以及 `candidate_addresses`：本客体非环回 IPv4 地址，RFC 1918 私有地址优先，各组内按数值升序，去重后最多 8 个。地址枚举是尽力而为的：即使无法读取网卡，安装也会以空列表完成。ProgramData 中的受保护记录仍是权威来源；导出的文件只是公开连接提示，不证明可达性或 hypervisor 隔离。

安装成功后，安装程序提供“保存安装结果 / Save install result”，并以默认文件名 `wukong-worker-<installation_id>.json` 打开 Windows 保存对话框。若对话框无法打开，或所选路径无法写入，同一字节内容会被复制到 `%USERPROFILE%\Desktop\wukong-worker-<installation_id>.json`（当用户配置不可用或没有桌面目录时改用 `%PUBLIC%` 或其桌面目录），并显示实际路径。保存不会改变安装成功状态：ProgramData 中的受保护记录此时已经完整。

在已存在有效安装的客体上以无参数方式再次启动安装程序不会重装；它会显示简短的“已安装”对话框，并询问是否保存刷新后的结果：重新枚举当前网卡地址，只改变 `candidate_addresses`，安装 ID、SSH 与主机密钥字段保持完全一致。客体地址变化后应使用此导出，而不是重置 worker。安装和修复仍必须使用离线介质及经过审查的 `install-*`/`verify-enrollment` 路径；刷新流程不会执行安装、服务或防火墙变更。

原生构建身份包含共用协议 crate、原生/Python 源码、锁定输入和构建脚本。组装时验证该身份并保留依赖许可证。登记绑定控制器签名者和安装身份；SSH 独立绑定 Worker 主机密钥。连接与任务绑定见[跨平台环境指南](../../docs/design/cross-platform-environments.zh-CN.md)，实际安装后的进程/调试器/恢复用例见[Worker 验收指南](../tests/README.zh-CN.md)。

单元测试验证契约和所有权不变量，不能证明 VM 隔离或真实模型效果。安装验收必须匹配准确的签名源码/包，并包含正常 Wukong 工具发现与分发路径。[2026-11-15 v3 验收记录](../../docs/history/validation/2026-11-15-execution-contract-v3.zh-CN.md)将新安装实例、x64/x86 执行与调试、重启恢复及完整 Wukong 流程绑定到准确源码/包和原始证据。较早的[历史记录](VALIDATION-HISTORY.zh-CN.md)保留各自的提交、失败和限制，包括未完成的维护及对抗性验收；正常路径验收不会取代这些限制。内核调试、自动创建 VM/快照和 GUI 目标交互仍不属于此 Worker 契约。

Windows Python 源码由 `python-components.json` 选择：独立[分析包](../analysis/README.zh-CN.md)与 `python/selftest.py`。辅助程序仅需 CPython 标准库；`python/requirements.txt` 与 Linux bridge 依赖分开。离线打包不安装或复制 Linux Worker 模块。签名源码身份包含分析包与组件清单。
