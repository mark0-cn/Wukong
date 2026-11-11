<a id="execution-worker"></a>

# 执行 worker

[English](README.md) · 简体中文

Rust 控制端默认不运行待分析目标。请选择持久化 Docker/VM/SSH worker。`trusted_local` 显式在宿主运行可信夹具，仅用于开发测试。Python bridge 管理进程，容器或 VM 提供隔离。

对于 Linux worker 和可信开发夹具，在 worker 使用的 Python 环境（可信本地测试为 `environments.<id>.runtime.python`）安装依赖：

```sh
python -m pip install -r workers/runtime/requirements.txt
```

托管 Windows VM 必须使用包含 Python 和全部依赖的完整离线包；不得在 VM 内下载软件包或运行在线 pip 安装。[工具包约定](../package/README.zh-CN.md)和 [Windows 组件](../windows/README.zh-CN.md)说明当前实现及尚未完成的安装器/生产验收工作。

Bridge 启动时会在导入 worker 模块前禁用 Python 字节码写入，新建 daemon 和作业监督器进程也遵循该设置，从而保持已签名部署文件不变；可写状态保存在配置的状态目录。不可变部署中的其他 Python 入口也应使用 `-B` 启动。

Docker 镜像自动安装依赖。GDB/MI 结果含解析后的 `payload`；异步事件是 type、message、payload、原始 `record` 的结构化记录。原始 MI transcript 保留为证据。

## Docker

在仓库根目录执行（PowerShell）：

```powershell
python -m scripts.toolchain.fetch_worker_deps
$docker = 'C:\Program Files\Docker\Docker\resources\bin\docker.exe'
$env:PATH = (Split-Path $docker) + ';' + $env:PATH
& $docker build -f workers/Dockerfile -t wukong-worker:0.1 .
& $docker volume create wukong-worker-state
& $docker run -d --name wukong-worker --network none --cap-drop ALL `
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

在隔离 Linux guest 安装 Python 3.12+、GDB、带 `afl-qemu-trace` 或 `afl-qemu-bridge` 的 AFL++。将整个目录复制到 `/opt/wukong/runtime`，配置专用管理账号。生产模式需要 root daemon，为每个 job/session 分配不同非特权 UID 并保护监督元数据。SSH 传输不自动添加 sudo；使用配置好的隔离 guest 账号。密钥 SSH 应预先可用，传输使用 `BatchMode=yes`。

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

隔离 GDB 会话的原始 MI 命令须先通过按操作定义的参数语法校验，才会进入具有较高权限的调试器。表达式支持简单寄存器/符号、解引用、常量下标及算术/位运算/比较运算符。函数调用（包括调试器侧便利函数）、括号/类型转换、赋值和自增自减均被拒绝。MI 字符串使用受支持的 JSON 转义并重新序列化；八进制/十六进制转义及未校验选项被拒绝。转储检查除了只读操作白名单，也执行同样的参数检查。结构化请求优先使用 `debug.inspect` / `debug.control`；这项策略不代表 GDB 本身是安全沙箱。

生产 Linux 元数据归可信 daemon 所有；目标/fuzz 清除附加组并使用独立 UID。每次实验有只读暂存文件及独立可写目录。GDB 保持可信身份，以实验 UID 启动 inferior。CHOWN/SETUID/SETGID 用于隔离；KILL 允许可信 supervisor 取消实验 UID 持有的进程组；SYS_PTRACE/DAC_OVERRIDE 用于可信调试器经 procfs 读取/修改 inferior；目标进程不保留这些 capabilities。只读容器根、私有卷、禁网及 VM/容器边界分别生效。`trusted_local` 明确标记较弱的可信夹具边界。

<a id="protocol"></a>

## 协议

JSON-RPC 2.0 每个 stdio 行一个对象。错误指出不可用工具；`doctor` 返回真实能力、路径、版本，不宣称缺失后端可用。请求/响应最大 48 MiB；单帧上传最大 32 MiB，可恢复传输支持最大 16 GiB 文件，每块最多 1 MiB。参见[分析环境](../../docs/design/cross-platform-environments.zh-CN.md)。控制端使用 jsonrpsee 有界 stdio；RPC 错误保留健康连接，格式/大小错误及超时关闭连接。下次重连不自动重放失败请求。

由中继管理的守护进程允许 30 秒完成初始化，包含 Windows 启动身份查询。就绪探测另设 1 秒响应超时；子进程若在就绪前退出，错误会指出退出状态及本地 `daemon.log` 位置。

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
| `interact.write` | `session_id`, `data_base64`、可选 `read_timeout_secs`；写 stdin 并返回新增有界输出 |
| `interact.close` | `session_id`；终止会话进程树 |
| `angr.solve` | `target`, `find`、可选 `avoid`/`stdin_length`/`max_steps`/`timeout_secs`/`printable_only`；有界符号执行 job，结果含 `input_base64` 和探索统计 |
| `crash.replay` | `target`, `input`, `args`（`@@` 替换输入）、timeout；返回 job |
| `input.minimize` | 重放参数、`max_attempts`, `minimize_timeout_secs`；delta debugging job |
| `poc.verify` | 重放参数、`trials`、不同 `producer_agent_id` / `verifier_agent_id`、`call_id`；job |
| `ghidra.query` | `target`, `target_sha256`、类型化 `query`、timeout；结果及哈希产物 |
| `worker.shutdown` | 拒绝活动 job，关闭调试会话并停止 daemon |

修改请求应包含控制端稳定 `request_id`。操作账本先保留再执行，之后记录结果。同 ID 不同参数拒绝；提供 request_id 时 job ID 确定。调试 owner 在命令中途死亡时结果保持未知，不盲目重放。

job 状态为 queued/running/completed/failed/cancelled/timed_out/lost。关闭控制端传输不取消工作。独立 supervisor 持续读取输出、限制每流字节、处理取消并更新 token 绑定心跳。过期后核对进程创建身份（Windows FILETIME 或 Linux `/proc` start time）；仅确认 supervisor 缺失/改变才标 lost，不确定则保留活动状态并记对账说明。不会重启 job；容器/VM 丢失不能恢复 GDB。

调试会话在同一 owner 锁下串行执行带 token 的 MI 命令。完整 transcript 有界并保留产物；MI 超时使会话失效，因为副作用可能未知。

PoC 每次使用新工作目录和只读输入副本。`job.status.result` 保存 passed、attempts、successes、逐次退出/信号、输入/目标 SHA256、生产者/验证者 ID、环境及 crash/behavior 阶段。重复原生崩溃不证明任意读写或代码执行。缩减保留信号/异常签名；同信号不同漏洞仍须独立确认根因。

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

`bridge.py` 只选择 stdio relay、daemon 或独立监督；部署整个 runtime 目录，Docker 已整体复制。`worker_transport` 管认证帧与启动；`worker_runtime` 装配/路由；`worker_operations` 管持久化保留与重放，`worker_sessions` 提供会话存活；`worker_jobs` 管记录/取消，`worker_job_supervisor` 管独立执行。调试、交互、文件、同步命令独立组件。`worker_processes` 管有界 I/O/进程树；`worker_common` 管协议值、路径约束、流式哈希和原子元数据。后端直接导入这些组件。

`worker_seccomp` 管离线 classic-BPF/具体探测；`worker_libc` 静态比较有界 ELF 候选；`worker_network` 校验准备好的服务绑定并执行声明式 TCP 验证。监督器仍拥有生命周期/取消；不会在特权 supervisor 内运行模型提供的程序。见 [Pwn 工具契约](../../docs/reference/pwn-tools.zh-CN.md)。

`worker_launch_spec` 管公共解析、受限成员查找、流式产物哈希。原生启动和网络重放各自保留执行策略/预算。`worker_network` 协调试验；`worker_network_plan` 校验步骤/捕获；`worker_network_transport` 管有界 TCP 与新响应边界；`worker_network_evidence` 保存精确收发块及哈希 transcript。取消仍由 supervisor 管理。
