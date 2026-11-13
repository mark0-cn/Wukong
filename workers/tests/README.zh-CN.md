<a id="worker-tests"></a>

# Worker 测试

[English](README.md) · 简体中文

默认忽略的原生 `live_sdk_subscription_opens_and_unsubscribes_on_the_dedicated_vm` 夹具打开真实的按应用/监听端口过滤的 WFP 订阅，检查可信回环对照并排空 SDK 订阅。仅以管理员身份在专用 Windows VM 运行。它不改变防火墙规则或全局事件选项，也不证明容器内数据包已被阻断。用户 Windows 10 VM 已通过这个 SDK 夹具；实时阻断及并发冷启动验收仍需已安装 worker 的资格检查。见 [Windows 观察契约](../windows/README.zh-CN.md)。

对于已实现原生发现的工具包，可在进程、LaunchSpec 或中断进程验收配置中设置可选 `resource_inventory: true`。共享控制器夹具以每页两项查询两个 API，核对排他游标推进及 ID 一致，并在报告保留返回的资源摘要/状态。进程验收观察活动任务；LaunchSpec 验收要求全部六个已结算任务，拒绝非法范围/游标/作用域覆盖，并核对其他 run 的清单为空。其已完成恢复夹具在独立服务重启后比较完整原清单。故障夹具要求已对账的未知/关闭资源仍可被发现且不重新打开。历史工具包省略该选项。这些新增检查已通过本地编译/库检查，并在 `ddf86dc` 的用户离线 VM 通过已安装服务的进程、LaunchSpec、独立重启和服务中断验收。源码/内容身份及原始证据的边界见 [Windows 资格记录](../windows/README.zh-CN.md)。

`remote_installed_windows_worker_launch_acceptance` 使用 `WUKONG_REMOTE_WORKER_LAUNCH_TEST`：托管 SSH `config`、预期 `manifest_sha256`、全新的绝对 `output`，以及恰含 `x64`/`x86` 的 `probes`。每种架构提供 `program` 和 `dependency`，各含工具包 `runtime/selftest/` 目录中文件的绝对 `path` 与固定 `sha256`。可信探针加载同目录 `wukong-fixture.dll`，读取 cwd 外声明的 peer 文件，创建 cwd 文件，并将二进制 stdin/argv 输入复制到 stdout。控制器检查六个资源、改变清单/调用覆盖被拒绝、完整请求/回执身份、清理和已完成启动的稳定重试。VM 内不编译或下载。独立观察服务重启后，`remote_native_launch_recovery_acceptance` 使用 `WUKONG_REMOTE_WORKER_LAUNCH_RECOVERY_TEST`，包含 `config`、绝对 `previous`、其 `previous_sha256` 与全新的绝对 `output`；无需上传或执行，即可比较四个已完成 LaunchSpec 结果和原始回执。这些夹具在源码 `6e2bc91` 的用户离线 VM 通过；见绑定源码的[资格记录](../windows/README.zh-CN.md)。

```powershell
$env:WUKONG_REMOTE_WORKER_LAUNCH_TEST = (Resolve-Path .\remote-launch-profile.json)
cargo test --locked --lib remote_installed_windows_worker_launch_acceptance -- --ignored --nocapture
$env:WUKONG_REMOTE_WORKER_LAUNCH_RECOVERY_TEST = (Resolve-Path .\remote-launch-recovery-profile.json)
cargo test --locked --lib remote_native_launch_recovery_acceptance -- --ignored --nocapture
```

进程配置可选的 `argument_files: true` 需要带 `--read-argument` 的新版可信探针。它以无执行权限上传二进制数据，拒绝缺失准入的参数，将同一逻辑数据路径传入两次，核对原始输出字节/哈希，并要求原生回执恰好保留一个文件引用。容器内探针必须实际读到文件，且写入和 `FILE_EXECUTE` 打开均得到 `ERROR_ACCESS_DENIED`。该用例已在源码 `ecc18af` 的离线 VM 全新安装及服务重启后通过；报告身份和限制见 [Windows 资格记录](../windows/README.zh-CN.md)。

默认忽略的 Rust 夹具 `remote_installed_windows_worker_process_acceptance` 使用真实控制器，以及已独立固定指纹并安装好的 Windows VM。将 `WUKONG_REMOTE_WORKER_PROCESS_TEST` 指向 JSON 配置，包含托管 SSH `config`、预期 `manifest_sha256`、全新的绝对 `output` 路径和 `probes`：必须恰含 `x64`/`x86`，各自提供该包 `runtime/selftest/` 目录中可信自编译 `wukong-probe.exe` 的绝对 `path` 及 SHA256 `sha256`。夹具通过准入输入协议上传字节；VM 无需编译器或下载包。检查 echo、截止时间、取消、单资源/租约屏障、稳定启动重试、原始/分块输出哈希和陈旧隔离令牌。报告保留原始回执/输出字节，不含租约令牌或私钥。这仅为有限原生进程验收；生产能力声明仍为 false。

对于带新版可信探针的 schema-2 工具包，`cleanup_adversary: true` 会另行在两种架构运行只读文件/受保护空 DACL 清理用例；旧探针应省略该项。也可固定独立构建的可信探针进行测试，无需替换已安装的签名 worker，但必须记录两者源码身份。上传路径/request ID 包含探针哈希，保留先前不可变输入。清理探针先向实际用户及 AppContainer SID 授予自身新目录的完整权限，保留 `READ_CONTROL | WRITE_DAC` 句柄，再通过 SDK 设置并读回受保护空 DACL。ACL 准备失败不算限制访问清理通过。

默认忽略的 `remote_native_interrupted_process_acceptance` 使用 `WUKONG_REMOTE_WORKER_PROCESS_FAULT_TEST`：托管 SSH `config`、预期 `manifest_sha256`、固定哈希的可信 `probe`（`path`/`sha256`）、`fault`（`preparation`、`host` 或 `service`），以及全新的绝对 `ready`、`resume`、`output` 路径。准备用例在启动前发布 `ready`，便于独立操作方持有原生输入的独占句柄；宿主/服务用例在启动后发布，由操作方终止前核对真实归属进程身份和目标就绪。新版探针使用 `--fault-ready` 在本次私有 scratch 写出固定标记；操作方核对映像哈希、父进程和标记字节，不以粗粒度 CPU 计数或等待时间推断执行。服务用例还由操作方持续持有仅查询的守护 Job 句柄，迫使恢复通路重新打开并排空真实对象。独立观察操作/恢复后，才由操作方创建 `resume`。

故障配置可选的 `launch_spec: true` 通过同一探针及故障协议使用清单指定的 `work/nested` cwd。操作方须核对受保护启动绑定，并在该 cwd 内观察到标记后才注入故障。这个新增服务中断用例也在 `6e2bc91` 的同一 VM 通过，包括独立仅查询守护 Job 观察及嵌套 scratch 清理。

额外的 `fault: os_boot` 配置要求 `resource_inventory: true`。独立操作方先核对实际拥有的探针及受保护请求，记录已有原件，再仅重启专用 VM。独立检查冷启动资格及原始清理证据后，在 360 秒内创建 `resume`。该分支保留旧传输/绑定，重连时拒绝旧启动，要求只有系统启动身份变化且租约 epoch 增长，拒绝旧 run 的发现/资源查询及旧输入复用，再于新 run 准入并发现一次新上传的执行。旧启动的结果通过独立原始证据核对，不放宽原生 run 绑定来暴露它们。编译/库检查已通过；这个新增控制器启动边界分支尚未通过 VM 验收。

夹具自身不终止进程、不注册任务；检查绑定的未知结果清理、不可变恢复字节、不重放启动、陈旧服务隔离令牌，以及后续准入成功。协调文件仅为信号，不是操作系统证据。这三个具体故障用例于 2026-11-13 在用户提供的离线 VM 通过；源码/内容身份及限制见 [Windows 资格记录](../windows/README.zh-CN.md)。先前已完成结果恢复不覆盖这些用例。

```powershell
$env:WUKONG_REMOTE_WORKER_PROCESS_FAULT_TEST = (Resolve-Path .\remote-process-fault-profile.json)
cargo test --locked --lib remote_native_interrupted_process_acceptance -- --ignored --nocapture
```

```powershell
$env:WUKONG_REMOTE_WORKER_PROCESS_TEST = (Resolve-Path .\remote-process-profile.json)
cargo test --locked --lib remote_installed_windows_worker_process_acceptance -- --ignored --nocapture
```

这些任务关闭后，独立重启该专用测试服务并观察新的资格回执，保留旧回执和连接身份。随后 `remote_native_completed_process_recovery_acceptance` 使用 `WUKONG_REMOTE_WORKER_PROCESS_RECOVERY_TEST`，配置包括托管 SSH `config`、既有报告的绝对 `previous` 路径、其 `previous_sha256` 和全新的绝对 `output` 路径。夹具检查系统启动/安装/包身份不变及租约 epoch 递增，无需重新上传，即可按原始稳定请求取回两个已完成结果，并比较原始回执字节。夹具本身不重启服务、不安装或卸载。两个夹具均于 2026-11-13 在 worker 源码 `fbf7912` 上通过用户提供的离线 VM 验收；只读 SQLite 检查和独立复制的原件进一步确认六个资源已关闭、副作用已完成且 scratch 为空。中断资源清理/恢复、准备阶段取消及其他执行/生命周期通路仍未完成。

```powershell
$env:WUKONG_REMOTE_WORKER_PROCESS_RECOVERY_TEST = (Resolve-Path .\remote-process-recovery-profile.json)
cargo test --locked --lib remote_native_completed_process_recovery_acceptance -- --ignored --nocapture
```

- `unit/`：可移植的单元和可信本地夹具测试；平台依赖项显式跳过。
- `integration/`：需要真实隔离 worker、Linux、GDB、AFL++ 或 Ghidra 的验收入口。
- `support/`：共享协议客户端，与生产运行代码分开。

从仓库运行 `python workers/tests/run.py unit`，可用 `-p 'test_workbench*.py'` 选择子集。
Linux 完整单测需以普通用户运行：其中的可信本地夹具使用 `trusted_local`，生产桥接明确拒绝以 root 开启该模式。
入口统一设置测试导入路径；不要通过扫描运行目录来发现测试，也不要把导入 live 脚本当成已执行验收。

镜像分别复制运行代码到 `/opt/wukong/runtime`、测试到 `/opt/wukong/tests`。生产桥接入口不变。
容器完整单测从宿主机以普通用户启动：

```sh
docker exec --user 10001:10001 wukong-worker python3 /opt/wukong/tests/run.py unit
```

在已经配置好隔离与能力的 worker 内，以 root 运行隔离验收及需要切换目标 UID 的专项测试：

```sh
python3 /opt/wukong/tests/run.py unit -p 'test_workbench*.py'
python3 /opt/wukong/tests/run.py integration --state /state/acceptance --fuzz
python3 /opt/wukong/tests/run.py isolation /state/isolation-acceptance
python3 /opt/wukong/tests/run.py launch --state /state/launch-acceptance
python3 /opt/wukong/tests/run.py ghidra --state /state/ghidra-acceptance
```

测试状态目录应为本轮独占目录；真实后端要求见 [worker 说明](../runtime/README.zh-CN.md)。

`launch` 套件构建可信 C 夹具，经配置好的 Linux UID 隔离运行 stdin、argv-file 两种 LaunchSpec 任务，检查 cwd 外声明的 peer 文件、可写 cwd 输出、原始二进制 stdout、原文件不变及稳定启动。运行时须使用文档规定的 capabilities、只读容器根目录和可执行 `/tmp`。可移植规划/单元检查不能替代该隔离验收。

Windows 原生服务访问夹具对复制的管理员令牌同时禁用最大特权和管理员组，以检查普通账户的 DACL 权限，避免 `SeTakeOwnershipPrivilege` 授予 `WRITE_OWNER`。它不修改父进程令牌或已安装服务的策略。此夹具于 2026-11-13 在用户提供的 Windows 10 VM 上通过。

引导程序 PE 格式夹具在独立 Rust 库的 `pe_imports` 测试中直接调用 `object`，覆盖普通/延迟导入、两种架构、畸形目录及不安全库名。Python 自动化另行检查检查器响应绑定、CRT 拒绝、超时和执行前哈希验证；模拟观察数据不是 PE 格式验收。原生格式测试于 2026-11-13 在用户提供的离线 Windows 10 VM 上通过；真实控制端工具在本机重现了全部三个引导程序和两种编译探针的导入清单。

`integration --static-fixture` 使用静态链接夹具验证调试器和重放适配器，不验证动态加载器及库身份。Docker 运行需要可执行的 `/tmp`、ptrace 和文档要求的隔离能力。

Windows 上已有 MSVC 和 Debugging Tools 时，先运行 `cargo build --locked --bin wukong-dbgeng`，然后运行：

```powershell
python workers/tests/run.py windows-debugger --state target/dbgeng-acceptance --helper target/debug/wukong-dbgeng.exe --engine-dir '<Debugging Tools x64 directory>'
```

此入口编译可信 x64/x86 夹具，检查 Dump 寄存器、栈、内存、模块、线程、反汇编，以及实时断点、单步、执行至返回、请求去重、超时中断后的继续检查。哈希与原始记录保留在指定目录。它不安装工具，也不证明生产隔离或 VM 离线安装；这些需要独立 VM 验收。

对于没有编译器的独立 VM，只复制本机自行编译的 `fixture_x64/`、`fixture_x86/` 目录（位于输出的本轮目录中），再以 `--prebuilt-fixtures ABSOLUTE_DIRECTORY` 指定它们的父目录。VM 仍需完整的离线 Python/依赖及 Debugging Tools 载荷，无需在 VM 下载包。此入口还用正确 PE 身份及畸形 PE/MSF/minidump 输入检查真实的 `object`、DbgHelp 和 DbgEng 格式读取路径，拒绝冲突的 PE 时间戳、属于另一映像的有效 PDB、改名为 `.pdb` 的映像，以及在保留原匹配 PDB 缓存时篡改了已知夹具 RSDS GUID 的 dump；同时确认缓存缺失时不会加载生产者的外部 PDB。可移植单元测试另行覆盖对应关系策略及缺失身份的明确缺口，其中提供的观察数据不能替代原生 SDK 验收。DbgEng 初始化后拒绝文件索引请求，初始化失败则结束辅助程序。此套件于 2026-11-13 在用户提供的离线 Windows 10 VM 上通过；可信会话不证明 AppContainer 已加载 PDB、旧式完整用户态 dump 已通过、有效内核 dump 已被拒绝，或重启恢复已通过。

默认忽略的 Rust 夹具 `remote_windows_worker_reboot_binding_acceptance` 在保留控制器旧传输的同时，检查独立 Windows VM 的真实系统重启。先运行 [Windows 指南](../windows/README.zh-CN.md)中的安装后控制器验收，建立不可变的回读夹具并释放租约。将 `WUKONG_REMOTE_WORKER_REBOOT_TEST` 指向 JSON 配置，其中包括相同的固定指纹托管 SSH `config`、预期 `manifest_sha256`，以及全新的绝对 `ready`、`resume`、`output` 路径；这三个文件起初都不得存在。

```powershell
$env:WUKONG_REMOTE_WORKER_REBOOT_TEST = (Resolve-Path .\remote-reboot-profile.json)
cargo test --locked --lib remote_windows_worker_reboot_binding_acceptance -- --ignored --nocapture
```

测试在认证并保留租约后创建 `ready`，随后最多等待六分钟。操作者通过独立固定指纹的管理连接，仅重启专用 VM，确认服务自动就绪并生成新的 schema4 资格回执后，创建 `resume`。测试本身不执行重启、安装或卸载。它在写入前拒绝控制器旧绑定，检查系统启动身份变化及持久 epoch 增长，并以全新持租约连接验证已有回读夹具和写入哨兵不存在。`output` 仅包含身份/计数观察，不含租约令牌或密钥内容。用户提供的离线 Windows 10 VM 于 2026-11-13 在 worker 源码 `7b6c4c0` 上通过，包含管理 SSH/原生服务自动启动；原始回执和转录另行完成哈希验证。该结果覆盖固定可信资源及重启绑定，通用分析资源恢复与完整离线生命周期仍未完成。
