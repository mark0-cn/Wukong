# 跨平台分析环境

[English](cross-platform-environments.md) · 简体中文

## 环境与任务绑定

控制器管理环境注册表、模型凭据、任务数据库与证据库。每个登记环境声明平台和传输方式；按需建立连接，因此某个环境不可用不会阻止另一个环境执行。`environment_list` 报告配置和连接状态，不表示已经就绪。`environment_doctor` 检查 worker 的实际操作系统和工具；平台不匹配时拒绝连接。

分析任务保存环境配置快照。root 使用 `default_environment`；原生 `task` 接受 `environment_id`，省略时继承父任务绑定。绑定不可变。去重同时考虑环境身份、角色和范围。环境配置变更后拒绝该分析任务继续执行，不静默切换 worker。子任务只能探测绑定环境；root 可探测本次分析登记的环境。

例如，Linux Ghidra 专家和 Windows 调试专家可以共享同一原始 PE 制品，而不共享 worker 文件系统路径。角色权限仍为 root/analyst/verifier；选择环境不会授予额外权限，也不会启用尚不可用的后端。

每次子任务模型请求前，异步 worker 预检将角色注册表定义与实际观测到的能力及平台要求取交集。Rig 仅在探测期间挂起该请求；root 编排保持延迟连接，取消会释放尚未完成的探测。探测失败或超时后仍保留控制器工具，并在请求中明确给出诊断与不可用工具的要求。工具执行时再次校验后端，包括指定调试引擎和保留崩溃位置的输入最小化模式。缺失能力会在派发给 worker 前拒绝，即使调用由手工构造。Windows 工具列表排除仅支持 Linux 的工具。本地配置的 Ghidra 使用控制器；远程 Ghidra 需要通过启动器、桥接脚本和 Java 可用性探测。能力观测只表示可用性，不代表分析成功或已完成验收。

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

已安装的原生 Windows worker 使用 `runtime.managed_ssh`，包含 `installation_id`、`user`、`port`、`identity_file`、`known_hosts_file` 及 `host_key = { algorithm = "ssh-ed25519", key = "BASE64_PUBLIC_KEY" }`。实际主机公钥须经 VM 指纹独立核对后登记；仅扫描主机不能建立信任。管理用户名为 `wkgm` 加安装 ID 的前 16 个十六进制字符。私钥留在控制端。路径相对配置目录解析，解析后须为绝对路径，且不含相对别名或 SSH 展开标记。专用 known-hosts 文件必须精确包含 `wukong-worker-INSTALLATION_ID ssh-ed25519 BASE64_PUBLIC_KEY` 及结尾换行，并与配置公钥一致。

`runtime.ssh_host` 提供 IP 字面值或 DNS 主机名，`runtime.ssh_executable` 选择控制端 OpenSSH 客户端。托管传输忽略用户及系统 SSH 配置，固定 Ed25519 公钥，只使用指定私钥，并禁用 agent/X11/端口转发、代理命令、本地命令与连接共享。发送固定中继标记，不发送 Python/shell 启动命令。原生服务端独立强制使用登记的中继，协议握手还会核对安装 ID。通用 Linux SSH 保留桥接路径引用，同时显式启用严格主机校验并禁用转发。客户端参数校验及真实 OpenSSH `-G` 配置解析测试独立于 SSH 连通和 VM 安装验收；桌面连接导入会校验公开安装凭据和独立核对的指纹，再保存专用公开主机固定文件，不导入私钥内容，参见 [Windows 组件指南](../../workers/windows/README.zh-CN.md)。登记不进行连接检查；桌面介质准备尚未完成。

协议 2 握手确认安装 ID、操作系统启动身份以及 worker 代码/后端内容哈希。控制器在连接时固定该身份，并在每轮分析首次使用环境时以事务持久化。重连或恢复任务发现身份变化时，在执行前拒绝请求。环境配置可通过可选的 `installation_id` 进一步固定登记的安装实例。daemon 重启使用单独的诊断 ID，不代表操作系统重启，也不证明会话仍然存活。

Windows 将 OS loader 的 `KUSER_SHARED_DATA.BootId` 序号与动态链接的 `NtQuerySystemInformation(SystemBootEnvironmentInformation)` 返回的启动配置项 GUID 组合。仅靠启动配置项 GUID 可能跨重启保持不变；loader 序号用于区分各次启动，不依赖时钟推算、PowerShell/WMI 或完整进程可见性。查询只读取[只读用户共享映射](https://www.microsoft.com/en-us/msrc/blog/2022/04/randomizing-the-kuser_shared_data-structure-on-windows)的 712 字节前缀及 32 字节原生响应，校验长度、Windows 10+ x86/x64 布局及非零配置项身份；遇到不受支持的数据则拒绝建立身份。参见 Microsoft 的 [BootId 定义](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddk/ns-ntddk-kuser_shared_data)及[原生查询兼容性约定](https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntquerysysteminformation)。当前机器及跨进程检查不替代真实 VM 重启验收。

生产 Windows 环境要求 120 秒独占 run/controller 租约，并持久化递增 epoch。独立传输通道每 15 秒续租已使用的环境，与模型/调试器请求及控制器数据库心跳分开。worker 在读取持久回执前验证身份与隔离令牌；续租不改变业务请求指纹。仅超时不能证明进程已退出：执行中的请求阻止接管，仍有所属会话/job 时，另一轮分析不能获取租约。释放前须关闭和核对资源。显式可信夹具绕过生产租约，但不绕过身份检查。

Windows 原生服务统一拥有握手身份、能力声明、租约、文件与执行入口。受保护的 `metadata/lease-epochs/` 在发放令牌前持久化递增 epoch；单写入句柄及完整记录校验防止重用，上限为 100,000 个新 epoch，续租不增加记录。`worker_leases.rs` 只管理准入；`worker.rs` 分发请求，资源账本管理操作系统资源。没有常驻 Python 元数据 Worker、内层 Python 租约或反向健康通道。静态分析按需调用受隔离的 Python 辅助进程，仍使用同一资源账本。

资源请求通过 Tokio 异步互斥锁排队，避免锁竞争阻塞全部服务执行线程。同步服务轮询使用非阻塞加锁尝试，资源忙时报告未空闲。关闭时先取消请求任务，再异步等待资源锁。真实调度回归持有资源锁，轮询四个排队请求，并验证续租仍能完成、释放与接管仍被拒绝。单次同步准备/清理工作仍串行执行；托管调试交换在资源锁和输入锁之外等待，服务仍持有守护 Job 和账本票据。

原生资源账本复用 `rusqlite`，在事务中限制一个活动资源和每个资源的一个待定变更。`doctor.capacity.max_active_resources = 1` 明确声明该限制。容量不足在副作用准入前返回 `resource_busy`；控制器通过 Tokio FIFO 准入锁有界等待，期间允许查询、关闭、取消和续租。只有明确的准入前容量拒绝可自动重试，传输失败和未知结果不能重放。控制器业务回执与 Worker 资源账本各自保留。

原生资源发现使用受保护账本，而非 metadata worker 的任务目录。持租约调用的 `job.list` 数组及带类型的 `resource.list` 分页按 run/绑定隔离，最多 64 项并按资源 ID 排序；控制器变化后，原始归属/启动副作用/规格哈希仍可追溯。资源摘要省略规格和环境值。发现不能替代操作系统清理或直接的账本静止屏障。见 [Windows API 契约](../../workers/windows/README.zh-CN.md)。

## 共享制品与大文件

`artifact_stage` 接受同一 run 的证据 ID，在调用方环境中准备经过校验的不可变副本。跨环境传输经过控制器，并保留原证据身份。`stage_target` 同样流式传输通过哈希校验的原始文件，不将完整文件放入单条 RPC。

worker 提供 `file.upload.begin`、`file.upload.status`、`file.upload.write`、`file.upload.commit`、`file.stat` 和 `file.read`。上传上限 16 GiB，每块上限 1 MiB，现有 RPC 帧上限仍为 48 MiB。传输身份绑定目标路径、大小、SHA256 和可执行策略。状态查询独立于缓存的 begin 回执。重复块或部分落盘块必须与现有字节匹配；拒绝空隙和内容冲突。提交前验证大小和完整哈希，再原子地发布到尚不存在的路径，不覆盖已有目标内容。

大文件下载在流式导入证据前校验每个有界块与完整哈希。`file_get` 返回元数据和证据 ID，不内联文件字节；面向模型的输出保持有界。准备上传时检查实际剩余磁盘空间，但其他进程占用空间仍可能导致 IO 失败。重新连接不重放结果未知的调试修改。

## 统一进程启动

`job_start` 接受已有 schema-1 `launch_spec`，可另提供已暂存的 `input`。调用时省略 argv、环境和 cwd 覆盖；显式超时须与清单一致。进程任务支持 `stdin_pipe` 和带 `@@` 的 `argv_file`，PTY 输入属于交互会话。原始进程任务也可接收已暂存的 stdin。在将这些新输入发给 worker 前，分派必须观察到 `launch_spec_jobs` 或 `process_stdin` 支持。

Windows 从当前 owner 范围内已完成的原生输入回执解析清单及每个声明成员。新的 System 宿主在准备 persona 前，重读已准入清单并核对完整配置与受保护请求一致。成员保留逻辑布局及可执行/数据策略；单独受 guard 保护的可写 cwd 复用已有资源目录和清理机制。原生启动要求离线服务模式，拒绝 Linux loader/library 路径，最多允许 64 个声明成员及一个 argv 输入。清单和受保护请求各限 64 KiB；stdin 限 32 MiB，路径须符合清理深度限制。内部进程 schema 3 将完整请求哈希绑定到结果；旧版活动契约拒绝继续。查询缓存启动仍先于解析新输入。当前执行路由见 [Windows 组件指南](../../workers/windows/README.zh-CN.md)，历史包的验收仅证明其记录的范围。

Linux 复用已有 LaunchSpec 解析器和 UID 隔离，将包括 cwd 外文件在内的完整声明布局暂存为密封执行文件及单独可写工作树。题目准备将 cwd 内普通文件记入清单；符号链接被拒绝，暂存仍限 1024 文件/32 MiB。文档规定的容器 capabilities、只读根目录和可执行 `/tmp` 仍为必要条件。

## 结构化调试适配器

Linux 使用 GDB/MI；托管 Windows 使用原生 `dbgeng` 路由和现成 WinDbg MCP。`mode = "dump"` 要求独立暂存的转储，不允许启动参数，目标仍绑定原始映像。检查和控制保留有界类型接口、停止/期限规则和会话所有权，转储会话拒绝执行控制。原始 `debug_command` 仅适用于 GDB/MI。Windows 的同一方法注册表同时驱动路由和能力声明；每次服务启动必须完成签名检查、资源恢复、依赖检查和新的隔离/调试器资格检查后才开放执行能力。

Windows 使用固定版本的外部 WinDbg MCP 和 `rmcp` 客户端；自写 DbgEng 桥已删除。私有会话协议使用 Prost 生成的 protobuf，公共身份、租约、错误和容量类型由 `workers/protocol` 共享。保留原始 MCP 结果作为证据；启动与转储输入、会话所有权和清理由原生 worker 管理。当前边界见 [Windows 组件指南](../../workers/windows/README.zh-CN.md)，此前验收范围见 [历史记录](../../workers/windows/VALIDATION-HISTORY.zh-CN.md)。

## Windows 离线交付契约

完整离线包由联网控制器使用固定版本、经过校验的官方依赖组装，VM 安装不能在线补依赖。原生构建身份覆盖共用协议 crate；组装校验实际源码身份并保留许可证。安装、维护和实际执行各自需要对应验收，登记成功不能替代执行资格。私钥和模型凭据保留在控制器。详见 [Windows 组件指南](../../workers/windows/README.zh-CN.md)及其中明确标注的历史记录；旧包的验收不证明当前修改通过。

目标后端为 DbgEng，提供结构化检查与控制、有归属的会话和独立重放。VM 提供外层隔离，目标权限和元数据保护仍是独立控制。内核调试、创建 VM、自动快照管理和 GUI 交互不在首版范围内。声明 Windows 执行或离线安装支持前，必须通过真实 Windows VM 验收。

## 状态与验证边界

新资源记录要求状态 schema 4。拒绝旧工作区且不迁移；应创建新工作区并单独保留旧数据。单元/可信夹具检查覆盖任务绑定、按需连接、真实 bridge 文件传输、可恢复上传和证据身份，不代表真实 GDB/Ghidra/DbgEng 或 Windows VM 安装器验收。参见[实现契约](implementation-contract.zh-CN.md)和[worker 指南](../../workers/runtime/README.zh-CN.md)。
