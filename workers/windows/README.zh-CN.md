# 原生 Windows Worker 组件

[English](README.md) · 简体中文

## 职责与请求链路

原生 Rust Worker 是 Windows 上唯一的执行管理入口。托管 SSH 调用固定 relay，经命名管道认证已登记的 SCM 服务。服务负责启动、资格检查、恢复和关闭，`native/rpc/worker.rs` 分发公开 API。不再存在常驻 Python 元数据 Worker、内层 Python 租约、回环 Worker 端点或反向 broker-health 通道。

| 组件 | 职责 |
| --- | --- |
| 控制器 / Rig | Agent 调度、角色、持久化业务请求、证据与独立验证 |
| `workers/protocol` | 共用身份、租约、错误和容量类型；公开 JSON-RPC 语义 |
| `native/host/` | 安装、SCM 生命周期、已验证运行上下文及启动资格检查 |
| `native/rpc/` | JSON-RPC 注册、传输、启动身份绑定及控制器／运行隔离 |
| `native/resources/` | 单一资源管理器拥有准入、操作票据、账本及恢复 |
| `native/executors/` | 进程／会话计划及受控执行观察；不修改账本 |
| `native/artifacts/` | 租约范围内的不可变输入、运行／资源范围内的输出及受限保护文件 IO |
| `native/isolation/` | Windows Token、AppContainer、Job、ACL/WFP 及进程启动 |
| 外部 WinDbg MCP / Python | 在受控资源内执行调试或有界分析，不独立管理 Worker |

公开传输继续使用 JSON-RPC 2.0。控制器使用 `jsonrpsee`，共用 Rust 类型使用 Serde。内部会话消息采用 Prost 生成的 protobuf 和 Tokio 帧处理，外部调试器使用 `rmcp`。原始调试回复保留为带哈希的证据，不重新实现一套调试协议。Linux 保留现有 Python Worker。

`ping`、`doctor` 由原生层直接回答。只有签名包检查、遗留资源恢复、离线依赖检查和本次服务启动的新隔离/调试器资格检查全部完成，服务才开放已注册的执行能力。`doctor.readiness` 说明当前状态。历史资格回执不能替代新启动检查。注册表只声明可调用的原生执行或隔离分析方法；Windows 不声明 Linux GDB 命令、AFL/QEMU 和运行时准备能力。

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

在 x64 MSVC 环境构建。模型凭据和签名私钥只保留在控制器。安装/卸载验收只在专用 VM 或隔离 runner 上执行，不覆盖开发者安装。

```powershell
python -m scripts.worker.toolchain --cache target/windows-offline-cache --download
python -m scripts.worker.build_native
python -m scripts.worker.layout --cache target/windows-offline-cache --output target/windows-offline-layout
python -m scripts.worker.assemble --cache target/windows-offline-cache --output target/windows-worker-package --keys ABSOLUTE_CONTROLLER_KEY_DIRECTORY
cargo fmt --manifest-path workers/windows/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path workers/windows/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path workers/windows/Cargo.toml --all-targets
```

原生构建身份包含共用协议 crate、原生/Python 源码、锁定输入和构建脚本。组装时验证该身份并保留依赖许可证。登记绑定控制器签名者和安装身份；SSH 独立绑定 Worker 主机密钥。连接与任务绑定见[跨平台环境指南](../../docs/design/cross-platform-environments.zh-CN.md)，实际安装后的进程/调试器/恢复用例见[Worker 验收指南](../tests/README.zh-CN.md)。

单元测试验证契约和所有权不变量，不能证明 VM 隔离或真实模型效果。安装验收必须匹配准确的签名源码/包，并包含正常 Wukong 工具发现与分发路径。[2026-11-15 v3 验收记录](../../docs/history/validation/2026-11-15-execution-contract-v3.zh-CN.md)将新安装实例、x64/x86 执行与调试、重启恢复及完整 Wukong 流程绑定到准确源码/包和原始证据。较早的[历史记录](VALIDATION-HISTORY.zh-CN.md)保留各自的提交、失败和限制，包括未完成的维护及对抗性验收；正常路径验收不会取代这些限制。内核调试、自动创建 VM/快照和 GUI 目标交互仍不属于此 Worker 契约。

Windows Python 源码由 `python-components.json` 选择：独立[分析包](../analysis/README.zh-CN.md)与 `python/selftest.py`。辅助程序仅需 CPython 标准库；`python/requirements.txt` 与 Linux bridge 依赖分开。离线打包不安装或复制 Linux Worker 模块。签名源码身份包含分析包与组件清单。
