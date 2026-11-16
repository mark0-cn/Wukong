<a id="execution-worker"></a>

# 执行 worker

[English](README.md) · 简体中文

Docker 容器使用 `--init`（虚拟机使用其 init）回收孤儿后代，包括 Ghidra 辅助进程和脱离客户端的 worker 守护进程。工作台自动添加该选项，并拒绝复用缺少 init 的旧容器；先结束活动任务，主工具容器通过 `env update --image <image-or-ID>` 重建，题目工具容器通过 `challenge down <id>` 后接 `challenge up <id>` 重建。题目重建保留文件和命名工具数据卷；仅运行 `env update` 不会重建已有的题目工具容器。`ProcessResources` 统一拥有执行与会话的进程组，在成功、超时、取消和异常时收尾。守护进程等待持久作业监督进程退出，客户端断开不会取消作业。关闭时拒绝并发请求或未终止的作业，关闭会话并等待监督进程退出。资源注册表不调用 `waitpid(-1)`，也不做全局进程清理。

本指南的 Python 进程管理适用于 Linux Worker 和可信开发夹具。托管 Windows 由原生 Worker 统一管理身份、能力、租约、上传和执行；Python 仅按需运行受隔离的分析代码。当前职责和容量限制见[Windows 组件指南](../windows/README.zh-CN.md)。

Rust 控制端默认不运行待分析目标。请选择持久化 Docker/VM/SSH worker。`trusted_local` 显式在宿主运行可信夹具，仅用于开发测试。Python bridge 管理进程，容器或 VM 提供隔离。

对于 Linux worker 和可信开发夹具，在 worker 使用的 Python 环境（可信本地测试为 `environments.<id>.runtime.python`）安装依赖：

```sh
python -m pip install -e workers/analysis -r workers/runtime/requirements.txt
```

托管 Windows VM 必须使用包含 Python 和全部依赖的完整离线包；不得在 VM 内下载软件包或运行在线 pip 安装。[工具包约定](../package/README.zh-CN.md)和 [Windows 组件](../windows/README.zh-CN.md)说明实现及按源码身份记录的安装/生产验收边界。

Bridge 启动时会在导入 worker 模块前禁用 Python 字节码写入，新建 daemon 和作业监督器进程也遵循该设置，从而保持已签名部署文件不变；可写状态保存在配置的状态目录。不可变部署中的其他 Python 入口也应使用 `-B` 启动。

元数据发布使用 `atomicwrites`：完整 UTF-8 JSON 在替换前刷盘，写入失败时清理临时文件。Windows 写穿移动和 POSIX 父目录同步由库处理。仅对 Windows 临时访问/共享冲突重试，最多五次；保留记录失败不得授权执行。已有操作记录损坏仍报错，绝不静默重置。Linux bridge 依赖固定在 `requirements.txt`；原生 Windows 元数据不使用这套 Python 实现。故障注入检查不证明断电或当前源码的 VM 重启验收通过。

作业准入在预留资源前形成有类型的内部计划。网络、LaunchSpec、重放和进程校验规范化参数的私有副本，并计算原有请求指纹；作业管理器仍用同一把锁覆盖重复请求查询、预留、UID 分配和监督器创建。启动失败保留预留记录供对账，绝不授权自动重启。传输 schema 和状态格式仍由共享协议及既有账本管理。

子进程环境策略、RPC 错误分类和原子元数据 IO 各有独立归属，调用方直接导入；数值/地址校验来自 `wukong_analysis.validation`，协议定义来自生成的契约。制品帮助函数保留路径约束、哈希和有界读取。元数据重试、错误分类及子进程环境过滤保持原有行为。

会话恢复在 stderr 报告无法读取的 JSON 或 IO 错误，每类会话最多输出 16 条具体警告，再汇总其余失败数量。诊断仅含类别、可识别的会话 ID 和异常类型，不含记录内容或任意路径。损坏记录保持原样，原运行中会话在元数据可写时标为 lost，缺少必需状态字段仍使启动失败。RPC stdout 继续仅用于协议响应。

Docker 镜像自动安装依赖。GDB/MI 结果含解析后的 `payload`；异步事件是 type、message、payload、原始 `record` 的结构化记录。原始 MI transcript 保留为证据。

隔离 shell 目标、Python 目标和进程输入共用执行副本准备路径。`py_run.target` 作为只读、可执行的副本追加到参数，无需 chmod。原始 `job_start` 可用 `executable_inputs` 指定最多十六个已暂存的 argv 文件，其余文件参数保持只读数据。LaunchSpec 继续决定可执行文件、cwd 和输入方式。结果公开执行 cwd 以及各准备文件的源哈希、执行哈希和访问权限；复制哈希不一致时准备失败，绝不修改原始附件权限。每次隔离调用都有新的可写工作目录，应显式提取产物，不能假定下一次调用能看到相对路径文件。

作业生命周期写入统一经过 `JobLedger`，使用每个作业的进程间锁。父进程登记 supervisor 创建身份后，子进程才能认领执行令牌；查询和迟到的 supervisor 不能覆盖终态。缺少 PID 或旧版活跃记录视为不确定结果，必须恢复协调，禁止自动重放。Python worker 只能在空闲时升级。快照导入撤销执行权限并保留终态结果。账本令牌、登记握手和锁文件属于内部元数据，不进入 RPC 状态和产物清单。

登记持久化成功后，父进程才通过子进程继承的无缓冲 stdin 管道发送一个字节的确认，放行 supervisor。仅看到登记记录不足以授权执行：父进程退出或登记失败时，管道在未确认的情况下关闭，即使撤销记录的发布也失败，子进程仍不得执行。收到确认后，supervisor 仍须在账本锁内认领令牌。

## Docker

在仓库根目录执行（PowerShell）：

```powershell
python -m scripts.toolchain.fetch_worker_deps
$docker = 'C:\Program Files\Docker\Docker\resources\bin\docker.exe'
$env:PATH = (Split-Path $docker) + ';' + $env:PATH
& $docker build -f workers/Dockerfile -t wukong-worker:0.1 .
& $docker volume create wukong-worker-state
& $docker run -d --init --name wukong-worker --network none --cap-drop ALL `
  --cap-add CHOWN --cap-add SETUID --cap-add SETGID --cap-add KILL --cap-add SYS_PTRACE --cap-add DAC_OVERRIDE `
  --security-opt no-new-privileges --security-opt seccomp=unconfined `
  --pids-limit 256 --memory 4g --cpus 2 --read-only --tmpfs /tmp:rw,size=1g,exec `
  --mount type=volume,source=wukong-worker-state,target=/state wukong-worker:0.1
```

保持容器运行以保留会话。控制端通过 `docker exec` stdio 通信，不发布网络端口。文件上传到命名卷，不挂载宿主文件系统或模型凭据。内核实验使用更强 VM 边界；不要挂载 Docker socket。

Dockerfile 按 digest 固定 AFL++ 基础镜像，安装 GDB、GCC、Python、带 QEMU 插桩的 AFL++、pwntools/z3-solver/angr/ropper、校验 SHA256 的 Temurin JDK 25 LTS（Ghidra 12.2 所需）及校验 SHA256 的官方 Ghidra 12.2。`--build-arg INSTALL_GHIDRA=0` 可省略 Ghidra。审查 `docker image inspect` 后将 `AFL_IMAGE` 设为上游 digest 以固定基础镜像；完整可重复性仍受未完全锁定的传递依赖限制。

在应用配置中登记命名环境：

```toml
default_environment = "linux"
[environments.linux]
name = "Linux analysis"
platform = "linux"
[environments.linux.runtime]
backend = "docker"
docker_container = "wukong-worker"
# 可选：Windows Docker Desktop 自动探测，否则使用 "docker"。
# docker_executable = 'C:\Program Files\Docker\Docker\resources\bin\docker.exe'
remote_bridge_path = "/opt/wukong/runtime/bridge.py"
remote_state_dir = "/state"
request_timeout_secs = 300
```

<a id="ssh-or-a-vm"></a>

## SSH 或 VM

在隔离 Linux guest 安装 Python 3.12+、GDB、带 `afl-qemu-trace` 或 `afl-qemu-bridge` 的 AFL++。在该 Python 环境安装 `workers/analysis`，将本目录复制到 `/opt/wukong/runtime`，生成的 `workers/protocol/schema` 复制到 `/opt/wukong/protocol/schema`，并配置专用管理账号。生产模式需要 root daemon，为每个 job/session 分配不同非特权 UID 并保护监督元数据。SSH 传输不自动添加 sudo；使用配置好的隔离 guest 账号。密钥 SSH 应预先可用，传输使用 `BatchMode=yes`。

```toml
default_environment = "linux"
[environments.linux]
name = "Linux VM"
platform = "linux"
[environments.linux.runtime]
backend = "ssh"
ssh_host = "root@analysis-vm"
remote_bridge_path = "/opt/wukong/runtime/bridge.py"
remote_state_dir = "/state/wukong"
```

子进程环境移除模型凭据；job 环境覆盖只允许 AFL/QEMU/sanitizer 设置。不互信用户不能共享状态目录。

执行、重放、调试器及启动描述的输入使用公开产物路径策略。隔离文件/目录暂存不能把传输端点、job 描述或其他 worker 私有元数据作为程序输入发布。`files/` 下的上传载荷可以使用 `status.json` 等文件名；路径约束在解析别名后检查。目录暂存会在复制任何内容前校验全部成员。

内部启动解析器仅在核对固定缓存路径、归属/权限及精确执行文件哈希后，才能暂存状态目录之外的准备运行时派生文件。该例外不是 RPC 选项，也不能授权元数据输入；原始目标与实际执行目标的身份仍分别记录。

隔离 GDB 会话的原始 MI 命令须先通过按操作定义的参数语法校验，才会进入具有较高权限的调试器。表达式支持简单寄存器/符号、解引用、常量下标及算术/位运算/比较运算符。函数调用（包括调试器侧便利函数）、括号/类型转换、赋值和自增自减均被拒绝。MI 字符串使用受支持的 JSON 转义并重新序列化；八进制/十六进制转义及未校验选项被拒绝。转储检查除了只读操作白名单，也执行同样的参数检查。结构化请求优先使用 `debug.inspect` / `debug.control`；这项策略不代表 GDB 本身是安全沙箱。

生产 Linux 元数据归可信 daemon 所有；目标/fuzz 清除附加组并使用独立 UID。每次实验有只读暂存文件及独立可写目录。GDB 保持可信身份，以实验 UID 启动 inferior。CHOWN/SETUID/SETGID 用于隔离；KILL 允许可信 supervisor 取消实验 UID 持有的进程组；SYS_PTRACE/DAC_OVERRIDE 用于可信调试器经 procfs 读取/修改 inferior；目标进程不保留这些 capabilities。只读容器根、私有卷、禁网及 VM/容器边界分别生效。`trusted_local` 明确标记较弱的可信夹具边界。

<a id="protocol"></a>

## 协议

JSON-RPC 2.0 每个 stdio 行一个对象。错误指出不可用工具；`doctor` 返回真实能力、路径、版本，不宣称缺失后端可用。请求/响应最大 48 MiB；单帧上传最大 32 MiB，可恢复传输支持最大 16 GiB 文件，每块最多 1 MiB。参见[分析环境](../../docs/design/cross-platform-environments.zh-CN.md)。控制端使用 jsonrpsee 有界 stdio；RPC 错误保留健康连接，格式/大小错误及超时关闭连接。下次重连不自动重放失败请求。

由中继管理的守护进程使用已有 `filelock` 依赖和内核持有的 `startup.guard` 锁，持锁直到就绪；竞争中继取得锁后再次检查端点。守护进程自身在初始化及服务全程独立持有 `daemon.guard`，启动者崩溃不会允许第二个守护进程竞争状态。进程退出即可释放锁归属，不再通过目录时间戳删除所谓过期锁。共享状态目录的所有中继必须使用同一 worker 部署。守护进程允许 30 秒完成初始化，包含 Windows 启动身份查询。就绪探测另设 1 秒响应超时；子进程若在就绪前退出，错误会指出退出状态，并包含本地 `daemon.log` 最后最多 2048 字节。

| 方法 | 参数 / 结果 |
| --- | --- |
| `doctor` | 平台、隔离标签、core handler、能力/工具路径 |
| `file.put` | `path`, `data_base64`, `executable`；不可变 `files/<path>` |
| `file.get` | `path`、可选 `max_bytes`；base64 与 SHA256 |
| `job.start` | `argv`、可选 `cwd`, `timeout_secs`, `output_limit`, `env`；返回 `job_id` |
| `job.status`, `job.list` | 持久化 job 元数据及有界输出摘要 |
| `job.cancel`, `fuzz.stop` | `job_id`；取消进程树 |
| `job.artifacts` | `job_id`；内容哈希与 worker 相对路径 |
| `fuzz.start` | `target`, `seeds`, `args`, `timeout_secs`, `exec_timeout_ms`, `memory_mb`；AFL++ `-Q` job |
| `fuzz.status` | job 状态、解析的 `fuzzer_stats`、哈希崩溃产物 |
| `debug.start` | `target`, `args`；worker 所属 `session_id` |
| `debug.command` | `session_id`、一条 GDB/MI `command`、可选 `timeout_secs` |
| `debug.events`, `debug.close` | `session_id`；有界异步记录 / 关闭 |
| `interact.start` | `argv`、可选 `env`；持久化交互进程与 `session_id` |
| `interact.write` | `session_id`, `data_base64`、可选 `read_timeout_secs`；写 stdin 并返回新增有界输出。主进程退出后，在本次读取期限内等待 stdout EOF；管道尾部输出未收完时仍可返回 `timed_out=true`，`data_sent=false` 表示未尝试新的写入。 |
| `interact.close` | `session_id`；终止会话进程树 |
| `angr.solve` | `target`, `find`、可选 `avoid`/`stdin_length`/`max_steps`/`timeout_secs`/`printable_only`；有界符号执行 job，结果含 `input_base64` 和探索统计 |
| `crash.replay` | `target`, `input`, `args`（`@@` 替换输入）、timeout；返回 job |
| `input.minimize` | 重放参数、`max_attempts`, `minimize_timeout_secs`；delta debugging job |
| `poc.verify` | 重放参数、`trials`、不同 `producer_agent_id` / `verifier_agent_id`、`call_id`；job |
| `ghidra.query` | `target`, `target_sha256`、类型化 `query`、timeout；结果及哈希产物 |
| `worker.shutdown` | 拒绝活动 job，关闭调试会话并停止 daemon |

符号执行作业通过 `explore(n=max_steps)` 使用 angr 原生 `Explorer`，包括初始状态匹配和避开地址处理。结果记录实际观察到的步数；独立的作业超时也限制执行时间。

修改请求应包含控制端稳定 `request_id`。操作账本先保留再执行，之后记录结果。同 ID 不同参数拒绝；提供 request_id 时 job ID 确定。调试 owner 在命令中途死亡时结果保持未知，不盲目重放。

job 状态为 queued/running/completed/failed/cancelled/timed_out/lost。关闭控制端传输不取消工作。独立 supervisor 持续读取输出、限制每流字节、处理取消并更新 token 绑定心跳。过期后核对进程创建身份（Windows FILETIME 或 Linux `/proc` start time）；仅确认 supervisor 缺失/改变才标 lost，不确定则保留活动状态并记对账说明。不会重启 job；容器/VM 丢失不能恢复 GDB。

调试会话在同一 owner 锁下串行执行带 token 的 MI 命令。完整 transcript 有界并保留产物；MI 超时使会话失效，因为副作用可能未知。

PoC 每次使用新工作目录和只读输入副本。`job.status.result` 保存 passed、attempts、successes、逐次退出/信号、输入/目标 SHA256、生产者/验证者 ID、环境及 crash/behavior 阶段。重复原生崩溃不证明任意读写或代码执行。崩溃缩减默认要求保留原始模块内容及位置签名。显式 `allow_signal_only` 才允许较弱的信号/异常等价性，并标明这一限制；根因仍须独立确认。

输入缩减使用固定版本 Picire 26.11，替代本地手写 delta-debugging 算法。现有回放模块负责每个候选的执行和证据。原始基线占一次尝试；Picire 对已接受候选的再次断言复用该试验。空输入会被显式检查。超时、取消、尝试预算耗尽或候选结果无法确定时，只保留之前已验证的结果，返回 `complete: false` 和 `stop_reason`。意外评估错误使 job 失败。`reducer: "picire"` 标明所用算法。Doctor 仅在 Picire 及其依赖均存在时声明最小化能力。这些依赖属于 Linux Worker，不再装入 Windows 包。Windows 原生通路使用 Rust `fixture3-ddmin` 库和 WinDbg MCP 进行受限试验；本地检查已通过，已安装 VM 验收仍待完成。最小化调用须移除此前被忽略的 `trials`、`expected` 字段。

2026-11-14，Picire 26.11 通过真实隔离 Linux/GDB 验收。静态夹具及在已准入主映像内故障的动态链接夹具，都在 22 次尝试内将 23 字节缩至 `BAD`，并返回 `complete: true` 和 `location_and_frames`。动态集成还通过类型化调试检查、回放及三次验证试验；两种隔离 LaunchSpec 输入用例均通过。验收镜像为 `sha256:0887afcc06e9ae0df50bd2f7c4b560a9faa7accd032c5c72514b2323ef53a2e6`；动态报告 SHA256 为 `96c4426c5690179db45f4173eff98b722366712593cd00a822ee34be19886e38`。另一个 `abort()` 夹具的 PC 属于未准入 libc，正确拒绝了强等价性缩减；模块路径不能证明内容哈希。这些夹具不代表任意库身份或 AFL 模糊测试已通过验收。

`ghidra.query` 与 Rust 使用相同 query：`{"operation":"functions","pattern":"main","offset":0,"limit":100}`、`{"operation":"decompile","address":1234}`、`{"operation":"xrefs","address":1234,"direction":"both","limit":100}`。地址为模块相对。核对上传 SHA256，缓存绑定目标、Ghidra 版本/properties、launcher、bridge、分析设置。项目锁内复用导入程序；清理失主锁前核对进程。较长导入须将控制端请求超时设为大于查询超时，传输最多 300 秒。

<a id="acceptance-checks"></a>

## 验收检查

可移植可信夹具进程测试：

```powershell
python workers/tests/run.py unit -p test_bridge.py -v
cargo test --test runtime_jobs
```

真实 Linux GDB、重放、独立验证与缩减：

```powershell
& $docker exec wukong-worker python3 /opt/wukong/tests/run.py integration --state /state/acceptance --fuzz
```

真实 Ghidra 导入、分析、functions/decompile/xrefs、缓存身份和哈希不符拒绝：

```powershell
& $docker exec wukong-worker python3 /opt/wukong/tests/run.py ghidra --state /state/ghidra-acceptance
```

Ghidra 套件编译自己的 C 夹具，通过只读适配器导入，定位 main/helper，反编译 main（必须含 memcpy 数据流），核对 caller xref、重复查询命中身份缓存、错误哈希拒绝；不执行目标。

运行时套件编译 C 夹具，在 main 断点，GDB/MI 读栈和内存，控制端重连，重放 abort() 输入，验证三次、缩减至 `BAD`；`--fuzz` 运行真实 AFL++ QEMU 并检查统计执行次数。完整记录写 `<state>/acceptance-results.json`，不攻击外部系统。

历史检查（2026-11-06）：Windows 可移植测试 6/6；隔离 Linux 通过 GDB、重连、重放、3/3 验证、缩减至三字节及 AFL++ QEMU（八秒 17,059 次、一个 SIGABRT）。Ghidra 12.2/Temurin 25 通过导入、分析、查询、缓存及错误哈希拒绝。原始记录为 [runtime-linux.json](../../docs/history/validation/2026-11-06/runtime-linux.json) 和 [ghidra-linux.json](../../docs/history/validation/2026-11-06/ghidra-linux.json)；不代表当前提交验收。

该历史环境 Docker Desktop 宿主 core pattern 调用外部 handler，AFL++ 默认拒绝。夹具显式设 `AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES=1` 验证 QEMU/统计而不修改宿主全局设置，并记录覆盖值；不证明完整收集崩溃。需要完整收集时使用 core pattern 合适的专用 Linux VM，或显式决定每个 job 是否使用同一覆盖。

参考：[GDB/MI](https://sourceware.org/gdb/current/onlinedocs/gdb.html/GDB_002fMI.html)、[AFL++ QEMU](https://github.com/AFLplusplus/AFLplusplus/blob/stable/qemu_mode/README.md)、[Ghidra 12.2 与校验和](https://github.com/NationalSecurityAgency/ghidra/releases/tag/Ghidra_12.2_build)。

<a id="ghidra-project-ownership"></a>

## Ghidra 项目归属

控制端使用标准库文件锁，Python worker 使用 filelock。`worker.lock` 是持久锁 inode，分析器可能持有时不能删除。锁覆盖执行、取消/回收及缓存发布。`analysis.active.json` 在 owner 崩溃或不确定执行失败后保留；清除 claim 前检查并对账分析器。没有锁 owner 不代表子进程已退出。

<a id="module-boundaries"></a>

## 模块边界

`bridge.py` 保留 CLI 参数，在导入 `wukong_worker.cli` 前禁用字节码写入。部署整个 `workers/runtime` 目录及相邻生成的协议 schema，无需额外安装运行时包。内部显式导入 `wukong_worker` 包。`core/` 负责 schema 校验、稳定错误、有类型的作业意图及纯完成规则。`rpc/` 负责帧、协议错误转换及可调用注册；`host/` 负责身份、能力观察和服务装配。CLI 向 daemon 传输注入运行时构造。身份哈希覆盖所有嵌套包源码路径/字节、bridge、共享 schema 和分析包。

`operations/` 负责工具专属准入及进程/fuzz/重放/验证/缩减流程。`JobService` 准备 `JobPlan`；`resources/jobs.py` 预留和启动计划，无需了解工具专属参数。`resources/supervisor.py` 负责心跳和最终发布，接收返回 `JobOutcome` 的执行器；执行器不写作业状态。`resources/` 还负责会话、持久副作用及关闭和维护共用的完成清单。

`execution/` 负责有界进程 IO、启动解析、输入暂存和缩减适配。`debugger/` 负责 GDB/MI 和交互会话；`artifacts/` 负责受限路径、哈希、传输及原子元数据。`platform/` 负责环境策略、OS 进程控制、进程所有权/日志、Linux 隔离及可信夹具使用的 Windows 身份/路径辅助。这些原语不导入资源服务或 RPC。`workbench/` 包含宿主调用的辅助程序；control/prepare 通过显式入口引导支持直接脚本执行。注入的分析/收集脚本保持自包含。架构测试约束运行时依赖方向，不计仅用于类型注解的导入。

`analysis/adapters.py` 在同一文件内保留 `SeccompService`、`LibcService` 和 `EmuService` 各自的准入；`debugger/crash.py` 保留真实 GDB 观察。它们将输入适配到独立[分析包](../analysis/README.zh-CN.md)，该包负责 BPF 解码/探测、有界 ELF 候选比较、Unicorn 模拟与崩溃数据解释，不拥有 Worker 状态或传输。`network/service.py` 校验准备好的服务绑定并执行声明式 TCP 验证。监督器仍拥有生命周期/取消；不会在特权 supervisor 内运行模型提供的程序。见 [Pwn 工具契约](../../docs/reference/pwn-tools.zh-CN.md)。

`execution/launch_spec.py` 管公共解析、受限成员查找、流式产物哈希。原生启动和网络重放各自保留执行策略/预算。`network/service.py` 协调试验；`network/plan.py` 校验步骤/捕获；`network/transport.py` 管有界 TCP 与新响应边界；`network/evidence.py` 保存精确收发块及哈希 transcript。取消仍由 supervisor 管理。

## 资源所有权

| 资源 | 所有者与生命周期 |
| --- | --- |
| 守护进程与端点 | `rpc/transport.py` 持有独立于 stdio 转发进程的守护锁；重连不会重新启动已有归属的作业。 |
| 作业预留与回执 | `resources/jobs.py` 串行执行去重、UID/目录预留和监督器启动；`operations/planning.py` 在加锁前准备规范化输入。失败或未知的启动仍保留持久身份。 |
| 作业进程树与输出 | 监督器负责取消和最终发布；`platform/process_owner.py` 负责进程组/Job 终止，`execution/processes.py` 收集有界输出。主进程死亡不能证明后代已退出。 |
| GDB 与交互会话 | `resources/sessions.py` 保留活动所有者；会话类串行处理操作并保存观察。恢复将旧会话标为 lost，报告无法读取的元数据，但不修复原记录。 |
| 网络试验 | `network/service.py` 持有 socket 上下文并最终发布 transcript；传输层检查取消/期限，保留旧响应与新响应的边界。 |
| 输入缩减 | `execution/minimize.py` 管尝试次数和期限；注入的求值器负责每次执行及崩溃等价性。中断时保留的候选不代表已经最小化。 |
| 实验文件 | `platform/isolation.py` 在独立 UID 下准备已校验的公开输入，通过同一受限身份收集制品。执行结束后仍保留存储证据。 |

管理命令 `doctor` 在临时状态上绑定真实注册表，再观察能力。它不会在运行中的守护进程状态上构造 Runtime，避免错误恢复活动会话。关闭传输、取消执行和删除保留证据是由不同所有者负责的不同操作。

会话清理在调用 worker 前持久化关闭意图。Linux 进程管理器记录启动与命名空间身份、进程创建身份、进程组和自有临时目录；即使会话记录尚未发布就中断，重启恢复也会扫描这些日志。worker 中查不到会话不等于清理成功：无法确认归属时保留 `recovery_required`，关闭中或待恢复的会话、以及带 `cleanup_pending` 的终态作业，都会阻止任务完成和环境维护。控制器重试关闭中的会话及已失败或停止的所有者资源，不重放调试命令。Windows 继续使用原生 ResourceManager。PID 身份或命名空间不匹配时需要恢复所属环境，不向无关进程发信号。导入的日志不赋予目标环境的进程归属权。

关闭实时会话后，worker 关停与环境维护检查同一份持久资源清单。恢复中或尚未发布的会话若未确认清理，即使没有内存会话对象，也会阻止关停。生命周期观察将执行结果的确定性与清理确认分开。
