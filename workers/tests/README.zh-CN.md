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

### 托管原生 WinDbg 启动验收

`src/runtime/ssh_debugger_acceptance.rs` 通过真实控制器传输测试已安装服务。将 `WUKONG_REMOTE_WORKER_DEBUGGER_TEST` 设为本机 JSON 配置路径，包含固定指纹的托管 SSH `config`、`manifest_sha256`、新的绝对 `output`，以及以 `x64`/`x86` 为键的 `probes`。每种架构提供 `program` 和 `pdb`，各自包含本机绝对 `path` 及该签名包中的精确 `sha256`。运行 `cargo test --locked --lib remote_installed_windows_worker_debugger_acceptance -- --ignored --nocapture`。夹具仅上传这些密封字节及离线 LaunchSpec，不安装软件。每次调用后 fsync RPC JSONL，最终原始 MCP 日志与报告一同保留。

2026-11-13，该夹具在专用离线 Windows 10 VM、原生源码 `5f2b692`、清单 `a8a755a2cd17c597f0ecfa87848db668aeba7654d6cddf4b5c90b45146a427a7` 上通过。两种架构均覆盖 PDB 符号断点、数字 ID 启用/禁用/删除、寄存器、内存、模块、栈、原始线程结果、反汇编、step/next/finish、异步 continue/interrupt、不可变活动日志前缀、拒绝释放活动租约、缓存关闭及拒绝重启已关闭会话。57 次控制器调用产生 60 次 MCP 调用，包含两次预期的非故障异常查询拒绝，原始错误均保留。首轮夹具错误地要求普通断点具有异常信息；修正运行前，已关闭其目标并独立收集原始证据。只读 SQLite 及独立 VM 检查确认三个资源关闭、32 个副作用完成、scratch 为空且无剩余所属调试器/探针进程。95 份原件均已核对哈希，先前运行的服务及管理 SSH 保持运行，ISO 已卸载。报告 SHA256 为 `cc50a5b0c35e00a21916bf8f8cab50731c20b8d9d451769d149b25db76ff0a04`；x64/x86 原始日志哈希分别为 `1934765a4e5dde8ff5465c0269393703aa73c43e5a6a075c2a188eefc1b6d58f` 和 `d825ff0644e0e5a538ea99f4889755354614a21c8d8d1796463f653d0f1b8986`。这验证所述隔离启动/控制/关闭通路，不证明完整映像/PDB/dump 准入、服务中断恢复、全部对抗边界或生产可用性。

当前夹具还在分别准入的 LaunchSpec 中交换两种架构的 PDB 文件。每次被拒绝的启动必须保留恰好一个资源，完成所属资源恢复并拒绝重放，随后才进行有效用例。有效启动/清理必须保留精确的已准入 PE/PDB 哈希及匹配的引擎元数据，且不得声称验证了已加载 PDB 的字节。

2026-11-13，该扩展在 `4edc290` 的全新离线安装上通过，清单为 `f3a32f66d5fa3eb7330f4671826c84273ab01d80536f61ac7f1bd34cd284c062`。75 次控制器调用保留了 62 次 MCP 调用（两次为预期的非故障异常查询拒绝）。两份对调 PDB 均被拒绝；独立读取受保护原件确认错误明确为 GUID/链接器 age/架构不匹配，无目标准备意图、无 MCP 日志，随后 scratch 被移除。正确的 x64/x86 启动保留匹配的 PE/PDB 身份，并通过完整控制/关闭流程。只读 SQLite 确认四个资源关闭、28 个副作用已结算。88 份原件均已核对哈希；此前运行的 14 个服务及管理 SSH 保持运行，VM 无默认路由，ISO 已卸载。报告 SHA256 为 `b0e6d49949120cbf67d1006583f46c71f91e3bc22c9c5a2ebb194c9b127b8666`；x64/x86 日志哈希分别为 `4f5b2fc57c3e8d5535cfb6f11d546b6b71fa179b1a2eac54b1accc687b598e0d` 和 `2b89a5816d289b81561bbc6423b4da0259d6a7d86be9524d191cf49380a57b47`。这验证所述元数据/负例夹具，不证明已加载 PDB 字节身份、dump、服务中断恢复或通用对抗准入。该构建包含取消修复，但其中断场景仍未验收。

夹具现在还通过 `debug.inspect` 查询正常及恢复终态，检查恢复结果仍为未知，并通过 `native-results/<id>/receipt` 读取原始恢复回执。这些新增断言已于 2026-11-13 在原生源码 `590a1cc`、清单 `e5313593570240f15d7c0c17729ee293f05f02184e6f1a8d116159dace738e85` 上通过。完整 x64/x86 流程保留 79 次控制器调用、62 次 MCP 调用及两次预期的非故障异常查询拒绝；报告 SHA256 为 `e2071673aa6217206ce2005ba5c8d2d81f10caa53d768e12c9a571a2f33d8b26`。88 份基线原件均已独立核对哈希。

### 托管调试器中断验收

`ssh_debugger_fault_acceptance.rs` 复用密封探针上传、RPC 记录、协调文件及恢复后 echo 夹具。`WUKONG_REMOTE_WORKER_DEBUGGER_FAULT_TEST` 选择本机配置，包含固定指纹的 `config`、`manifest_sha256`、含 `program`/`pdb` 的单个 `probe`、`fault`（`cancel`、`host`、`service_stop` 或 `service_kill`），以及新的绝对 `ready`、`resume`、`output` 路径。运行 `cargo test --locked --lib remote_native_interrupted_debugger_acceptance -- --ignored --nocapture`。可信探针需构建为支持 `--debug-wait`：其 `fixture_debug_wait` 调用 `Sleep(120000)`。到达该符号并移除断点后，夹具使用上游有类型的单步跳过调用越过短序言并进入等待。每次已完成的准备操作均被保留，不解析指令文本。调用必须保持待定 600 ms，随后独立 VM 操作者还须验证原始受保护请求、所属进程和账本中仍为 started 的变更，才允许所选故障。取消模式在观察完成后发布恢复标记；其他模式在独立故障操作完成后发布。由于单条传输串行处理请求，`cancel` 使用携带相同租约的第二条已认证控制器连接。测试要求原始结果保持未知、确认清理、回执哈希匹配、拒绝启动/控制重放，以及新的 echo 执行成功。服务重启必须增加租约 epoch 并拒绝旧凭据。构建/Clippy 检查不证明这些真实故障验收。下述已安装结果仅适用于明确列出的源码及夹具。

2026-11-13，`a0119db` 控制器夹具和单独绑定哈希的可信等待探针验证了上述 `590a1cc` 安装。取消、终止精确会话宿主、正常停止并重启服务均通过：独立的仅查询 guardian 观察器在各次故障前记录七个活动进程、之后为零；原始结果保持未知，原始恢复回执已核对，拒绝重放，且新的 echo 执行成功。正常停止耗时 1,969 ms，SCM 退出码为零；重启使租约 epoch 从 9 增至 10，拒绝旧凭据并保留既有启动/连接字节。取消、宿主终止和服务停止报告的 SHA256 分别为 `6fc1a2cce777a938f1e0082689694802458522783cd3b1b187a7529c5839f9c3`、`40facc7354ee76444394427a1081f352bea8fa767779163a972ac7a97278a9f2` 和 `049095c5eddc2a0e8319377d34fc7e00b133e52f5c3945588b15953472c2f02d`。每个成功用例均已复制并核对六份观察器原件的哈希。

真实服务进程终止后的自动重启失败：清理子进程返回 Windows 拒绝访问，保留资源阻塞状态和两个 scratch 目录。20 份原始失败文件已保留。随后用包内 SDK CDB 执行同一受保护恢复请求成功，但未复现或解释最初的拒绝访问。辅助清理及其后成功启动**不证明**服务强制终止后的自动恢复通过。最终独立收集核对了 326 份原件，全部 17 个资源关闭、136 个副作用已结算，包含先前失败的夹具准备；三份启动资格检查及原始连接身份均保留，之前运行的 15 个服务及管理 SSH 保持可用，ISO 已卸载。当前清理错误加入失败阶段以及资源/恢复尝试/退出码，便于定位这个未解决故障。dump 及通用对抗覆盖仍待完成。

原生源码 `6c9f91f`、清单 `28018c23904355bbe3b578ffba61d20fa0001ec03e263b6896dafcce0205fd95` 的全新安装再次复现了服务强制终止失败，位置为目标 scratch 树删除，Windows 错误为 5。残留可信 EXE 已收回私有 ACL，PDB 尚未处理。19 份原始失败文件已核对哈希。随后 CDB 清理同一原始请求成功，服务恢复。之后一个独立的服务强制终止用例通过，恢复期间一直持有仅查询 guardian：七个活动进程降至零，epoch 从 2 增至 3，旧凭据及重放被拒绝，结果保持未知，新的 echo 执行成功。报告 SHA256 为 `f2a1904eb17b7ca8175f0fb13fecebffb266f26191b8577c5fbe1fc1988a1230`；六份观察器原件及最终 74 份启动/资源原件均已核对哈希。全部三个资源关闭、27 个副作用已结算，此前运行的 16 个服务保持运行，ISO 已卸载。首个失败用例仍是缺陷；后续通过不证明其根因已修复，也未确定谁保留了 EXE 的映像映射或句柄。

当前删除处理使用 BackON，最多重试八次（初始等待 20 ms、单次最多 200 ms、每项累计约 1.1 秒），仅适用于 Windows 错误 5/32，并受既有目录树期限约束。确认所属资源静止及收回目录后，它只重复相对于持有句柄的条目删除；此操作不重试 ACL 修复或执行。每次重试在原始清理日志中保留条目和错误，持续失败仍保留阻塞状态。本机测试通过 Windows SDK 映射可信测试程序的副本：删除先以错误 5 失败，映像 section 释放后成功；持续保持映射则验证有界失败及后续清理。这些测试复现删除条件，未证明原 VM 中未确定的映射/句柄持有者。该缓解措施的已安装验收记录如下。

2026-11-13，该缓解措施在 `4ed3ead` 的全新离线安装中通过，清单为 `b31611f11db6e863d993d204272abead011876bd53ef957bb63304360d6bb000`。普通服务强制终止用例通过，未触发删除重试（报告 SHA256 为 `f2641b0871505b1f91d1db15547c96f063f533f9bf5009356e5c64217530db31`）。第二个通过的用例使用 .NET `MemoryMappedFile`，在 worker Job 外持有精确核对哈希的可信探针只读映射。通过 SDK 支持的 ACL 观察确认清理已收回 EXE 后，再等待 100 ms 释放映射。原始恢复日志记录了三次错误 5 删除拒绝，分别等待约 20/40/80 ms，随后成功对账同一资源。该日志 SHA256 为 `bee0d58dfbb00b15562e30e1f8e0393fc298b6c6fae60b24cbfcf7b7f9901a38`；控制器报告 SHA256 为 `3d50663751c8156cc4d0a566614f0673bbb9ef805d984995e3104b886c4a6147`。

两个用例均保留原始未知结果、核对恢复回执绑定、拒绝旧租约与重放，并成功执行新的 echo 任务。独立的仅查询 guardian 观察记录七个活动进程降至零，映射用例的 epoch 从 4 增至 5。普通用例的六份观察器原件、映射用例的十二份观察器/映射原件，以及最终 103 份启动/资源原件均已复制并核对哈希。最终只读检查确认五个资源关闭、41 个副作用已结算，其中包含一次未注入故障的准备失败：PowerShell 把空映射名转换成了空字符串；修正用例前已对账其原始资源。三份启动资格检查、此前运行的 17 个服务及管理 SSH 均保留，未增加默认路由，ISO 已卸载。这验证了有界删除恢复和明确列出的服务强制终止场景，不证明早前失败中的外部映射/句柄持有者是谁、无限占用能够被清除，或 dump 与通用对抗准入已经通过。

### 外部 WinDbg MCP 验收

显式启用的 `runtime::ssh::dump_acceptance::remote_installed_windows_worker_dump_acceptance` 夹具通过 `WUKONG_REMOTE_WORKER_DUMP_TEST` 读取固定哈希的 x64/x86 探针 EXE/PDB/minidump 引用及已登记 VM 连接。它检查 dump 身份对应、架构不匹配后的恢复、内存/寄存器/栈检查、全部类型化控制操作均被拒绝且不占用请求 ID、不执行目标的清理回执，以及完整上游工具日志。生成可信夹具转储时使用明确的环境变量白名单；绝不传输继承了控制器凭据的完整内存快照。编译及本机 SDK/ACL 测试不能替代该已安装验收。

`tests/windbg_mcp.rs` 通过现有 `McpRegistry`（Rig/rmcp）驱动真实外部调试器。它默认忽略，要求专用 VM 已具有仓库的可信 x64/x86 `wukong-probe` 程序、匹配的 PDB 和已生成的 dump。配置固定身份的 SSH stdio 命令，或在该 VM 内使用本地命令；测试不会安装软件或运行未知目标。

将 `WUKONG_WINDBG_MCP_ACCEPTANCE` 指向包含以下字段的机器本地 JSON 文件：

- `server`：现有 `McpServerConfig` 的 `name`、`command`、`args`、`env` 和 `timeout_seconds` 字段。使用 `name: "windbg"`、已验证的 `windbg-mcp.exe`、`--tools session,inspect,exec,crash` 参数和 45 秒超时。使用打包的 Win32-OpenSSH 时，在环境变量白名单中包含 `"PROGRAMDATA"`。SSH 须固定主机密钥并禁用转发。
- `fixtures`：按 x64、x86 顺序提供两个对象，各自包含 `architecture`、`target`、`dump` 和 `symbols`。路径均为 VM 内绝对路径；`symbols` 为保存相应夹具 PDB 的本地目录。
- `transcript`：控制端全新的绝对路径，保存原始 MCP JSONL 证据；不覆盖已有输出。

```powershell
$env:WUKONG_WINDBG_MCP_ACCEPTANCE = (Resolve-Path .\windbg-mcp-acceptance.json).Path
cargo test --locked --test windbg_mcp existing_rig_client_drives_external_windbg_mcp -- --ignored --nocapture
```

此验收于 2026-11-13 在用户提供的离线 Windows 10 VM 上通过，使用 [glslang/windbg-mcp v0.24.0](https://github.com/glslang/windbg-mcp/releases/tag/v0.24.0)（压缩包 SHA256 为 `f6150397f7bc07a6d5fbd773adde3ec4b78639378abb5edec17be7201b1d3f34`）和已封存的 Debugging Tools 引擎。覆盖两种架构的 dump、实时启动、本地符号、`wmain` 断点、寄存器、16 字节指令内存、模块、调用栈、反汇编、单步、释放及最终空会话列表：共 51 次 MCP 调用，无工具错误。对 WOW64 额外加载器断点进行显式有界处理。线程列表文本作为原始证据保留，不宣称是结构化线程清单。这些结果证明客户端与后端互通，不证明托管 AppContainer 隔离、跨任务所有权、重启恢复或旧适配器已完整替换。

独立的 `external_windbg_mcp_run_handles_are_bounded_and_fenced` 夹具使用 `WUKONG_WINDBG_MCP_RUN_ACCEPTANCE`，配置结构相同，但须指定新的 transcript 路径；可以省略 `dump`。它仅启动已签封的循环探针，验证上游 `continue_async`、`wait_for_stop` 和 `break_in`：重复读取停止结果、不会取消运行的零时长轮询、旧运行句柄拒绝、按运行句柄重复中断，以及引擎执行期限。不引入替代执行调度器或控制台解析器。

```powershell
$env:WUKONG_WINDBG_MCP_RUN_ACCEPTANCE = (Resolve-Path .\windbg-mcp-runs.json).Path
cargo test --locked --test windbg_mcp external_windbg_mcp_run_handles_are_bounded_and_fenced -- --ignored --nocapture
```

2026-11-13，该异步夹具在专用离线 Windows 10 VM 上通过，使用已安装的 `10a6860` 包（清单 `fd9af35aa002ca8c9393f54f8bb03de872c1fd074c0e691cf7fe2b9edebb5636`），后端为固定版本 WinDbg MCP 0.24.0 及已记录的关闭 stdin 管道补丁。x64 和 x86 均覆盖上述断言。35 次调用的原始记录包含 33 次成功和两次预期的旧句柄拒绝，SHA256 为 `f7709ce5aaa9722a90c74e6127b2fed61a39392f052836dde2078e21f1196c2f`。两个会话均报告已释放、引擎 worker 已终止且未留下运行目标，最终会话列表为空。独立的前后检查核对了工具和两组 EXE/PDB，确认所属工具/目标进程无残留、worker 与管理服务仍运行、VM 没有默认路由。这是使用可信夹具的直接 Rig/rmcp 互通验证，不是托管原生会话准入或 AppContainer 验收；无需安装或重启服务。

引导程序 PE 格式夹具在独立 Rust 库的 `pe_imports` 测试中直接调用 `object`，覆盖普通/延迟导入、两种架构、畸形目录及不安全库名。Python 自动化另行检查检查器响应绑定、CRT 拒绝、超时和执行前哈希验证；模拟观察数据不是 PE 格式验收。原生格式测试于 2026-11-13 在用户提供的离线 Windows 10 VM 上通过；真实控制端工具在本机重现了全部三个引导程序和两种编译探针的导入清单。

`integration --static-fixture` 使用静态链接夹具验证调试器和重放适配器，不验证动态加载器及库身份。Docker 运行需要可执行的 `/tmp`、ptrace 和文档要求的隔离能力。

默认忽略的 Rust 夹具 `remote_windows_worker_reboot_binding_acceptance` 在保留控制器旧传输的同时，检查独立 Windows VM 的真实系统重启。先运行 [Windows 指南](../windows/README.zh-CN.md)中的安装后控制器验收，建立不可变的回读夹具并释放租约。将 `WUKONG_REMOTE_WORKER_REBOOT_TEST` 指向 JSON 配置，其中包括相同的固定指纹托管 SSH `config`、预期 `manifest_sha256`，以及全新的绝对 `ready`、`resume`、`output` 路径；这三个文件起初都不得存在。

```powershell
$env:WUKONG_REMOTE_WORKER_REBOOT_TEST = (Resolve-Path .\remote-reboot-profile.json)
cargo test --locked --lib remote_windows_worker_reboot_binding_acceptance -- --ignored --nocapture
```

测试在认证并保留租约后创建 `ready`，随后最多等待六分钟。操作者通过独立固定指纹的管理连接，仅重启专用 VM，确认服务自动就绪并生成新的 schema4 资格回执后，创建 `resume`。测试本身不执行重启、安装或卸载。它在写入前拒绝控制器旧绑定，检查系统启动身份变化及持久 epoch 增长，并以全新持租约连接验证已有回读夹具和写入哨兵不存在。`output` 仅包含身份/计数观察，不含租约令牌或密钥内容。用户提供的离线 Windows 10 VM 于 2026-11-13 在 worker 源码 `7b6c4c0` 上通过，包含管理 SSH/原生服务自动启动；原始回执和转录另行完成哈希验证。该结果覆盖固定可信资源及重启绑定，通用分析资源恢复与完整离线生命周期仍未完成。
