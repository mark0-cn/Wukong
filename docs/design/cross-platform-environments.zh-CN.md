# 跨平台分析环境

[English](cross-platform-environments.md) · 简体中文

## 环境与任务绑定

控制器管理环境注册表、模型凭据、任务数据库与证据库。每个登记环境声明平台和传输方式；按需建立连接，因此某个环境不可用不会阻止另一个环境执行。`environment_list` 报告配置和连接状态，不表示已经就绪。`environment_doctor` 检查 worker 的实际操作系统和工具；平台不匹配时拒绝连接。

分析任务保存环境配置快照。root 使用 `default_environment`；原生 `task` 接受 `environment_id`，省略时继承父任务绑定。绑定不可变。去重同时考虑环境身份、角色和范围。环境配置变更后拒绝该分析任务继续执行，不静默切换 worker。子任务只能探测绑定环境；root 可探测本次分析登记的环境。

例如，Linux Ghidra 专家和 Windows 调试专家可以共享同一原始 PE 制品，而不共享 worker 文件系统路径。角色权限仍为 root/analyst/verifier；选择环境不会授予额外权限，也不会启用尚不可用的后端。

配置不再提供顶层 `[runtime]`。传输配置位于 `environments.<id>.runtime`，由 `default_environment` 选择默认环境；空登记表允许仅使用控制器进行静态分析。拒绝旧单运行时配置，不进行转换。工作台的 Docker CLI 配置位于 `[workbench].docker_executable`；生成的工作台配置登记名为 `workbench` 的 Linux 环境，并保留其他已登记环境。桌面初始化与健康检查使用选中的环境，因此已配置的 Windows VM 不要求 Docker。`wukong doctor` 分别报告每个已登记环境。

```toml
default_environment = "linux"

[environments.linux]
name = "Linux analysis"
platform = "linux"

[environments.linux.runtime]
backend = "docker"
docker_container = "wukong-worker"
```

控制器 job/session 引用包含 worker 身份，并单独保存 worker 的内部资源 ID。文件登记键包含 run、worker 和路径。后台 job 观察选择 owner 绑定的环境，复用手工查询的证据与通知流程。

协议 2 握手确认安装 ID、操作系统启动身份以及 worker 代码/后端内容哈希。控制器在连接时固定该身份，并在每轮分析首次使用环境时以事务持久化。重连或恢复任务发现身份变化时，在执行前拒绝请求。环境配置可通过可选的 `installation_id` 进一步固定登记的安装实例。daemon 重启使用单独的诊断 ID，不代表操作系统重启，也不证明会话仍然存活。

生产 Windows 环境要求 120 秒独占 run/controller 租约，并持久化递增 epoch。独立传输通道每 15 秒续租已使用的环境，与模型/调试器请求及控制器数据库心跳分开。worker 在读取持久回执前验证身份与隔离令牌；续租不改变业务请求指纹。仅超时不能证明进程已退出：执行中的请求阻止接管，仍有所属会话/job 时，另一轮分析不能获取租约。释放前须关闭和核对资源。显式可信夹具绕过生产租约，但不绕过身份检查。

## 共享制品与大文件

`artifact_stage` 接受同一 run 的证据 ID，在调用方环境中准备经过校验的不可变副本。跨环境传输经过控制器，并保留原证据身份。`stage_target` 同样流式传输通过哈希校验的原始文件，不将完整文件放入单条 RPC。

worker 提供 `file.upload.begin`、`file.upload.status`、`file.upload.write`、`file.upload.commit`、`file.stat` 和 `file.read`。上传上限 16 GiB，每块上限 1 MiB，现有 RPC 帧上限仍为 48 MiB。传输身份绑定目标路径、大小、SHA256 和可执行策略。状态查询独立于缓存的 begin 回执。重复块或部分落盘块必须与现有字节匹配；拒绝空隙和内容冲突。提交前验证大小和完整哈希，再原子地发布到尚不存在的路径，不覆盖已有目标内容。

大文件下载在流式导入证据前校验每个有界块与完整哈希。`file_get` 返回元数据和证据 ID，不内联文件字节；面向模型的输出保持有界。准备上传时检查实际剩余磁盘空间，但其他进程占用空间仍可能导致 IO 失败。重新连接不重放结果未知的调试修改。

## 结构化调试适配器

`debug_start` 选择 `auto`、`gdb_mi` 或 `dbgeng`；`mode = "dump"` 必须提供单独暂存的 Dump，且禁止启动参数。暂存目标仍为本 run 的原始映像。`debug_inspect` 有界提供寄存器、栈、内存、模块、线程、反汇编、异常和执行状态；`debug_control` 提供类型化断点及 run/continue/interrupt/step/next/finish。执行操作等待停止；到达期限后请求中断，无法确认停止状态时使会话失效。Dump 会话拒绝执行控制，原始 `debug_command` 仅用于 GDB/MI。

Windows 原生辅助进程加载配置指定且通过哈希校验的 DbgEng 库，仅接受结构化操作。适配器记录目标、Dump、辅助程序及引擎哈希，保留有界 JSONL 原始记录。本地符号路径排除服务器及环境变量指定的路径。真实 x64/x86 可信夹具验收与生产隔离分开；托管 broker 完成前，Windows 执行必须显式使用可信夹具模式，不宣称可用于生产。

## Windows 离线交付契约

Windows 托管隔离 broker 和安装器属于后续实现工作；登记环境不证明这些能力已经可用。交付契约要求联网控制器从固定、经过校验的官方载荷组装完整离线包。VM 安装器必须包含所需运行时、依赖及 SSH/调试器安装载荷，不得联网补齐缺失组件。安装、修复、升级和回滚必须在 VM 无互联网连接时完成，同时保留 SSH 管理连接。符号由控制器准备并传入仅使用本地路径的符号缓存。

目标后端为 DbgEng，提供结构化检查与控制、有归属的会话和独立重放。VM 提供外层隔离，目标权限和元数据保护仍是独立控制。内核调试、创建 VM、自动快照管理和 GUI 交互不在首版范围内。声明 Windows 执行或离线安装支持前，必须通过真实 Windows VM 验收。

## 状态与验证边界

新资源记录要求状态 schema 4。拒绝旧工作区且不迁移；应创建新工作区并单独保留旧数据。单元/可信夹具检查覆盖任务绑定、按需连接、真实 bridge 文件传输、可恢复上传和证据身份，不代表真实 GDB/Ghidra/DbgEng 或 Windows VM 安装器验收。参见[实现契约](implementation-contract.zh-CN.md)和[worker 指南](../../workers/runtime/README.zh-CN.md)。
