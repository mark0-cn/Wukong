# Windows 原生 worker 组件

[English](README.md) · 简体中文

原生 `interact.profile` 通过现有生成式 protobuf 会话通道，观察宿主已持有的交互目标。它只接受 `session_id`，不会打开调用者指定的 PID，也不会访问模块路径指向的文件。现有 `windows-sys` 库提供已提交内存区域、内核报告的映像路径、创建 FILETIME，以及 DEP／ASLR／CFG 策略原始标志。查询始终持有进程句柄，保留待读 stdout，不创建变更 effect。观察失败返回不可用状态，不会终止会话。已关闭会话拒绝新观察。

观察在目标运行期间顺序进行，并非原子快照。枚举达到两秒、16,384 次区域查询或 4,096 个已提交区域后停止；最多保留 64 个映像路径，每个路径少于 4,096 个 UTF-16 单元。完整性标志明确报告限制和错误。现有不可变 transcript 保留完整观察；返回值预览 64 个区域和 16 个模块，路径限制为 256 个字符，并显式标注截断。模块 SHA256 保持 null：内核报告的路径不能证明映射内容未变，包括写时复制页面。准入时的启动内容身份单独返回。无法查询的策略表示未知。本机 OS 查询、大小边界测试及原生回归检查已通过；独立 x64／x86 远端 profile 验收入口已实现，安装环境资格验收尚待完成。生产执行仍禁用。

源码 `26b7040`、清单 `bff1ed43b5f6950076d952078acd7e7fc0608ee05daa8ad204662655920014fd` 已在专用离线 Windows 10 VM 通过已安装资格验收。八个 shell 用例覆盖 CMD cwd/环境、引号/重定向、非零退出、输出限额及 x64/x86 目标管道/超时；独立控制端连接在同步调用期间观察了活动资源并续租。独立观察服务重启后，全部八个原请求在不上传或执行的情况下返回不变的结果、凭据、二进制输出及资源清单。现有调试检查（包括拒绝错误 PDB）、八个普通进程用例及五个 Python 用例也通过。固定指纹 SFTP 核对了 224 份原件，并确认重启前 216 份原件全部未变；只读 SQLite 检查确认 25 个资源已关闭、49 个 effect 已完成。scratch 为空，24 个测试服务和管理 SSH 全部仍在运行，无默认路由，ISO 已卸载。两次启动资格检查保留了 40 次成功 WinDbg MCP 原始调用。shell/恢复报告 SHA256 分别为 `fc372878cb5b69eb743eb1a8988e68db4b34e360297fa32738b00abbb506db8d`、`d7263a3939e4f4ab54e2202915a9a4facbafca36ea1184f4824f6259fe2d5b14`。这些可信用例不代表 shell 中断、对抗性限制、剩余交互 profile 或离线维护已通过；生产执行仍禁用。

shell 适配器向 `subprocess` 显式传入三个现有标准流，并将已准入的本地盘符 cwd 从 Windows 扩展语法转换为普通盘符语法。缺少这些步骤时，隐藏子进程的输出可能丢失，CMD 会把 `\\?\` 当成 UNC 并退回 Windows 目录。本地可信回归测试覆盖普通及扩展路径；VM 中的受限诊断复现了两种故障，并确认修正后的 cwd/标准流、引号及清理行为。重新构建的签名包随后通过了上述具体已安装资格验收；失败前版及其诊断原件仍作为独立证据保留。

原生 `exec.run` 现在复用现有受限 Python/进程资源。固定且已封存的 Python 适配器使用标准库 `subprocess` 调用 Windows `System32/cmd.exe /d /s /c`，没有增加第二套进程监督器或命令解析器。命令文本、相对 cwd 和已批准的环境覆盖作为独立不可变 JSON 数据传递，用户文本不能成为解释器参数或适配器源码。可选 `target` 必须是已准入可执行文件，通过 `%TARGET%` 暴露；`%WUKONG_STATE_DIR%` 表示新建的所属 scratch 根，`%WUKONG_JOB_DIR%` 表示所选 cwd。环境覆盖复用现有 ASAN/UBSAN/AFL/QEMU 允许列表。使用 CMD 语法，不选择 PowerShell 或 Bash。API 准入 1–65,536 个不含 NUL 的字符，但仍受 CMD 自身命令行限制；失败直接返回，不改写命令。

现有 Job 实施 1–120 秒执行超时及 stdout/stderr 合计 1 KiB–8 MiB 输出预算（默认 30 秒、1 MiB）。同步回复等待进程/Job/persona/scratch 清理确认，包含 `resource_id`、退出码、宿主执行与清理耗时、4 KiB 输出尾部、保留字节数及完整哈希凭据/输出产物引用。非零退出为 `failed`，到期为 `timed_out`。中断结果保持 `lost`/未知，不伪造输出或自动重放。原请求重试先复用原资源，再决定是否准入新输入；同一 ID 更改命令/cwd/环境/目标/限额会被拒绝。等待使用 Tokio，在两次观察之间释放输入/资源锁，允许继续续租。本地测试覆盖无效输入、大型封存配置、原作用域重试及已完成结果观察；原生库测试和控制端/worker Clippy 已通过。上述具体已安装 VM 及已完成结果恢复检查已通过；生产执行仍禁用。

源码 `6a485bf`、清单 `812c3bc61c9f8fec8138ade0c952db8783f34d914900be8ea61e790c0c91a19d` 已在专用离线 Windows 10 VM 通过已安装资格验收。十个 x64/x86 交互用例覆盖二进制全双工交换、多次写入与空输入查询、自然退出、阻塞写入的未知结果恢复及离线 LaunchSpec。独立观察服务重启后，12 次已完成写入返回完全一致的原结果，2 次未知写入仍不可重放。现有调试器夹具（包括拒绝错误 PDB）及八个普通进程用例也通过。最终固定指纹 SFTP 核对了 233 份原件；只读 SQLite 检查确认 22 个资源已关闭、64 个 effect 已完成。scratch 为空，22 个测试服务和管理 SSH 全部仍在运行，无默认路由，ISO 已卸载。两次启动资格检查保留了 38 次成功 WinDbg MCP 原始调用。交互/恢复报告 SHA256 分别为 `97fb5ae38d36f0ecdc9727e0c755344085ca75f4838dd3b499ede63973820ec8`、`433ae67702fa4f1a40b384579791a09a330f4bcf327403a279719aa4f16531ac`。这些可信夹具不代表交互宿主/服务故障注入、对抗性限制、PTY/profile/shell 或离线维护已通过；生产执行仍禁用。

原生 `interact.start`、`interact.write` 和 `interact.close` 现在与调试会话共用现有的所属会话宿主、生成的 protobuf 通道、SQLite effect 和恢复路径。共同入口为 `resource-session`；已移除仅用于调试的运行时模块和重复的 transcript 写入器。交互目标复用已准入映像/LaunchSpec 准备、目标 persona、Job 和 scratch 清理。多次调用之间保持 `stdin` 打开；`stdout` 和 `stderr` 共用一条管道。写入期间及空闲时持续读取输出。空输入仅查询输出，不发送 EOF。每次写入最多 1 MiB，读取超时为 1–300 秒（默认 10 秒）；整个会话的输出总额上限为 1 MiB，证据总额上限为 8 MiB。直接启动的会话一小时后到期；LaunchSpec 决定自身的截止时间。返回输出为有界的 4 KiB 字节尾部，包含 UTF-8 文本、base64、字节数及截断状态；完整二进制输出保留在具有哈希的 transcript/`stdout` 产物中。

写入 effect 保留解码后输入的 SHA256、大小和超时。原请求重试返回原结果，包括会话关闭后；同一请求 ID 更改输入或超时会被拒绝。阻塞或中断的写入保持结果未知，要求清理所属资源，不会自动重放。自然退出只有在 Job 排空、persona/scratch 清理及宿主退出后才报告 `exited`；随后关闭只观察现有终态资源。原生交互输入由写入调用提供：初始已准入输入仅支持 LaunchSpec `argv_file`。LaunchSpec 决定 argv/环境；公共接口要求的 `argv` 占位参数会被忽略。PTY 和交互进程 profile 仍未实现。生产执行仍禁用。本地管道/日志测试及静态 x64/x86 夹具编译已通过。上述具体用例的已安装服务及已完成结果恢复资格验收已通过。

私有继承调试管道现在使用 [`blocking::Unblock`](https://docs.rs/blocking/1.7.0/blocking/struct.Unblock.html)，显式设置 8 KiB 缓冲，并通过 `tokio-util` 兼容适配器接入。帧仍由生成的 protobuf 和 `LengthDelimitedCodec` 处理，外部调试请求仍使用 rmcp。它替换了[文档限定用于普通文件](https://docs.rs/tokio/1.54.1/tokio/fs/)的 `tokio::fs::File` 包装，同时移除了不再直接使用的 Tokio 文件系统 feature。现有所有者在丢弃管道操作前排空 Job，写入刷新后才报告成功。本地真实管道检查覆盖二进制 protobuf 往返、读取超时后继续，以及阻塞写入在所属对端关闭后结束。上述已安装启动及持久调试会话检查进一步验收了此适配器。交互 profile 仍待完成；原生 shell 路径见上文。

同一 `2aaa155` 包随后通过独立注入的回放宿主及服务终止验收，故障发生在三次已准入验证试验的第一次。两次保留的只查询 guardian 观察器均记录到四个进程降为零。原请求保持 lost、结果未知及 `kind=verify`；原请求重试没有重跑，修改试验次数被拒绝，随后新的普通执行成功。宿主/服务报告 SHA256 分别为 `c454a037be9f06adb1eede009852e264e0fb973778c24bfc2191001f442663e3`、`9779660776852fbaa7aad90188dd22c90b7a10efd8d7f1d7003d04300cd06367`；服务租约 epoch 从 6 增至 7。最终固定指纹 SFTP 核对了 173 份启动/资源原件，另核对 12 份独立观察原件。全部 24 个资源已关闭、24 个 effect 已完成，scratch 为空；三次启动资格检查通过，21 个测试服务及管理 SSH 仍在运行，且无默认路由。ISO 保持卸载。上述用例覆盖首次试验中断，不覆盖后续试验之间或结束后的故障。

原生回放已在专用离线 Windows 10 VM 通过资格验收，源码为 `2aaa155`，清单为 `0f72752e1ee45116325a5be0e7784d74aed0da3b140455e41e87242a8bdaf6d7`。真实控制器检查了 12 个 x64/x86 回放资源、24 次试验：原生访问违例、阴性结果、二进制 stdin、超时、取消及离线 LaunchSpec 输入。独立观察服务重启后，全部原状态、汇总及逐次产物哈希保持一致，没有上传或执行。另有 8 个普通进程回归资源通过。固定指纹 SFTP 核对了 141 份原件；只读 SQLite 检查确认 20 个资源已关闭、20 个 effect 已完成，scratch 为空。两次启动资格检查保留了 36 次成功 WinDbg MCP 原始调用。全部 21 个测试服务及管理 SSH 仍在运行，VM 无默认路由，ISO 已卸载。回放、恢复及普通进程回归报告 SHA256 分别为 `443fd71dfc06baa88941b22e2c504b51b79ce024b051658cfcf6ceef0a1f868d`、`b74bfcfdf506ffd0b8ae1c829d806a211decd3436b4ef31923548e40547042a0`、`c5f0b891ff43612908f9bb2f7fcd33c962005b17f1bc5ee1081b0f8f2dd19a73`。此结果覆盖上述可信用例和已完成结果恢复；其他试验边界中断、其余执行路径、对抗性隔离及离线维护仍待完成。生产执行仍禁用。

原生 `crash.replay` 与 `poc.verify` 现在复用已准入的进程执行器、SQLite 资源账本及 guardian 清理。它们顺序执行 1–16 次试验，每次使用新的登录身份、AppContainer 和 scratch 目录。输入使用封存的参数文件或二进制 stdin，并支持现有离线 LaunchSpec。每次试验限时不超过 60 秒，输出上限为 8 MiB。验证要求生产者与验证者 ID 不同。预期退出码接受 Windows 32 位有符号或无符号值；可选 stdout/stderr 匹配各不超过 4 KiB，合计编码后不超过 8 KiB。未指定预期时，仅文档规定的原生异常退出码算作崩溃；崩溃本身不证明利用成功。

每次试验保留完整二进制输出及其哈希，以及受保护的请求/清理回执。汇总仅展示各流末尾 128 字节，并通过 `job.artifacts` 引用原件；所有请求的试验都匹配才返回 `passed=true`。取消会停止后续试验。相同启动请求返回原结果，同一请求 ID 修改参数会被拒绝。执行中断复用现有未知结果恢复，清理所有已准入的试验身份，不重新执行试验。没有另建回放进程监管器、传输或资源类型。此原生路径不支持网络回放和 POSIX 信号预期。上述指定回放及已完成结果恢复用例已通过安装服务资格验收；生产能力声明仍为 false。

2026-11-13，源码 `1556c9a`、清单 `5a53e8e9a90d7177f0c2db7811ee9a62196333132c2755cc53fc445ebb0752d7` 在专用 Windows 10 VM 通过全新离线安装、服务重启及一次断电式重启。三次启动均生成新的 schema-4 回执，取得实时 WFP 阻断证据，并通过隔离 x64/x86 WinDbg MCP 资格检查。原有控制器夹具通过真实跨启动恢复：中断进程保留为 lost、结果未知，guardian/profile/scratch 已回收，旧传输/run/资源/输入绑定被拒绝，租约 epoch 从 1 增至 3，重新准入的执行成功完成。此前 17 份启动原件字节保持一致。这在此前租约文件全零故障之后验证了 `atomicwrites` 元数据修复；一次断电式重启不能证明所有存储故障下的持久性。控制器报告 SHA256 为 `efd609d7fc3f730ce8e33b708d3fd9b679444489dc7aa9a5c44f2f5983759f75`。三次启动保留了 48 次成功 MCP 调用，会话均已释放。固定指纹 SFTP 核对了 37 份原件；只读 SQLite 检查确认两个资源均已关闭、两个 effect 均完成，scratch 为空。四个旧快照查询版本因缺少匹配 WFP 阻断证据而拒绝自动启动；已逐个恢复，公共连接身份保持不变。随后原先运行的 11 个测试服务及管理 SSH 均在运行，VM 无默认路由，ISO 已卸载。此结果不证明全部已安装版本均能自动启动。通用托管调试/重放/交互通路、对抗性隔离及离线维护仍未完成；生产执行仍禁用。

`3b3811e` 的全新离线安装在用户 VM 被拒绝：实时订阅错误地将接收/接受层的监听端口作为远端端口过滤。该层在 `localPort` 报告监听方，在 `remotePort` 报告探针，已有事件适配器已按此记录。订阅现使用 `FWPM_CONDITION_IP_LOCAL_PORT`。已通过固定指纹 SFTP 核对三份失败启动原件并卸载 ISO；失败包保持原样。修正后已安装服务的匹配已通过上文当前源码验收。这处确定的订阅错误与先前历史实例缺少快照证据的问题分开记录。

固定网络隔离资格检查现通过 [`FwpmNetEventSubscribe1`](https://learn.microsoft.com/en-us/windows/win32/api/fwpmu/nf-fwpmu-fwpmneteventsubscribe1) 在拥有的探针运行前订阅事件。SDK 过滤器限制应用及监听端口；已有解析器独立核对套接字元组、时间窗口、System 监听方 SID、方向及实际 AppContainerLoopback 阻断过滤器。最多保留 16 条相关记录，探针结束后等待通知最多两秒。溢出、回调格式错误、缺少证据或取消订阅失败均拒绝资格检查。回调复制有界且自行拥有的记录，不跨 SDK 边界展开异常，释放上下文前排空回调；取消订阅失败时将该有界上下文保留到进程退出。不改变防火墙规则或全局事件采集选项。成功 WFP 证据增加 `event_collection: "live_subscription"`；套接字权限拒绝证据保留已有契约。

此改动源于 2026-11-13 的专用 VM 断电式重启：`ddf86dc` 实例冷启动，并将一个中断原生资源对账为未知结果，保留 19 个已关闭资源、四次启动资格检查、空 scratch 及 141 份哈希核对的原件。四个历史实例在并发启动时因快照查询未找到匹配 WFP 事件而拒绝启动；已逐个恢复，公共连接身份未变。微软[日志契约](https://learn.microsoft.com/en-us/windows/win32/fwp/logging)描述约可容纳 100–150 个事件的共享循环日志。覆盖是可能解释，不是这些具体缺失事件的已证实原因。等候中的控制器夹具超时，因此本轮**未验证通过**其旧传输/启动围栏断言，也未验证全部实例自动启动。本地原生检查通过（113 项测试、四项明确跳过），用户 VM 也通过实际 SDK 订阅/取消订阅。该历史尝试未证明已安装服务的实时阻断事件、并发启动及重启行为；当前源码的结果与限制记录于上文。

原生 `job.list`、`resource.list` 现通过已有当前租约门禁读取 System 资源账本。两者接受 `limit`（默认/最大 64，最小 1）及可选的 `after`（规范小写 32 位十六进制资源 ID），按 ID 排序并使用排他的游标。范围覆盖被拒绝，仅返回当前 run 中匹配安装/包/系统启动绑定的记录。`job.list` 保留数组响应，包含进程状态；以最后任务 ID 继续查询，直到不足一页或空页。`resource.list` 跨原生资源类型返回 `{schema: 1, resources: [...], next_after: ID|null, production_execution: false}`。摘要包含 `resource_id`、`kind`、`phase`、原始 `native_owner`、`start_effect`、`specification_sha256`，不包含原始规格/环境。持续查询至 `next_after` 为 null；最后一页恰满时可能还需查询一次空页。发现复用直接查询的意图指纹/归属检查及状态发布，保留待清理状态，不以 Python 上游任务目录替代原生状态。它不证明操作系统存活，也不替代静止/完成屏障。两个 SQLite 查询索引以幂等方式创建，不改变账本 schema 1 或进程 schema 3。本地库/Clippy 检查及用户 VM 的三项无目标执行清单检查已通过。已安装服务的发现及重启验收在 `ddf86dc` 通过，记录如下。生产能力声明不变。

2026-11-13，源码 `ddf86dc`、清单 `5295e67dec5b127d0d2367e67fc67ed2244e1123551d604a7559024aa69e1a6a` 在用户提供的 Windows 10 VM 通过全新完全离线安装。真实控制器开启原生发现，检查十个基础进程资源及六个 x64/x86 LaunchSpec 资源：无租约查询被拒绝，活动任务可见，每页两项保留相同原始 ID/归属，非法查询被拒绝，其他 run 看不到记录。独立观察服务重启后，全部六个原启动状态/资源摘要及四份已完成结果回执无需执行即可保持。另一次服务终止保留仅查询的守护 Job 观察器：四个进程排空至零，中断资源仍可被发现为 lost/closed 并保留未知结果，启动未被重放。随后新的执行通过。只读 SQLite 检查确认 18 个资源关闭、18 个副作用完成，scratch 为空；三次启动资格检查均通过容器内 x64/x86 PDB 门槛。十个测试服务及独立管理 SSH 保持运行，VM 无默认路由，ISO 已卸载。固定指纹的 SFTP 独立核对 129 份启动/请求/结果原件及五份观察器原件。启动、已完成恢复及中断报告 SHA256 分别为 `4f3f40f0730a42e2361c40c45c3dfa130ec6f4bd091b2489a8c7fa966ff7f75d`, `481b6132f7d9d2486baf15e7d2b12df651f268a311d809de8d3528438aed3072` 和 `e277df1ae61d8261bf00481a70b75b7fc2b2ac5c49d239bf1a85e6f8e0cdaa90`。这验证所述进程/发现用例及同一次系统启动内的服务恢复；当前代码的系统重启、其余执行通路、对抗屏障及离线维护尚未完成。

2026-11-13，源码 `6e2bc91`、清单 `6275aa24103183cfc00d46b62362390c7dcd9d11fb0daa21eca1a4e62365f333` 在用户提供的 Windows 10 VM 通过全新离线安装。真实控制器通过十个基础进程资源及六个 x64/x86 二进制 stdin/LaunchSpec 资源。两种启动模式均加载已准入的同目录 DLL、读取 cwd 外声明的 peer 文件并创建 cwd 文件；改变成员哈希和调用时覆盖 argv 均被拒绝。独立观察服务重启后，全部四个已完成启动结果和原回执字节无需上传或执行即可返回。另一个服务终止用例核对受保护启动绑定及 `work/nested` 内实际标记；仅查询的观察者保留守护 Job，恢复将四个活动进程排空至零，清理嵌套 scratch/profile，并保留未知结果而不重放。之后准入的新执行成功。最终只读检查 SQLite 确认 18 个资源已关闭、18 个副作用已完成，scratch 为空；三次启动资格检查均通过容器内 x64/x86 PDB 检查，九个测试服务保持运行，VM 无默认路由，ISO 已卸载。经独立固定指纹的 SFTP 核对 129 份启动/请求/结果原件及五份观察者原件。启动、已完成恢复及中断启动报告的 SHA256 分别为 `68f617cd730b70204f8da75b32589a7a3f1cd752ee76c5762edd45ed64ddb721`、`1d01111f5dddc293c21f828948e4015efd259eb3aee13bae8a31480e498f4c58`、`bbdfb43a1f44504d96b8284ef8878b5a3cfa5d58b3b2b7cda6107c04151beb04`。这验证明确列出的可信进程启动及服务恢复；完整生产路由、对抗屏障和离线维护尚未完成。

独立 `job.start` 现接受已有 schema-1 `launch_spec`，并可另提供已准入 `input`。清单统一提供目标、完整文件布局、cwd、argv、环境和超时；调用时覆盖这些配置会被拒绝。System 宿主再次核对原始已准入清单，通过共用映像复制器暂存全部成员，并创建单独受 guard 保护的可写 cwd。`stdin_pipe` 传入二进制输入；`argv_file` 以密封输入路径替换 `@@`。原始 argv 任务也可接收 stdin。成员保留可执行/数据权限及持续持有的源/目标句柄。原生启动最多允许 64 个声明文件及一个 argv 输入，清单/请求限 64 KiB、stdin 限 32 MiB，路径受清理深度约束；Linux loader/library 路径、PTY 和网络服务模式被拒绝。内部进程 schema 3 将完整请求哈希绑定到结果，拒绝旧版活动契约。重试/恢复仍先查询已完成记录，再准入新输入。本地库检查（108 项通过、三项专用夹具明确跳过）、Clippy 及用户 VM 两项无目标执行启动检查已通过；这条启动通路的已安装服务验收在 `6e2bc91` 通过，详见上文。[架构指南](../../docs/design/cross-platform-environments.zh-CN.md#统一进程启动)与[夹具指南](../tests/README.zh-CN.md)说明共享控制器/Linux 通路及验收入口。生产能力声明仍为 false。

原生独立进程通路现通过已有准入输入读取器暂存直接的 `files/...` argv 参数。除程序外，最多允许 64 个不同参数文件；重复引用只复制一次，大小写别名、文件/目录重叠、缺失回执及其他归属范围均被拒绝。全部源句柄在 persona 创建前取得，密封后的目标文件句柄持续持有至 Job 排空。可执行上传保留读取/执行权限；数据上传仅获读取权限，不含 `FILE_EXECUTE`。原始引用绑定到受保护的原生回执，并在发布前核对。序列化请求在预留前必须满足现有 64 KiB 受保护记录限制。该实现共享程序暂存通路，未另建文件复制实现。本地库检查通过（105 项通过，三项专用夹具明确跳过），用户 VM 的两项无目标执行映像检查也通过。统一 LaunchSpec 支持见上文；其余执行通路尚未接入。

2026-11-13，源码 `ecc18af`、清单 `b905559f4f633635959931ed95dec34b8fa804ed78f68ea47ae784df733734ab` 在同一 Windows 10 VM 通过全新离线安装。真实 Rust 控制器通过全部十个 x64/x86 echo、截止时间、取消、限制访问清理和参数文件资源。各参数文件探针收到两次相同逻辑数据路径，读回精确二进制字节，实际观察到写入/`FILE_EXECUTE` 访问被拒绝，且只保留一个引用。缺失准入的输入在资源创建前被拒绝。独立观察服务重启后，在新租约 epoch 下无需上传或执行，即可取回两个已完成参数任务和原回执字节。初次/重启的容器内 PDB 资格检查通过，旧回执/连接字节不变，经独立固定指纹的 SFTP 核对 75 份原件。只读检查 SQLite 确认十个资源均已关闭、十个副作用均已完成，scratch 为空；八个测试服务保持运行，VM 无默认路由，ISO 已卸载。进程和恢复报告 SHA256 分别为 `661462e917c189e81c46d7f59dcf7e5ccd40d3ee5d7763bd86686c5c26279edb`、`d3b278b8a1678c46a38ed75f8df5c9b285a13f4abb75be3c8106788bd0949866`。生产能力声明仍为 false。

先前的操作方文件复制助手使用宿主机 OpenSSH 8.6 SCP 客户端，原有固定指纹和字节/哈希验证记录仍有效。下文这些历史记录描述为 OpenSSH 文件传输，不再声称使用 SFTP。`ecc18af` 验收对先前已合格签名工具包中的客户端二进制核对哈希，并由 OpenSSH 10.4 跟踪确认实际使用 SFTP 子系统。未将私钥复制进 VM 载荷。

用户提供的 Windows 10 build 17134 VM 暴露了较新开发机未复现的只读文件清理失败。清理现只请求读取及安全/属性元数据权限，避免通用数据写入权限，并通过所持 `Dir` 打开/删除子项。修复前，无目标执行的只读回归在该 VM 失败；修复后与外部硬链接/junction 哨兵检查一并通过。另一个独立固定哈希的 System 测试助手也清理了 native32 遗留的精确签名只读夹具；未更改已安装的签名载荷，其空 DACL 准备并未通过验证。这种显式夹具协助不等于生产恢复验收。

2026-11-13，源码 `69a5e3b`、清单 `2c905d0c56818cf8490c064c86ad57c520faac279e1ff6310cbd5ac063b29444` 在同一专用 VM 全新离线安装通过。真实 Rust 控制器在无需协助的情况下，通过 x64/x86 echo、截止时间、取消及只读文件/受保护空 DACL 清理。修正后的可信探针独立构建，经原生准入上传；未修改已安装的签名 worker。探针源码 SHA256 为 `5ad9ab02cf961db2ab2461c8c01147776110c0d3a2809abbeeb5b6ab43b402b4`，进程报告 SHA256 为 `137187d007dd75e909abd26c4c1a32fccd477e94baa699f09f67be36740f669d`。旧探针 ACL 准备失败保留为失败，不算限制访问清理证据。

独立持有输入独占句柄造成的准备中断、精确终止所持 System 资源宿主、以及精确终止/重启所持 SCM 服务进程也均通过。宿主/服务注入前核对已准入映像、父进程，以及可信运行探针在本次唯一 scratch 写出的标记。带标记探针的源码 SHA256 为 `8d788c57da3d4c463904a19c361b425cc97ff52c05bfb14ced4d743af839e0c1`。服务用例中，独立观察者仅持 Job 查询/读控制权限：恢复重新打开所保留守护 Job，排空其中四个活动进程，清理 profile/scratch，保持原资格回执和连接字节，并重新通过容器内 x64/x86 PDB 资格检查。控制器观察到租约 epoch 增加，拒绝陈旧隔离令牌，保留未知结果/原始恢复字节，不重放启动，之后成功执行新的准入任务。准备、宿主、服务报告 SHA256 分别为 `e93e8b9ec758e759d76727c6aef72ecca8ba2d81b223f5a109ecbd96528bf0bc`、`b2666f2d4bd410228590cdb4694b4e89a2aaf2d51e2c251bac22ef1f0eb1b7d4`、`a558cf205e8d668a438f5e884fcc547e2656aab71a8a8817710ab1d10af4412e`。先前 CPU 计数就绪尝试未注入故障，不算恢复验收。最终只读检查 SQLite，确认全部 27 个资源已关闭、27 个副作用已完成，包括保留的失败尝试；scratch 为空。经独立固定指纹的 OpenSSH 文件传输按大小/SHA256 核对 179 份启动/请求/结果原件。VM 无默认路由，未使用托管 CI，先前服务保持运行。这仅验证有限可信进程恢复，不代表完整对抗隔离或离线维护验收。

内部原生进程 schema 2 增加由 System 持有、名称不可预测的 `Global\WukongResource-INSTALLATION-RESOURCE` 守护 Job。创建时拒绝已有名称；重新打开仅授予查询/终止权限，并核对实际 System owner 及精确的 System/管理员 DACL。新的执行宿主必须在 persona/输入准备前证明自身位于该 Job。中断的 schema-2 账本项会启动独立可信 `resource-recover` 子进程，排空精确守护 Job（或观察其已销毁），核对受保护的 profile/scratch 意图，再在原角色上下文调用 `DeleteAppContainerProfile`。不执行原目标，也不凭 PID 接管。启动时先取得 metadata worker 的系统启动绑定，在离线/容器内资格检查及管理 SSH 就绪前完成原生资源恢复。恢复期间继续阻止资源准入及租约释放。旧版活动资源契约或归属不匹配时拒绝继续。

归属 scratch 清理复用 `cap-std`/`cap-fs-ext` 相对句柄操作，目录句柄不共享删除权限。SDK 只修复所持、无重解析的对象；存在其他硬链接的普通文件绝不修改 ACL 或只读属性。重解析条目仅解除链接，不修复其目标。受限作用域的 System 备份/恢复特权用于处理目标创建的限制访问后代；ACL 变更不含可继承 ACE，不向目录树传播。清理限制为 100,000 个条目、128 层、30 秒；失败继续保留资源阻断项。确认资源静止后，BackON 仅针对 Windows 错误 5/32 在该期限内重试条目删除，最多八次，原始错误继续记录。真实映像映射测试覆盖释放与持续占用。已安装的普通服务强制终止及人为映射占用用例已在 `4ed3ead` 通过，参见[中断记录](../tests/README.zh-CN.md#托管调试器中断验收)。原 VM 失败中的外部映射/句柄持有者仍未确定。输入/资源发布共享同一套仅新建、写穿透重命名实现。确认恢复后，已存在且有效的原生结果会被导入，不替换产物；缺失结果记录为 `lost`、`outcome = unknown`，另附哈希绑定的恢复回执，不编造退出码或空输出产物。在 persona 创建前及映像暂存后分别检查取消。可信探针另含只读文件/受保护空 DACL 夹具。上文已安装验收覆盖明确列出的三类可信进程中断；下文既有 `fbf7912` 结果仅覆盖其旧版已完成资源行为。完整生产能力声明继续为 false。

暂存目标映像、依赖和数据现仅向所属资源的登录 SID 与 AppContainer SID 授予读取权限，保留 System/管理员控制；不再沿用公共工具包向 Users/All Applications 授予的读取权限。可执行成员允许读/执行，数据成员仅允许读。仍复用现有暂存流程及禁止写入/删除共享的持有句柄。真实 NTFS/Authz 回归检查两个所属身份、其他资源身份、宽泛群组、写入/删除/ACL 修改拒绝及数据执行拒绝。这些检查已在本机及专用 Windows 10 VM 通过。VM 管理员测试进程需要使用禁用最大特权的令牌副本，否则所有权特权会绕过一项 DACL 断言；父令牌及产品权限保持不变。描述符检查不证明跨资源对抗性验收已通过。

2026-11-13，专用离线 Windows 10 VM 已在 `10a6860`（清单 `fd9af35aa002ca8c9393f54f8bb03de872c1fd074c0e691cf7fe2b9edebb5636`）通过全新安装及服务重启。真实控制器验收通过八个 x64/x86 进程用例（回显、超时、取消、参数文件访问）和六个原始 stdin/LaunchSpec 用例，涵盖 DLL 加载及两种输入模式。参数文件写入和执行均被拒绝。重启后的只读日志检查确认 14 个资源全部关闭、14 个 effect 全部完成，scratch 为空。两次启动均通过隔离的 x64/x86 MCP 资格检查：38 次调用成功，四个会话已释放且引擎 worker 已终止。103 份启动/资源原始文件经固定主机密钥的 SFTP 复制并核对哈希。资格回执 SHA256 为 `faf8d1f53e37212bb3a05c2d6415eaca135413a4022eee1e3147cc37c7f6ca89` 和 `95c15ed7cf750cdcd48e05fcaab960458ae54b907420fb95dad6c2ca0e4fe7b4`。公开连接身份及此前运行的服务保持不变，管理 SSH 可用，VM 没有默认路由，测试 ISO 已卸载。这验证了指定暂存输入执行路径；通用调试准入及更广泛的对抗性验收仍未完成。

`native/resource_runtime.rs` 将独立进程的 `job.start/status/cancel/artifacts` 及原生输出的 `file.get/stat/read` 接入已有控制器 SSH 隔离令牌。启动要求稳定且匹配的 request/call ID、当前归属范围内已完成上传的可执行文件、`files/...` 程序，原始 argv 使用私有 cwd `.`，或采用上文已准入 LaunchSpec。有界 argv、环境、期限及输出选项使用 16 个进程、1 GiB 的 Job。先查询原生账本，再解析新输入，因此控制器变化后，已结算或中断的启动也不会再次执行。服务以可跨线程的归属内核句柄持有新的 System 宿主；线程相关的 desktop 留在该子进程。只有宿主 Job 成功排空且绑定的清理完成后，才暴露受保护的结果、二进制输出和原始回执。轮询在租约过期时请求取消；服务停止时请求取消，并最多等待 15 秒清理。宿主/准备错误继续保留活动账本阻断项，并报告待清理。metadata broker 仍仅提供健康检查，完整生产能力声明继续为 false。更广泛的恢复/修复、debugger/replay/interactive/shell/Python 路由及完整生命周期验收仍未完成。所提供 Windows 10 VM 上 98 项惰性库检查通过，两项专用 fixture 明确跳过。

2026-11-13，该离线 VM 在 worker 源码 `fbf7912`、清单 `2fea6e1ef8247b45099b92860366af63e0ad57ccbc10c336585abe121af03767` 上通过全新安装及服务重启资格检查。真实 Rust 控制器通过 x64/x86 可执行文件上传、AppContainer echo 执行、超时/取消清理、单资源及租约释放屏障、续租、参数变更拒绝、已完成启动重试及陈旧隔离令牌拒绝。完整 stdout/stderr/回执字节及分块哈希与原生产物匹配。独立观察服务重启后，控制器无需重新上传或启动目标，即可取回两个已完成结果及原始回执字节。另行只读检查 SQLite，确认六个资源均已关闭、六个副作用均已完成，scratch 为空。经独立固定指纹 OpenSSH 文件传输按大小/SHA256 核对 13 份启动原件及 24 份资源原件。进程及已完成结果恢复报告的 SHA256 分别为 `c8224137c47bc83452b7dcde90ad58cfb77f45fe52bc2eaf68dbe2ae596cbd04`、`a9f38c18c3b699e543eebc6bcbf457df8852a1e66781555e1a0d6e912f6479b7`。重启后的普通控制器/分块输入验收也通过，租约 epoch 达到 6。VM 仍无默认路由，ISO 已卸载，先前测试服务保持运行。这验证有限的独立进程通路及已完成结果恢复，不代表中断资源恢复或完整生产隔离/生命周期通过。

`native/resource_process.rs` 现已实现内部 `resource-execute` System 宿主，与启动资格检查共享签名部署验证、固定环境准备及 profile 创建。请求必须位于精确受保护的 `metadata/resource-requests/RESOURCE.json` 槽位，绑定当前安装/包、控制器归属及已准入的可执行输入引用。输入在 persona 创建前验证；映像从持续持有的原生输入读取器复制，保留逻辑 `files/...` 布局，并获得受保护的只读/执行策略和禁止写入/删除共享的句柄。profile 意图在创建前记录资源 ID。宿主通过已有的挂起创建、Job 归属 AppContainer 通路启动零能力目标，成功排空后关闭 persona 并删除已接管的 scratch。清理错误仍作为错误；目标创建的限制访问后代不会被误报为已删除。独立进程的隔离令牌/账本调度，以及明确列出的可信准备/宿主/服务恢复用例已在上文通过验收；其他执行 RPC 通路仍待完成。普通管理/metadata 令牌无法进入此仅 System 可用的入口。既有有限已安装 VM 执行通路在 `fbf7912` 通过；当前中断进程证据记录在上文。

隔离 x64/x86 托管调试启动/控制/关闭通路已在 `5f2b692` 通过真实控制器验收，包含不可变活动日志读取及确认清理。参见[绑定源码的验收记录](../tests/README.zh-CN.md#托管原生-windbg-启动验收)。取消、终止会话宿主和正常停止/重启服务也已在 `590a1cc` 通过；早前服务进程强制终止后的自动恢复失败已通过有界删除重试缓解，并在 `4ed3ead` 的普通及人为映射占用用例中通过验收。参见[中断记录](../tests/README.zh-CN.md#托管调试器中断验收)。已安装 minidump 准入记录见下文；dump 中断/重启恢复及更广泛对抗验收仍待完成。

内部托管调试通路由 System 服务处理 `debug.start`、`debug.inspect`、`debug.control` 和 `debug.close`，接受 `mode = "launch"` 或 `"dump"`，引擎为 `engine = "auto"` 或 `"dbgeng"`。launch 与进程任务共享原生输入/LaunchSpec 准入及目标准备，并配对独立的目标/调试器 AppContainer。直接目标的资源期限默认为 3,600 秒，LaunchSpec 使用自己的超时。附加门控仅释放自己持有的一次挂起计数。

dump 模式调用固定版本外部工具的 `open_dump`。`minidump` 库接受最多 256 MiB、128 个 stream、2,000 个模块的 Windows x86/x64 minidump。所选模块的名称、架构、时间戳、映像大小、校验和及 RSDS GUID/age 必须与已准入的 PE/PDB 对应；外部引擎随后还须报告匹配的模块元数据及 dump 加载地址。这些检查证明元数据对应，不证明内存映像字节完全相同。dump 模式不创建目标 persona/进程；所有映像/PDB/dump 成员暂存时均不授予执行权限，只使用已准入的本地符号/映像目录。在预留副作用前拒绝逐次调用的执行参数、stdin 及全部 `debug.control` 操作，会话宿主也限制为检查和关闭。LaunchSpec 可提供映像文件集及期限，但不会执行其中的执行配置。清理回执明确记录 `target_executed = false` 并保留 dump 哈希，不编造目标输出产物。内核 dump、TTD 和旧版完整用户转储格式不在该准入范围内。原生调试请求 schema 2 绑定模式及 dump 引用。本机真实 SDK minidump 和 NTFS/Authz 测试通过。已安装 x64/x86 minidump 准入及 live 回归已在 `ea248d1` 通过，参见[绑定源码的验收记录](../tests/README.zh-CN.md#托管调试器中断验收)。dump 中断/重启恢复仍待完成。其余执行、恢复和对抗覆盖完成前，完整生产可用性仍禁用。

创建调试目标之前，由 `object` 解析已准入的 x86/x64 PE（最多 64 MiB）。已准入文件中，与映像同目录且名称匹配 CodeView 文件名的文件作为可选本地 PDB；绝不跟随内嵌的编译机目录。`pdb` 读取其 MSF 7.0 信息/DBI 流，每次视图最多 8 MiB，累计读取最多 32 MiB，并校验 GUID、架构及链接器 age。信息流的 age 可以增加，但不得早于映像 age；缺少 DBI age 时退回该信息流 age 规则。选中的 PDB 错误或损坏时，在创建目标前拒绝；缺少本地符号时明确记录。逻辑目录不得含有符号路径分隔符。附加后，固定的上游符号重载及结构化模块工具必须给出无歧义的完整清单，其中目标映像的文件名、时间戳、映射大小及校验和一致。若已选择 PDB，则引擎必须返回匹配且非 unmatched 的 PDB 元数据。启动与清理证据保留已准入文件引用/哈希，原始 MCP 记录保留观察到的模块。这证明元数据对应关系，不证明引擎所打开 PDB 的字节身份（`loaded_pdb_bytes_verified = false`）。本机测试使用实际 MSVC 链接的测试 PE/PDB，覆盖身份变化、损坏/有界读取、读取句柄回到起点，以及截断/歧义清单。已安装 x64/x86 及 PDB 对调验收已在 `4edc290` 通过，参见[绑定源码的记录](../tests/README.zh-CN.md#托管原生-windbg-启动验收)。这覆盖所述可信元数据夹具，不证明通用对抗准入。

账本确认关闭后，`debug.inspect` 的 `kind = "state"` 从原始受保护终态证据读取结果，不调用 MCP，也不创建宿主。正常关闭报告 `closed`；恢复报告 `lost`、`outcome = "unknown"` 及已确认的清理。恢复还会在返回回执选择器前核对回执哈希、资源及原始归属。同一 run 中已准入的后继控制器可读取旧归属的终态记录，不会获得新调试会话。`native-results/<id>/receipt` 返回确认关闭证明或原始恢复回执。关闭后仍不可进行其他检查/控制操作。本机测试覆盖缺失、变更及跨资源/归属回执，并保留未知结果。控制器夹具检查两种终态并读取恢复回执；这些断言已在 `590a1cc` 通过，见绑定源码的验收记录。

继承管道通道使用从 `proto/session.proto` 生成的 `prost` 消息及 Tokio `LengthDelimitedCodec`，帧上限 4 MiB，每个会话最多 4,096 次交换，每次期限 75 秒。回复绑定资源、准入请求哈希及递增序号。交换在资源锁/输入锁之外等待；服务保留守护 Job 和持久化票据。交换期间请求关闭会发出取消并报告清理待定。取消后最多给予宿主五秒，随后进行所属资源恢复；重复取消不会延长期限。服务停止或租约过期时，即使持有通道的 RPC 任务已被中止，仍可核对已退出的宿主。成功关闭要求 Job 已清空、profile/scratch 已删除；恢复使用两组原始 persona 意图，绝不重新启动目标。上游 `continue_async`、`wait_for_stop` 和 `break_in` 持有执行句柄。因该后端缺少独立跳出/断点开关工具，仅固定 `gu`、`be N` 和 `bd N` 使用上游 `execute`，其中 ID 必须是已校验的数字；仍不接受任意命令。

目标输出和调试器 stderr 使用有界管道读取，stderr 最多保留 1 MiB。MCP 错误保留为原始结果数据。每次回复发布受保护的日志前缀回执，包含长度、SHA256 和请求绑定。校验归属后的 `file.get`、`file.stat` 和 `file.read` 可在日志增长时读取该不可变前缀。超过 32 KiB 的结果通过大小/哈希及日志引用返回，避免填满账本；8 MiB 上限的日志保留原始工具结果。本机原生测试覆盖参数拒绝、回复绑定、诊断上限、不可变前缀，以及在查询新输入前核对中断启动。Clippy/格式检查通过。这些检查不证明已安装 AppContainer 会话验收；先前 VM 结果保留原来的源码及范围。

`native/process_capture.rs` 复用标准库匿名管道及安全句柄归属转换，分别处理二进制 stdin/stdout/stderr。读取线程在同步 profile/私有窗口站准备后才启动。两条输出流合计保留 1 KiB–8 MiB；超限、截止时间或取消会先停止并排空同一个 Job，再汇合 I/O，stdin 另行报告实际写入字节及完成/错误。流式分析启动在第一条指令前配置并读回 Job 总内存（16 MiB–4 GiB）、进程数（1–64）及低于正常的优先级。成功停止还等待所持主进程对象发出退出信号：终止收尾时 Job 活动进程计数可能更早归零。用户提供的 Windows 10 VM 已通过可信自编译夹具检查，覆盖二进制流分离、输出洪泛、内存/进程限制强制、取消/截止时间，以及持有输出写端的后代。上文独立的已安装 System/AppContainer/控制器验收仅验证有限独立进程通路。

内部资源账本现在还可为活动资源保留带类型的调试控制和交互写入副作用。它检查资源类型及 run/worker 绑定，串行化待定变更，保留稳定请求指纹，并拒绝关闭中或已关闭资源的新工作。已完成的变更可再次读取，但不会重新开始；中断的变更在数据库重新打开后仍保持待定，仅在归属运行时提供清理确认后成为 `uncertain_closed`。恢复可读取原始启动副作用、归属、类型、规格及阶段，包含旧系统启动留下的未完成意图。启动意图缺失/歧义或载荷指纹变化时拒绝继续。这些记录描述持久意图，不是操作系统存活或清理的观察证据。原生运行时使用前仍须验证当前控制器租约；进程和托管调试启动 RPC 已使用此账本，其余执行通路仍待完成。

启动资格检查还在每个 target/debugger AppContainer 中包含固定的自身 DACL 对抗探针。检查真实令牌为零能力且仅保留遍历特权后，密封子进程通过自身进程和线程伪句柄，将两者的 DACL 替换为 Everyone/完全访问，并读回验证。随后对仍存活的 System 资源宿主分别请求十种进程权限和九种线程权限，覆盖内存访问、句柄复制、线程创建/上下文/令牌修改、终止及安全修改；只有 `ERROR_ACCESS_DENIED` 才能通过。不对其他进程实际行使任何取得的权限。System 父进程将报告中的 PID/TID 与自己的当前身份核对。私有文件、scratch/guard 及关联网络检查在自身 DACL 修改之后执行，随后沿用已有 Job/profile/scratch 清理。这验证可变自身 DACL 下的指定边界，不证明进程 ACL 不可变或完整对抗隔离。[伪句柄约定](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getcurrentprocess)和[安全 API](https://learn.microsoft.com/en-us/windows/win32/api/aclapi/nf-aclapi-setsecurityinfo)说明了 SDK 机制。生产执行继续禁用。

2026-11-13，用户提供的离线 Windows 10 VM 在源码 `f44a562` 的全新安装及服务重启中通过此自身 DACL 门槛。两种角色都实际替换 DACL、取得全部 19 项逐项访问拒绝，并通过随后的私有文件/根目录/网络检查和清理；隔离 x64/x86 PDB 资格检查也通过。回执 SHA256 分别为 `881323e912abee09d15786097480fb5350e8dad932550d189b208d01c4dd623a` 和 `5b5dfb8b73ed30028719bd800ba74a3860f09ce0ae3c8de8e01ac1d61859f4f0`。全部 13 份原始回执、宿主/探针/调试器日志及公开连接记录，经独立固定指纹 OpenSSH 文件传输传回并核对大小/SHA256。真实控制器 SSH 验收在重启前后均通过，租约 epoch 为 1–4，不可变/分块输入均取得原生证明。VM 没有默认路由，安装 ISO 已卸载，之前已通过验收的服务均保留。此结果不证明全部对抗场景、通用资源执行或完整离线维护生命周期通过。

`native/resource_input.rs` 为原生资源准备提供内部 System 侧输入读取入口。原生调用方必须先验证当前租约并提供其归属；序列化引用仅用于选择输入。查询使用已有私有哈希存储，核对声明/已完成回执、精确的归属/run/epoch/worker 绑定、内容 ID、SHA256、大小及可执行策略。它不打开 worker 提供的操作系统源路径，也不创建缺失的归属目录。执行准备会拒绝仅作为数据上传的输入。返回的读取器持续持有同一个已验证的单链接、无重解析文件句柄，直到完成字节读取并销毁读取器前均禁止写入/删除共享。上传完成也会持续持有该句柄直到发布原生回执。这复用已有输入哈希/ACL 检查；原生进程运行时现已解析这些引用，其他执行通路仍待完成。

2026-11-13，用户提供的离线 Windows 10 VM 在源码 `51d04dc`、清单 `2e63de639cb4461050ae4dc1632185f89b62f5fb60f2ce7be08d9a8e66f411ea` 上通过全新安装，包含由 System 服务创建私有 SQLite 资源账本及持有独占写锁。真实固定指纹 SSH 控制器验收在服务重启前后均通过，租约 epoch 为 1–4，不可变/分块上传均取得原生来源证明。两次启动均通过隔离 x64/x86 PDB、断点、栈及清理资格检查；回执 SHA256 分别为 `bd10cb180efcf80309650790d6a3d0decff4a3206f3b8cea55a9fb556cca3884` 和 `07b4568f5bddccd6fa66b0368898f9638ea5a5de785c0c5385990392d860f145`。全部九份原始公开回执/转录通过独立固定指纹 OpenSSH 文件传输传回，并核对字节数及 SHA256。VM 没有默认路由，随后已卸载安装 ISO。此结果证明该源码的安装、启动及控制器传输；资源执行和完整离线维护仍未完成。

服务现在会打开独立的 System/管理员私有资源账本 `metadata/native-resources/resources.sqlite`，复用控制器已有的 `rusqlite`/内置 SQLite 库，以及 worker 的绑定、归属和哈希类型。全同步事务限制同时只有一个活动资源，每个资源只有一个待定变更。稳定请求身份包含 worker 安装/启动/包绑定及 run，控制器和租约变化仅作为审计数据。相同 ID 的参数变化会被拒绝；已完成或已中断的副作用不会重新开始。中断的启动在数据库重新打开后仍保持待定。若原始结果未知，关闭仅记录 `uncertain_closed`，不会报告成功。

独占写入锁及持有的禁止删除共享数据库句柄阻止并发接管和替换。有界数据库使用 SQLite 回滚恢复、受保护文件及规范化 Windows 路径，不引入另一套文件系统事务引擎。活动或不确定的账本记录阻止原生租约释放和过期接管，服务重启后也一样；续租仍可执行。用户提供的 Windows 10 VM 上，无目标执行测试覆盖真实终止 SQLite 写入进程后热回滚日志的恢复、长路径、重放身份、陈旧完成结果及两种租约屏障。清理标志仅为内部业务前提，不是独立的操作系统证据。独立进程创建已接入；原生归属恢复及其他执行 RPC 通路仍未完成；broker 仍仅支持健康检查，并报告 `production_execution = false`。

独立 Cargo 包构建 worker 组件，不链接桌面控制器、WebView 或模型客户端。调试使用固定版本的上游 `windbg-mcp` 程序和官方 rmcp 客户端。自写 DbgEng 辅助程序、通信协议及 Python 会话实现已删除。交付身份绑定实际源码、二进制内容和运行时协议。

离线锁固定 [glslang/windbg-mcp v0.24.0](https://github.com/glslang/windbg-mcp/releases/tag/v0.24.0) 的原始源码压缩包，修订为 `91fba9283c5f9f9bb94cd7f9b04c0aa4f7885e8c`。控制器沿用上游锁定依赖构建，仅应用一份[标准输入兼容补丁](patches/windbg-mcp-pipe-stdin.patch)：引擎子进程从写端已关闭的管道接收 EOF，不再打开 `NUL`。提供的 Windows 10 VM 已实测原版工具在 MCP 握手成功后，被 AppContainer 拒绝访问 `NUL`。补丁不改变 MCP 或调试逻辑，也不修改设备 ACL。原生构建元数据绑定源码压缩包、补丁、可执行文件和依赖许可证的哈希；组装时重新检查，并携带原始源码、补丁和许可证。补丁版受限调试资格验证已在 `1556c9a` 通过，记录见上文。

x64 服务与固定 SDK 引擎相邻放置，支持原生 x64 和 WOW64 分析；不包含 32 位 .NET SOS。相邻的 `vcruntime140.dll` 来自已固定的 Python 载荷；系统 UCRT 由 Windows 10 提供。VM 无需编译器或下载软件包。旧清单仍可读取以支持维护，但新后端不会执行旧辅助程序；新包仅选择外部 MCP 入口。

现有控制端 Rig/rmcp 客户端已通过两种架构的专用 VM dump/live 验收，参见[外部调试器验收](../tests/README.zh-CN.md#外部-windbg-mcp-验收)。原生宿主复用 rmcp，帧上限为 2 MiB、单次调用期限为 40 秒、受保护 JSONL 记录上限为 8 MiB。超时保留结果不确定状态并触发所属进程清理，绝不自动重放变更。调用保留标准 MCP 返回类型，包括 `set_symbol_path` 这类仅返回文本的成功结果；仅需读取类型化字段的检查要求 `structuredContent`。模块/PDB 观察来自外部工具；文本结果保留为原始证据，不自写文本解析器。通用原生调试准入、完整映像对应关系及 dump/符号负向验收仍未完成。Python 元数据 worker 不宣称或启动 WinDbg 会话。

当前服务每次启动都会生成全新的 128 位资格检查 ID，在管理 SSH/服务就绪前运行已签封的 System 资源宿主；同一次系统启动中的服务重启也须重新运行。schema4 隔离回执绑定该 ID 与精确清单，缺失或旧 ID 无法通过验证。每份回执以 `metadata/isolation-selftest-MANIFEST-QUALIFICATION.json` 新建，保留此前证据而不覆盖。历史上仅绑定清单的回执不会跳过探针。此实现复用已有进程/Job 资格检查路径，不另写系统启动身份实现。普通依赖自检的缓存单独保留；重新进行资源检查不代表生产执行、生命周期恢复或系统重启验收成功。专用验收入口要求服务重启后出现新的资格回执。用户提供的 VM 已在 `7b6c4c0` 通过该检查和系统重启资格检查；未重跑托管 runner 夹具。

父进程通过既有挂起子进程 AppContainer 启动器准入外部服务，要求零 capability 且仅启用 `SeChangeNotifyPrivilege`。独立隔离的目标仅向所属调试器身份授予访问权限。操作系统网络及进程/文件权限仍强制执行；不提升服务权限，也不接收模型选择的 PID、工具路径或符号路径。固定验收选择本地夹具符号目录，保留原始 MCP 结果和独立 stderr；这不证明上游全部引擎选项或通用符号搜索行为已通过认证。

调试目标现保留初始 `CREATE_SUSPENDED` 线程句柄，直到 MCP 初始化完成。已有 Windows SDK 绑定通过所属进程句柄调用 `CheckRemoteDebuggerPresent`，确认附加后仅释放一次初始挂起计数。外部工具随后可完成加载器/初始断点等待；调试事件仍按 Windows 正常语义保持执行停止。未附加时绝不放行目标。DbgEng 在附加时可以增加自己的挂起计数：专用 VM 返回的先前计数为二，第一版门禁错误地拒绝了它。修正版仅释放自己持有的一次初始计数，保留调试器持有的计数；计数为零或 SDK 调用失败时结束资源而不重试。资格检查使用 `--debug-once`，仅调用一次目标符号便退出，不再依赖历史循环夹具。回执额外要求 `pre_execution_attach`、`single_pass_target`。本地回归证明附加前拒绝放行，并阻止将仍挂起的目标转交 System 宿主。参见 SDK 的[附加契约](https://learn.microsoft.com/en-us/windows/win32/api/debugapi/nf-debugapi-debugactiveprocess)和[线程恢复契约](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-resumethread)。

固定隔离 MCP 验收要求 x64、x86 均解析 `wukong_probe!fixture_step` 断点，在其地址非超时停止，程序计数器与架构对应，目标模块加载 PDB 符号，栈顶帧对应同一地址和符号。随后释放会话、清空两个 Job、删除身份配置并回收 scratch。回执必须包含 `backend = "windbg_mcp"`，旧辅助程序回执不能证明新实现通过。专用 VM 已在 `1556c9a` 的首次启动、服务重启及系统重启中通过此替代实现。下文较早的 `7b6c4c0` 结果描述已删除的辅助程序。

原生输入发布会规范化已经验证的私有元数据根目录，并在直接 SDK 调用中保留 Windows 长路径格式。因此，在未启用长路径选项的 VM 中，不可变输入和分块输入成员也可以超过旧的 260 字符路径限制。专用 Windows 10 VM 回归使用特意加长的私有根目录，验证发布成功及原始内容来源证明；无需修改全局长路径设置或扩大目录权限。

挂起附加可能以不同顺序暴露原生附加断点和 WOW64 加载器通知。已保留的 VM 失败记录中，`c4f6164` 停在 `0x4000001f`，`8cc728b` 停在 `ntdll!DbgBreakPoint`（`0x80000003`）；两个实例均已清空所属资源，原始证据已核对哈希。固定资格检查复用上游 `exception_triage`，仅读取一帧并禁用栈扫描。对于刚观察到的停止地址，每种可继续的首次机会通知最多接受一次：WOW64 代码要求 x86 夹具，原生断点要求匹配当前 `ntdll!DbgBreakPoint` 栈帧且偏移为零。未知或重复异常、陈旧崩溃上下文、超时及中断均拒绝通过资格检查。此规则仅用于已签封夹具，没有新增控制台文本解析器。回执记录 `wow64_loader_break_observed` 和 `native_initial_break_observed`。2026-11-13，专用离线 Windows 10 VM 已在 `f6cc228` 通过全新安装和服务重启。两次启动均命中仅执行一次的 x64/x86 夹具符号，寄存器与栈地址一致。第一次 x86 启动观察到两种已知通知，第二次仅观察到 WOW64 通知。38 次 MCP 调用全部成功，四个会话均已释放且引擎 worker 已终止，17 份原始文件通过固定主机密钥的 SFTP 复制并核对哈希。两份资格回执的 SHA256 为 `b8578d2636f6ab866e3cc9675d5406791b645d424aa5a5d21b7884e2ceaf90f0` 和 `0917813fb164755db85f9cfeccf5b889620637635f78be5cfb12e112c9015def`。scratch 为空，公开连接身份未变，管理 SSH 保持可用，测试 ISO 已卸载。这验证了挂起启动夹具门禁；通用调试会话准入仍未完成。

在仓库根目录的 x64 MSVC Native Tools shell 中运行：

```powershell
python -m scripts.worker.build_native --check
python -m scripts.worker.toolchain --cache target/windows-offline-cache --component windbg_mcp --download
python -m scripts.worker.build_native
cargo fmt --manifest-path workers/windows/Cargo.toml -- --check
```

构建入口使用已提交的独立锁文件，并显式设置静态 CRT。默认输出位于 `target/windows-worker-native`。写入 `native-build.json` 前，调用刚构建的控制端工具的 `inspect-pe` 命令：已有 `object` 库读取普通和延迟加载的 PE 导入，Python 应用拒绝外部 Visual C++/Universal CRT 的策略。不再包含 Python PE 解析器，也无需额外下载包。组装时先验证每个原生二进制的哈希，包含检查器自身，再运行工具并与构建清单中的导入比较。检查仍限制为 64 MiB 映像、96 个节、64 KiB 导入目录和 256 字节内终止的 DLL 名，显式指定 x64/x86 架构，延迟描述符只允许 RVA 寻址。该检查适用于引导程序自身；离线工具包仍须单独包含第三方工具的全部依赖及原始安装载荷。控制端工具不进入 VM 运行载荷。构建不会执行安装、服务注册或分析目标。

实现依据 [Rust CRT 链接约定](https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes)与微软 [PE 格式](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format)。Windows CI 对独立 crate 进行格式与代码检查、构建引导程序并验证导入；自动化测试还拒绝动态 CRT、非法库名与格式错误的普通或延迟导入表。

原生库可将已验证载荷暂存到新建目录，显式设置受保护的 Windows ACL。源文件须为单链接普通文件，打开时不跟随末级重解析点，也不共享写入或删除权限；在创建可读目标文件前，先通过持有的句柄核对签名清单中的大小及哈希，随后对复制内容计算哈希，并再次校验完整目标目录。清单与公钥字节来自已验证的内存身份；在暂存前，通过持有句柄有界读取的签名须与该清单匹配。VM 策略区分 System/管理员元数据与可读的可执行内容。源介质和暂存父目录仍须防止其他进程并发写入；这些检查不提供从目标可写目录进行通用提权收集的能力。失败暂存仍由调用方持有，用于事务恢复。该模块不激活版本，也不创建服务或账号。普通 Windows CI 测试仅使用无执行能力的签名文件及仅当前用户可访问的临时目录，验证真实共享冲突、重命名拒绝、硬链接拒绝、权限，以及源文件变更、相对根目录和覆盖写入的拒绝路径，不在开发宿主安装任何软件。

显式只读介质夹具可以将签封载荷复制到新临时目录并再次校验，不执行载荷，也不注册系统资源。仅以只读方式挂载指定 ISO，并独立选择其公钥；替换下列占位值。测试自身不挂载或卸载介质，也不执行 VM 安装验收。

```powershell
$env:WUKONG_STAGE_TEST_SOURCE = 'X:\payload'
$env:WUKONG_STAGE_TEST_PUBLIC_KEY = 'PINNED_CONTROLLER_PUBLIC_KEY_HEX'
$env:WUKONG_STAGE_TEST_CONTROLLER_VERSION = 'CONTROLLER_VERSION'
cargo test --locked --manifest-path workers/windows/Cargo.toml --lib stages_explicit_read_only_offline_media_without_execution -- --ignored
```

`wukong-worker.exe verify-package DIRECTORY PINNED_PUBLIC_KEY_HEX CONTROLLER_VERSION` 使用[离线包约定](../package/README.zh-CN.md)校验独立信任、兼容性与文件精确内容。`verify-enrollment PACKAGE ENROLLMENT_DIRECTORY PINNED_PUBLIC_KEY_HEX CONTROLLER_VERSION` 另行校验登记签名及载荷绑定。两个命令均不安装或激活 worker。静态 CRT 检查覆盖 Wukong 引导二进制；外部调试器及固定运行时依赖作为包内容校验。

`toolchain.lock.json` 以大小与 SHA256 固定官方 Python、OpenSSH、Python wheel 和完整 Debugging Tools MSI/CAB 清单。锁文件中的 OpenSSH 版本为上游**预览版**。在控制端准备缓存：

```powershell
python -m scripts.worker.toolchain --cache target/windows-offline-cache --download
python -m scripts.worker.toolchain --cache target/windows-offline-cache
python -m scripts.worker.layout --cache target/windows-offline-cache --output target/windows-offline-layout
```

只有 `--download` 允许网络访问。已有缓存条目不匹配时直接失败，不替换原文件。展开要求 Windows 控制端和全新输出目录，没有联网或下载路径。它以只读方式查询 MSI 表，仅解压回调批准的 CAB 成员，将完整调试器载荷清单与已固定的微软引导程序内部元数据核对，并保留全部原始 MSI/CAB/ZIP/wheel 及许可证。不会运行 MSI 安装、自定义操作或修改已安装工具。解压使用微软 [MSI 数据库](https://learn.microsoft.com/en-us/windows/win32/api/msiquery/nf-msiquery-msiviewfetch)及 [cabinet 回调](https://learn.microsoft.com/en-us/windows/win32/setupapi/spfilenotify-fileincabinet) API。

输出包含已预展开 wheel 的私有嵌入式 Python、Python worker 源码、OpenSSH 和完整 SDK 调试器文件树。Python 忽略用户和注册表路径，仅加载随包库及 worker 目录；VM 无须 pip。归档路径拒绝链接、目录穿越、别名与覆盖写入。缓存和暂存目录应仅供控制端访问：这些检查本身不提供并发写入隔离。失败输出保留用于诊断，不能复用。该命令生成**未签名的控制端展开目录**。普通控制端展开检查与可信样例不代表干净 Windows VM 的依赖覆盖。

组装并签名新的完整载荷前，先提交 worker 改动，再从该干净提交重新构建。组装器要求源码提交与目录哈希匹配，并核对已锁定的原生构建元数据、实际二进制哈希和静态 CRT 导入。包内包含原生二进制及其依赖许可文本、Rust 标准库版权声明和全部原始第三方载荷。原生声明收集保留原始 `LICENSE`、`COPYING`、`NOTICE`、`UNLICENSE`、`COPYRIGHT` 文件及显式声明的许可证文件；以 `COPYRIGHT` 文件分发的完整许可证文本会原样保留。不会下载缺失文件。

```powershell
$keys = Join-Path $PWD 'target/controller-signing'
./target/windows-worker-native/x86_64-pc-windows-msvc/release/wukong-package.exe key $keys
python -m scripts.worker.assemble --cache target/windows-offline-cache --output target/windows-worker-package --keys $keys
```

密钥命令仅输出公钥及指纹，供独立登记。私钥存储在控制端且位于包外，按[工具包约定](../package/README.zh-CN.md)保护；示例采用本地开发目录。`wukong-package.exe` 是控制端实用程序，不进入 VM 载荷。签名输出绑定完整文件与兼容性，但在声称生产安装支持前，仍须完成依赖与自检、原生安装器/服务/broker、桌面交付与登记，以及真实离线 VM 验收。

准备好公开登记草稿后，`wukong-package enroll PACKAGE DRAFT ABSOLUTE_KEYS NEW_ABSOLUTE_OUTPUT CONTROLLER_VERSION` 使用控制端受保护的签名身份校验包文件，检查签名登记记录与载荷的绑定关系，并仅在载荷之外生成公开登记记录。[登记约定](../package/README.zh-CN.md)说明各字段及首次信任边界。该命令不会创建 VM 账号、安装服务或自动登记 SSH。

原生构建入口还使用控制端 MSVC 工具链及静态 CRT 编译已知 x64/x86 探针与匹配 PDB。它们通过哈希绑定原生构建元数据，并复制到签名包中，客户机无须编译器。原生程序使用独立登记的公钥完成签名和目录校验后，操作员或安装器可运行随包 Python 入口：

```powershell
./target/windows-worker-package/runtime/python/python.exe -B ./target/windows-worker-package/runtime/worker/worker_selftest.py --package target/windows-worker-package --state target/windows-worker-selftest
```

原生调用方须先建立工具包信任，并防止其他进程写入版本目录。该入口不是 worker RPC，也不替代签名校验。它在执行前检查随包探针哈希，无须网络即可测试 OpenSSH 密钥生成，并使用随包运行时检查 x64/x86 可信执行、dump 生成及每种架构三次重放。固定 SSH 探针会额外接收所需的 Windows `ProgramData` 路径，不改变目标作业的环境变量过滤。临时测试 SSH 密钥会删除，日志及结果保存在签名目录之外。成功报告明确仅涵盖离线依赖和可信样例，不代表生产目标隔离或服务/SSH 安装。运行后应再次校验签名目录。

调试组件使用外部 WinDbg MCP 工具。直接可信夹具互通与托管准入、离线 VM 安装分别验收。Python 依赖测试检查可信执行、dump 生成和重放；隔离调试器由原生启动资格检查负责。重放使用不同生产者/验证者 ID，每种架构执行三次全新进程与目录试验，检查目标/输入精确哈希及 `0xc0000005` 异常。这不证明利用影响。参见[跨平台约定](../../docs/design/cross-platform-environments.zh-CN.md)和[验收指南](../tests/README.zh-CN.md)。

原生 VM 引导程序与 SCM 服务已有可调用的实现路径。从已签名完整载荷和匹配的签名登记生成新的公开安装介质：

```powershell
python -m scripts.worker.bundle --package ABSOLUTE_PACKAGE --enrollment ABSOLUTE_PUBLIC_ENROLLMENT --output NEW_ABSOLUTE_MEDIA --public-key PINNED_CONTROLLER_PUBLIC_KEY_HEX --controller-version CONTROLLER_VERSION --verifier ABSOLUTE_NATIVE_WORKER_EXE
```

介质包含 `WukongWorkerSetup.exe`、`setup.json`、`payload/` 和两个公开登记记录，不包含控制端私钥。应传入独立 Windows 测试 VM，绝不能安装到开发宿主上。安装界面展示版本、安装身份、允许的控制端地址、SSH 端口及签名指纹。首次由所有者建立信任时，必须通过独立通道与控制端比对指纹。提权后再次核对用户刚确认的精确登记与完整载荷，随后才创建系统资源。当前原生引导界面为英文。生成介质不代表安装验收完成。

已在独立测试环境取得管理员权限的自动化可以运行 `WukongWorkerSetup.exe install-reviewed ABSOLUTE_SETUP_JSON PINNED_CONTROLLER_PUBLIC_KEY_HEX REVIEWED_ENROLLMENT_SHA256`。参数须提供独立固定的控制端公钥，以及在控制端审阅过的精确规范 `enrollment.json` 的 SHA256。该命令采用相同的校验和安装路径，返回 JSON 凭据，不显示对话框，也不会自行提权。它是操作员入口，不是 worker RPC；不能用于覆盖开发宿主上的安装。

安装成功后，仅将公开的 `connection.json` 复制回控制端。在“工作区设置 → 分析环境 → 登记 Windows 虚拟机”中导入记录、输入虚拟机地址、选择已有控制端私钥，并独立输入安装界面显示的主机指纹。预览校验已安装记录的结构、Ed25519 公钥与指纹、安装账户及已有密钥文件路径，不读取私钥内容、不创建文件，也不连接虚拟机。重复环境 ID 会被拒绝。加入草稿保留其他配置修改及注释。保存时在控制端设置目录创建以内容命名、以 LF 结尾的公开主机固定文件；已有内容不同则拒绝覆盖。清理仅登记该精确公开文件；所选私钥保持为外部文件。未完成的连接在关闭设置后保留，保存前须先加入草稿或重置。首个环境自动成为默认环境。登记独立于连通性及生产隔离检查；桌面介质准备与完整 VM 生命周期验收尚未完成。

创建服务前，固定的主机密钥生成及 SSH 配置检查将诊断保存在仅 System/管理员可访问的 `metadata/install-tool-ssh-keygen.log` 和 `metadata/install-tool-sshd.log`。错误会标明失败的程序；SSH 语法检查输出详细诊断，但不监听连接。托管夹具仅将这两个固定日志各自最后 4 KiB 纳入公开报告，不包含私钥或其他元数据。

提权安装工具与 System SSH daemon 使用固定的八变量 Windows 环境：`SYSTEMROOT`、`SYSTEMDRIVE`、`WINDIR`、`PROGRAMDATA`、`COMSPEC`、`PATH`、`TEMP` 和 `TMP`。`SYSTEMDRIVE` 从已验证的 Windows 根路径盘符推导；即使已有 `PROGRAMDATA`，默认已知文件夹展开仍需要它。通过受 Job 约束的全新测试进程，以这个精确环境检查路径解析，排除继承变量及预热 Shell 缓存的影响。路径从 Windows 位置派生，程序查找仅限 Windows 目录和 System32。其临时目录为受保护的 `metadata/system-tmp`，在启动及激活前验证。普通 worker 继续使用独立的 `data/tmp`；不会将控制端/provider 的整个环境复制给提权子进程。

若 SSH 语法检查发生访问违例，独立托管夹具还会比较最小环境与固定的 Windows 运行环境，再使用签封 SDK 中的 CDB，仅检查出错指令指针、不含参数的调用栈和模块身份。探测保留已有程序网络阻断规则，并使用空的本地符号目录，不创建内存转储。报告不包含完整寄存器显示、私钥字节或指定调试区段外的输出；有界诊断输出不代表安装成功。

安装创建独立的管理、元数据 worker、调试器和目标账户，在 Windows ProgramData 已知目录下记录归属，并将生成的 worker 密码存入 LSA。所有账户禁止交互式/RDP 登录，仅 broker 持有的账户取得批处理登录权限。服务验证活动版本签名、目录及自身程序后，以普通账户、独立桌面和移除所有枚举特权的令牌启动 Python。显式进程属性确保子进程在执行前进入不允许脱离的 Job，且只继承输入/日志句柄。worker 代次退出时，服务先终止其所有后代进程再停止。首次就绪前必须通过固定依赖自检，并保存仅 System/管理员可写、绑定清单的凭据；其验证范围仍为可信样例。

随包 SSH 使用独立主机密钥、仅公钥认证、登记的控制端地址和忽略请求命令的固定原生中继。转发、TTY 及用户启动钩子均被禁用。原生防火墙规则限定来源、端口和程序。管道 ACL 不授予客户端创建服务端实例的权限；每帧检查实际 Windows 调用者 SID，中继发送数据前将服务端 PID 与 SCM 比对。回环端点令牌留在受保护 worker 状态中。原生身份绑定覆盖完整签名清单。遇到现有安装、服务或账户名时拒绝采用或覆盖；中断记录保留供显式修复。

专用 SCM 服务首次启动前会设置显式安全描述符：由 Administrators 持有，System/Administrators 保留完整服务权限，仅向本安装的管理 SID 授予 `SERVICE_QUERY_STATUS`。非交互 SSH 中继用该权限向 SCM 核对管道服务端 PID，不获得启动、停止、配置修改、所有权或 DACL 修改权限。启动时回读并核对精确所有者、主体和权限掩码，不修改 SCM 管理器及其他服务。本地内存中的 Windows AccessCheck 夹具验证仅状态查询准入及控制权限拒绝，不创建服务；真实登记及 SSH 检查仍在独立 VM 中验收。

`ssh/` 目录从创建起就仅允许 System/管理员访问，因此主机密钥生成期间不会在文件权限最终设置前暴露私钥。管理账户可读的公钥认证文件单独放在 `bootstrap/authorized_keys`。激活前和服务启动时，部署验证会检查这些目录及文件的 ACL，覆盖两个生成的主机密钥文件和服务器配置。

防火墙登记将签封程序解析为规范的 Windows 本地路径。失败会标明 COM 属性或登记步骤；登记错误包含公开的程序、端口、来源地址及配置文件范围设置。本地测试在未登记的规则对象上验证真实 COM 属性设置/读取和路径身份，不添加或修改任何宿主防火墙规则。实际登记仍需独立 VM 验收。

SCM 服务登记及固定原生子进程启动采用相同的本地路径规范化，使服务归属比较与已验证程序保持一致。外部调试器失败时，在受保护资格检查日志中保留有界 MCP 结果和 stderr。这些诊断不会放宽普通账户权限，也不会将失败的自检视为就绪。

Python 元数据 worker 使用既有的绝对本地盘符名称校验签封的 broker-health 程序，拒绝链接、重解析点及歧义组件。该检查由 `worker_windows_paths.py` 独立负责，不再属于调试器。真实 Windows 零特权令牌测试覆盖本地文件访问；普通访问校验不授予特权，也不恢复已删除的调试器路径协议。

账户创建按已知 SID 解析内置 Users 别名，并按 SID 将专用账户加入该组，避免依赖本地化组名。早期恢复严格限于载荷、数据与服务激活前中断的管理账户创建。准备新介质时加上 `--previous-package ABSOLUTE_PREVIOUS_PACKAGE --previous-enrollment ABSOLUTE_PREVIOUS_PUBLIC_ENROLLMENT`，即可携带原始公开签名证据。恢复将证据与受保护安装记录比对，要求更高代次的签名登记，并保留已接受的恢复代次。同时核对账户精确归属注释、普通用户权限和 SID，不重设其密码。该路径拒绝其他现有账户和后续安装阶段。首次激活前仍须由所有者核对指纹。

引导程序在创建账户前，将完整的首次所有者签名登记写入仅 System/管理员可写的 `metadata/authority.json`。恢复与服务启动从这份受保护记录取得签名公钥；清单哈希相同也不能授权替换签名身份。已接受的恢复记录在中断后仍保留递增的最高代次，启动时活动登记必须与已接受的最高记录一致。签名变更、过期/重用信封及非规范准入文件名会被拒绝。若中断的开发期引导仅留有原始安装意图记录，仍须提供原始签名证据并由所有者独立比对指纹，才能首次建立该信任记录；此窄路径不会采用已安装的 worker，也不实现通用迁移。失败更新的回滚需要另行授权的激活凭据，仍属于后续生命周期工作。

服务的 `lease_proxy` 持有面向控制端的 run 隔离令牌，并将递增 epoch 日志保存在仅 System/管理员可写的元数据中。它验证就绪时的安装、启动与包身份，在转发前逐次校验令牌，并替换为独立的 Python 租约，不向控制端披露内层令牌。长调用不会阻止续租。仍在执行、被取消或传输结果不明的业务调用阻止释放及过期接管；不确定调用保留为空闲阻塞项，直到 worker 代次被终止。日志支持最多 100,000 个新令牌且不覆盖记录，续租不消耗记录。本地无目标执行夹具检查真实受保护文件、并发写入拒绝、重启持久性、陈旧请求拒绝、延迟续租及取消，使用受控上游适配器，并非已安装的 VM 服务。原生目标资源归属和完整维护空闲屏障仍需要 broker 及 VM 验收。

元数据 worker 另有独立的本地 broker 通道。其管道仅允许 System 和本次安装的 worker SID，拒绝远程客户端，也不授予 worker 创建其他管道实例的权限。每次请求核对内核报告的调用者 SID 及其是否属于当前 worker Job；模拟身份在任何异步工作前结束。固定且经过哈希校验的原生客户端在写入前，将内核报告的服务端 PID 与 System 父进程提供的启动绑定比对。最多同时处理四个请求，每个连接仅接受一个 4 KiB 帧，期限为三秒。目前仅实现 `health`：`doctor.native_broker` 返回安装、启动及包身份，并明确返回 `production_execution = false`。本地可信子进程测试覆盖合法归属、同一 SID 的其他 Job、错误 SID、错误服务端 PID，以及超限或未知请求。托管的已安装 worker 验收要求通过 SSH 完成同一健康检查。该通道尚不创建目标或调试器资源，不准入执行，也不构成维护空闲屏障。

内部 broker 通信契约定义在 `proto/broker.proto`。构建时由 `prost-build` 与纯 Rust 的 `protox` 编译器生成 Rust 消息；VM 不需要 protoc 或编译器。`tokio-util::codec::LengthDelimitedCodec` 负责四字节大端长度前缀、缓冲和 4 KiB 消息体上限。通信 schema 2 要求显式指定已知操作；缺失或未知操作、不兼容版本及格式错误的消息均拒绝处理。Protobuf 为前向兼容忽略未知字段，但不会因此授予额外操作。两个原生端点必须来自同一已验证软件包；旧 JSON 管道客户端将被拒绝。`broker-health` 命令行保留供 Python 使用的 schema-1 JSON 输出。控制端 SSH RPC 保留已有 JSON 协议；外部 WinDbg 工具通过 rmcp 使用标准 MCP。源码身份包含 `.proto` 文件。本地协议及命名管道测试不代表这一通信修订已通过安装后 VM 验收。

对于托管的 `file.put`，服务会将控制器传入内容解码后，另存一份至仅 System/管理员可访问的 `metadata/native-inputs/`。声明绑定安装、启动、包、run、控制器、原生租约 epoch、逻辑路径、精确哈希与大小，以及可执行策略。不会以 System 权限打开 worker 提供的源文件。这些上传操作的 Windows 路径采用工具包的可移植 ASCII 路径规则：最多 512 个字符，每个组件最多 200 个，不允许路径遍历、设备名称或含歧义的别名。单次内容仍限制为 32 MiB。每个归属范围限制为 16,384 个成员及 32 GiB，包含为声明、数据和凭据预留的空间；中断遗留的临时文件也计入两项预算。发布不会覆盖已有条目。仅在核对私有的单链接副本和声明后，才向匹配的成功 worker 结果附加 `native_input` 凭据；畸形、丢失或冲突的返回结果会让原生租约保持不确定状态。转发前的校验失败会直接收敛，不制造这种不确定性。同一隔离令牌内重试保留相同凭据，不同 run、控制器或 epoch 会取得独立身份。原生输入发布与租约续期分别串行化。断点续传也将控制器传入的数据块复制到该私有存储：每块最多 1 MiB，每次上传最多 16 GiB，原生归属范围包含元数据的预算为 32 GiB。状态返回原生端与 worker 进度中的较小值，以便向任一端补发中断的写入。重复块必须匹配，不允许缺块、声明变化或最终哈希不符。其他 run 或 epoch 中已存在的 worker 文件不能跳过原生端接收控制器字节的步骤。提交时，核对原生副本及匹配的 worker 文件结果后才发放凭据；同一隔离令牌内重试保留该凭据。原生传输 ID 与现有 Python 协议一致。资源创建及全部执行、调试和重放路径仍需原生准入后，才能启用生产执行。

服务成功停止及离线自检验收前，会显式终止每个所属 Job，并最多等待 10 秒，直到其活动进程数降为零。主进程退出不能证明其后代也已退出。等待失败时不能返回生命周期成功或写入自检凭据；析构清理仅作为错误情况下的尽力回收。worker 和 SSH Job 回收期间，SCM 的 stop-pending 状态保留等待提示和进度检查点。本地可信夹具在主进程退出后保留活跃子进程，并验证 Job 屏障终止该子进程。这项检查验证进程归属，并非已安装服务或目标隔离验收。

启动时还会在独立的 System 子进程 Job 中运行固定的原生资源探针。该宿主加载本次安装自有的目标与调试器用户 profile，并为每个角色创建不同的无 capability AppContainer。全新的 System 宿主会在修改注册表缓存或加载角色 profile 前解析固定 Windows 环境；子进程复用该快照，并使用各自的私有临时路径及经过筛选的自有角色 profile 路径。已知文件夹解析错误保留实际 HRESULT。只有这个单线程子进程会选择自身私有窗口站。目录及桌面权限同时包含独立的登录 SID 和 AppContainer SID；暂存目录与桌面对象设置低完整性标签。父进程先以目录列举和安全管理权限持有不可继承的暂存目录句柄，再授予沙箱访问权限；纯元数据句柄不会建立所需的共享限制。通过该句柄核对精确的安全描述符，并在整个生命周期内拒绝目录删除共享。目录数据写入共享不是不可变性保证。授予访问权限前，先创建仅 System/管理员可访问的保护文件，并持有不共享的文件句柄，使目录根始终非空，防止在其上设置挂载点重解析。首条指令执行前还会核对创建的 AppContainer 身份，并移除除已启用 `SeChangeNotifyPrivilege` 以外的全部特权。每个经过封装的探针必须证明仅保留该项特权且 capability 为零、不能读取元数据和 worker 哨兵文件、不能连接父进程已证明可达的回环监听端口、可以写入私有暂存目录，不能删除目录根的保护文件，也不能在目录根设置挂载点重解析。必须得到访问拒绝、共享冲突、特权拒绝或目录非空错误；请求格式错误不能作为通过证据。成功要求探针 Job 清空、通过持有句柄收回并删除暂存目录根、自有 AppContainer profile 删除，以及加载的用户 hive 卸载。只有显式确认 Job 清空后，才能收回目录根并移除私有保护文件；失败资源析构时保留保护文件，供后续修复处理。受保护的凭据绑定签名工具包。角色的用户 profile 目录保留至自有生命周期清理；托管验收只在删除账户前移除 SID 与本次安装自有账户匹配的用户 profile。后续固定门禁还会分别在目标 AppContainer 中启动经过封装的 x64/x86 测试目标，并在另一个调试器 AppContainer 中启动固定版本的外部 WinDbg MCP 服务。只有对应资源的登录 SID 和 AppContainer SID 获得目标进程及线程访问权限；对象完整性标签允许低完整性调试器使用这些显式权限。宿主传入自己仍持有的进程 PID，以及固定的本地引擎和符号路径。交互记录必须证明用户态附加已停止、架构正确、调试特权未启用、观察到寄存器和预期目标映像，随后 MCP 会话释放、Job 清空，以及 profile 和暂存目录清理完成。这些已有调用路径的可信夹具门禁仍需在专用环境中完成真实资格检查；它们不证明对抗性隔离或生产执行验收。资源准入和全部执行路径仍未完成。

每个沙箱进程及其主线程都使用显式的 System 所有者安全描述符，仅包含 System、资源的登录 SID 和 AppContainer SID，以及确有需要时独立配对的调试器。首条指令恢复前，父进程通过持有的进程和线程句柄回读实际所有者、精确 DACL 和低完整性 no-write-up 标签。本地 Authz 夹具在不创建内核 AppContainer 的情况下，检查具体的受限 SID 上下文、其他资源拒绝和配对调试器准入；权限扩大的描述符、变更的所有者或错误标签均拒绝。运行中的进程可通过自身伪句柄修改自己的 DACL，因此这项初始对象策略不证明对抗性会话隔离。生产执行保持禁用。

创建 AppContainer 时，仅向父进程侧的 Windows API 提供已验证的普通角色令牌。实际子进程在 Job 内以挂起状态创建；父进程核对其 AppContainer SID 和零 capability，移除除已启用 `SeChangeNotifyPrivilege` 以外的全部子进程特权，并核对这一精确操作系统特权身份后，才恢复执行。可信子进程回归比较零特权与沙箱仅保留目录遍历特权的策略，父进程令牌保持不变：后者允许经过禁止遍历的祖先访问明确可读的文件，但仍拒绝读取私有文件和列举目录。如果固定的原生隔离自检无法启动，仅 System 可用的创建对照会使用同一个经过签封的自检程序，比较源令牌、AppContainer 属性、桌面及初始对象策略。所有对照均保持挂起，必须显式确认其 Job 清空；清理结果不确定时停止后续对照。这些有界诊断保留原始启动失败，不提供执行降级，也不允许进入就绪状态。本地挂起子进程回归确认夹具指令未执行。真实 AppContainer 创建和 DbgEng 资格检查仍须在专用环境中完成。

创建每用户 AppContainer 进程时，也会进入自有角色的同步身份模拟作用域，与 profile 设置保持一致；在检查或恢复挂起子进程前，先恢复 System 身份。已有线程身份模拟时拒绝进入，避免嵌套作用域抹除调用者身份。原生创建错误在恢复身份前捕获。可信的同身份线程测试覆盖成功返回、拒绝嵌套、保留操作系统错误及 panic 展开，不创建账户或 profile。固定自检失败时，还保留始终挂起的 System 上下文创建对照。仍须完成真实专用环境资格检查；这些线程测试不能证明 AppContainer 可以启动。参见微软的[每用户 AppContainer profile 约定](https://learn.microsoft.com/en-us/windows/win32/api/userenv/nf-userenv-createappcontainerprofile)及[进程创建约定](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessasuserw)。

自有角色 profile 加载后，资源宿主通过禁用继承的 Windows `CreateEnvironmentBlock` 获取 `APPDATA`、`LOCALAPPDATA` 和 `USERPROFILE`。仅将这三个已有、无链接的本地目录补充至子进程环境，不复制账户 PATH 或任意其他变量，也不修改创建者环境。全新进程测试从缺少这些变量的环境启动，解析当前可信测试身份的真实 profile 路径并检查筛选，不创建账户或 profile。真实安装中的始终挂起对照表明，仅补充子进程 profile 路径即可创建进程，缺少这些路径时返回错误203。随后父进程核对挂起进程和线程的所有者及 DACL，通过持有句柄使用 `LABEL_SECURITY_INFORMATION` 设置低完整性 no-write-up 标签，并在任何指令恢复前回读完整的预期权限。真实本地挂起子进程测试检查两种对象的标签，清空 Job，且不执行夹具代码。仍须完成真实运行资格检查，不提供执行或就绪降级。

2026-11-13，开发提交 `008d805` 在提供的 Windows 10 1803/build17134 VM 中完成了全新签名离线安装，VM 没有 IPv4 或 IPv6 默认路由，服务进入 Running。与清单绑定的 schema3 回执确认：target/debugger 使用不同 AppContainer、零 capability、仅保留 `SeChangeNotifyPrivilege`，私有数据及根保护文件访问被拒绝，临时目录已清理，两个回环阻止事件均由 SDK 关联 WFP 证据验证。独立受限的 x64/x86 DbgEng 辅助程序附加到各自持有的目标，在没有 `SeDebugPrivilege` 的情况下观察寄存器和模块，随后关闭、排空 Job 并删除 profile/临时目录。回执 SHA256 为 `c7ae98f29cec3f261c5806b2ae1630d2e9a217bb4a3d26a818544f83f297f7f0`，绑定清单 `4671bc0501df1022b05898fb66b349d8bc43e3c373b69abbcbf1c5a861b84b39`。这是可信夹具资格验证，不证明对抗性资源准入或生产执行。受限转录仍提示绝对本地符号路径不可用；该门槛证明附加及寄存器/模块观察，不证明 PDB 加载、符号断点或完整控制/重放。该版本按清单缓存回执。当前每次启动重新验收的实现已在用户提供的 VM 上以 `7b6c4c0` 通过服务重启及系统重启检查，证据见下文。生产执行保持禁用；原生资源所有权与全部执行路径及完整离线生命周期仍未完成。较新的固定系统重启结果见下文。

可选的托管安装验收使用 `tests/acceptance/check_windows_worker.py` 及其 PowerShell 辅助脚本。两者都会在系统级副作用前拒绝开发宿主或自托管环境。夹具先作为控制端获取资源，使用仅留在控制端的临时密钥签封介质，再阻断介质和安装路径内随包程序访问外部地址，保留同一测试机已分配地址的管理连接和 CI agent 的连接。它验证真实原生安装器、普通账户自检、Rust 控制端固定 SSH 传输、不可变文件传输、陈旧令牌拒绝，以及服务重启后递增的 epoch。仅上传有界公开报告。清理前核对精确服务路径、账户归属注释、防火墙说明和安装意图，再删除夹具资源；剩余临时 LSA 密钥随一次性 VM 回收。这并非生产卸载、整台 VM 断网、Windows 10 VM 或目标隔离验收。配置流水线不代表实际运行通过，须查看真实报告与最终结果。

仅对固定的已签封隔离探针，父进程会在 Resume 前通过实际封存的子进程令牌记录有界启动访问观察：Windows SDK 模块枚举提供当前已签封程序及其已加载依赖，读取/执行及祖先目录遍历打开在现有同步身份模拟守卫内进行。通过父进程持有句柄，使用共用安全描述符所有权封装回读私有窗口站和桌面标签。最多保留八项拒绝，仅在探针失败时输出。这些观察不证明加载器访问过某个具体路径，也不改变特权、ACL、就绪或执行准入。本地测试枚举并打开当前进程的真实模块，无需解析 PE；真实受限令牌观察仍须专用安装验收。

回环探针使用现有固定版本的 socket2 库保留其实际绑定的源端口，报告观察结果，不将超时直接认定为拒绝。System 在子进程执行前后检查同一个监听器，并拒绝出现意外已接受连接。明确的 Windows 权限拒绝可以直接通过网络检查；超时还必须匹配 Windows SDK WFP 分类丢弃事件，核对已签封原生应用的 SDK 标识、精确回环套接字地址及端口、System 监听器身份和探针时间窗口。取回的执行过滤器必须是 `AppContainerLoopback`，采用阻止动作，并与事件的 IPv4 入站接收/接受层一致。回环丢弃事件保留 System 监听器身份，同时报告出站数据包方向（`FWP_DIRECTION_OUT`、`0x3901`）；2026-11-13 在专用 VM 中通过 Windows SDK 观察到了该值。接收/接受过滤层需独立核对，不能据此推定数据包方向。最终 WFP 回执包含 `event_direction = "outbound"`。[Windows 丢弃事件约定](https://learn.microsoft.com/zh-cn/windows/win32/api/fwpmtypes/ns-fwpmtypes-fwpm_net_event_classify_drop2)定义了该方向编码。只读枚举每个快照最多读取 1,024 个同应用事件，最多查询五个快照；不会启用事件收集或修改防火墙策略。证据缺失、收集禁用或无法确认时，资格检查失败。历史 schema3 和当前 schema4 回执均保留独立核对的网络证据；上文 2026-11-13 的提供 VM 回执包含两个已关联的 SDK 拒绝事件；其他环境尚未完成资格验收。

隔离探针失败时，在有界日志中保留操作位置及底层操作系统错误。外部调试器保留原始 MCP 记录和独立 stderr 日志；诊断数据不能将失败资格检查变为就绪。

临时资源根目录通过仅继承条目，把 `DELETE` 只授予各资源 AppContainer 和登录 SID 的后代文件与目录。根目录保留读、写、执行权限，不授予 `DELETE` 或 `FILE_DELETE_CHILD`；其受保护的私有保护文件不会继承这两项删除授权。现有描述符校验器单独核对精确的后代专用条目，普通策略仍拒绝此类条目。进程和文件系统授权夹具复用同一个 Windows Authz 辅助模块。专用 VM 回归验证真实 NTFS 继承、原先缺失删除权限的策略、其他资源拒绝、根目录及保护文件拒绝，以及回收。请在专用 Windows 测试 VM 中运行：

```powershell
cargo test --locked --manifest-path workers/windows/Cargo.toml --lib inherited_delete_permits_owned_children_but_not_root_guard_or_other_resources -- --ignored
```

这项回归不在普通宿主检查中执行。2026-11-13，开发宿主拒绝打开已授予 AppContainer 权限的夹具目录，而同一个测试程序在专用 Windows 10 1803 VM 中通过。该文件系统检查不证明已完成安装后服务或生命周期资格验收。

AppContainer 特权策略遵循 Windows 文档中的目录遍历例外：仅保留 `SeChangeNotifyPrivilege`，它跳过祖先目录遍历检查，不授予最终对象读写或目录列举权限。普通元数据 worker 保持现有零特权策略。沙箱令牌回读会拒绝缺失、禁用或任何额外特权（包括 `SeDebugPrivilege`），capability 数仍须为零。不通过修改机器全局目录 ACL 来补偿加载器拒绝。资格检查回执使用 schema4，带有本次启动的资格检查 ID 和明确的特权名称数组，仍要求真实哨兵/网络拒绝、自有 DbgEng 附加及静止清理。其他安装环境仍须专用验收；该策略不会启用生产执行。参见微软的[受限令牌约定](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken)、[文件访问指引](https://learn.microsoft.com/en-us/windows/win32/fileio/file-security-and-access-rights)及 [WFP 事件枚举约定](https://learn.microsoft.com/en-us/windows/win32/api/fwpmu/nf-fwpmu-fwpmneteventenum2)。

仅在控制端运行的远程验收入口复用托管夹具的传输、原生输入来源和租约检查，连接已安装并登记的专用 VM。将 `WUKONG_REMOTE_WORKER_TEST` 指向 JSON 配置，字段包括 `config`（固定主机公钥的 managed-SSH `RuntimeConfig`）、新的 `output` 路径、预期 `manifest_sha256` 和 `minimum_epoch`。远程入口要求私有、非回环 IPv4 地址及 managed 登记，不会在控制端安装或删除任何软件。托管安装入口继续保留 GitHub-hosted 和地址必须分配给本机的检查。

```powershell
$env:WUKONG_REMOTE_WORKER_TEST = (Resolve-Path .\remote-worker-profile.json)
cargo test --locked --lib remote_installed_windows_worker_controller_acceptance -- --ignored --nocapture
```

2026-11-13，同一台提供 VM 及上文软件包通过了真实控制端检查：产品 SSH 主机公钥固定、原生 broker 身份匹配、带原生来源证明的不可变及可恢复上传、读回、重试一致性、其他控制端冲突拒绝、递增 epoch 和过期租约拒绝。该检查不执行目标，也不证明更新、卸载或重启恢复。

2026-11-13，用户提供的离线 Windows 10 VM 完成了源码 `7b6c4c088521229b3805ca3fa13365fe40911804` 的资格检查，清单为 `eb97dbe0f70b0032c5db19999fd2bcf6d04a9c6bed8851179135797a02e8ac27`。静态 worker/辅助程序的 SHA256 分别为 `88883b84b8227b254ad414e386e711adeba91aff7c4683172e153ec82334ba17` 与 `d101f4682441c71e6cdecd0a7ab82ce86a353db977549b95c9d2deda8e2399a2`。全新安装、服务重启和系统重启各自生成独立的 schema4 回执，重新运行 x64/x86 受限 PDB 断点与栈检查，保留两个已由 SDK 关联的网络拒绝事件、哨兵/根保护限制及静止清理结果。VM 没有 IPv4/IPv6 默认路由。13 份原始回执、转录及连接文件均通过独立固定指纹的管理 SSH 回传，并核对了原始字节哈希。

| 启动阶段 | 资格检查 ID | 原始回执 SHA256 |
| --- | --- | --- |
| 首次安装 | `8ca136f6dbd16c288096b8a46ba1072e` | `0d2cea3995b62dfe2c3c0a414a4c617c65bcb7bfce02e30c8b78216d15454663` |
| 服务重启 | `3af6fd23c9a78ba1492e32975c72a517` | `71318df77775a63ac3d4053db6b39c20d167169dcd7c5a7c2736e2bb85240d6d` |
| 系统重启 | `760b3297ebd339a4cdb4c2df754fcb4f` | `7e73ed952c0b563196eda252e704136aff9198f355fa279cd740e4e47f76177f` |

真实控制器在首次安装及服务重启后通过验收，持久租约 epoch 分别增长至 2、4。独立的协调重启夹具保留了重启前的实际控制器传输：下一次读取失败，重连在写入前拒绝旧绑定；新控制器在成功读取已有夹具的同时确认写入哨兵不存在。启动配置项 GUID 保持为 `b79fde1663b711ef984cccad5e43e22f`，loader 序号从 4 变为 5；安装/包身份不变，租约 epoch 从 5 增至 6。管理 SSH 与原生服务自动启动，两份旧回执及产品主机身份均保持不变。重启报告 SHA256 为 `7ef998ac30a418097f5e2f7aebae03561e506c509d09b4526c478c6167068158`，入口见[协调夹具](../tests/README.zh-CN.md)。这些结果覆盖固定可信资源及控制器绑定，不证明对抗性执行准入、普通分析资源恢复、更新/修复/回滚/卸载、旧式完整用户态 dump 或有效内核 dump 拒绝。生产执行仍禁用；上述检查未使用托管 CI，也未发布版本。

## 原生 Python 源码作业

托管 `py.run` 现与 `job.start` 共用原生进程账本、当前租约校验、目标 persona、guardian Job、有界输出及清理/恢复路径。它使用签名包内的 CPython 入口，固定选项为 `-I -B -u -X utf8`，源码及用户参数位于解释器选项之后。提交的源码绝不在 System 服务或元数据 worker 内编译、执行。因此，CPython 语法错误产生带原始 stderr 的失败作业。原生资源类型仍为 `process`，保留 `job.status/list/cancel/artifacts` 行为。

源码为非空 UTF-8，最多 256 KiB，禁止 NUL。其哈希确定 `files/native-python/` 下密封、不可执行的输入；发布复用既有输入配额、持有文件核对和私有回执。最多 64 个 argv 参数及一个可选已准入目标沿用已有文件暂存策略。超时为 10–3600 秒（默认 120），输出为 1 KiB–8 MiB（默认 1 MiB），沿用 16 个进程/1 GiB 的 Job 限制。不接受环境、运行时或 LaunchSpec 覆盖。调试器准入仍要求原生映像。发布源码前先查询已完成或中断的启动，包括控制器归属变化后；恢复核对原始解释器及绑定源码的进程意图。

本机原生检查通过 137 项测试、五项明确跳过，覆盖不可变源码准入、其他 owner/执行访问拒绝、解释器与调试器隔离、无效参数在发布前拒绝，以及中断 ID 对账且不准入继任控制器源码。[已安装 Python 夹具](../tests/README.zh-CN.md#原生-python-验收) 已在 `d02d1db` 通过真实包内执行、已完成结果恢复及 x64/x86 进程回归。同一安装随后通过独立观察的 Python 宿主/服务终止，含真实原生子进程：每个 guardian Job 从五个进程排空至零，精确重试保留未知结果，变更源码重试被拒绝，新原生任务准入成功。最终收集核对 136 份启动/资源原件、12 份观察器原件、18 个已关闭资源、18 个已完成副作用及空 scratch。Python 系统重启恢复和更广泛对抗验收尚待完成。生产执行仍禁用。固定解释器选项遵循 [CPython 命令行契约](https://docs.python.org/3.13/using/cmdline.html)；包中已提供隔离的 `._pth` 布局及依赖。
