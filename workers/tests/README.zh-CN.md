<a id="worker-tests"></a>

# Worker 测试

[English](README.md) · 简体中文

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
python3 /opt/wukong/tests/run.py ghidra --state /state/ghidra-acceptance
```

测试状态目录应为本轮独占目录；真实后端要求见 [worker 说明](../runtime/README.zh-CN.md)。

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
