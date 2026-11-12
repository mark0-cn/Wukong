# Windows 原生 worker 组件

[English](README.md) · 简体中文

独立 Cargo 包构建 worker 组件，不链接桌面控制器、WebView 或模型客户端。`native/dbgeng.rs` 与控制器仓库的开发辅助程序共享。组件包版本独立于应用发布版本；交付的 worker 身份还绑定实际源码及二进制内容与运行时协议。

在仓库根目录的 x64 MSVC Native Tools shell 中运行：

```powershell
python -m scripts.worker.build_native --check
python -m scripts.worker.build_native
cargo fmt --manifest-path workers/windows/Cargo.toml -- --check
```

构建入口使用已提交的独立锁文件，并显式设置静态 CRT。默认输出位于 `target/windows-worker-native`。写入 `native-build.json` 前检查普通和延迟加载的 PE 导入，拒绝依赖外部 Visual C++/Universal CRT 库。该检查适用于引导程序自身；离线工具包仍须单独包含第三方工具的全部依赖及原始安装载荷。构建不会执行安装、服务注册或分析目标。

实现依据 [Rust CRT 链接约定](https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes)与微软 [PE 格式](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format)。Windows CI 对独立 crate 进行格式与代码检查、构建引导程序并验证导入；自动化测试还拒绝动态 CRT、非法库名与格式错误的普通或延迟导入表。

原生库可将已验证载荷暂存到新建目录，显式设置受保护的 Windows ACL。源文件须为单链接普通文件，打开时不跟随末级重解析点，也不共享写入或删除权限；在创建可读目标文件前，先通过持有的句柄核对签名清单中的大小及哈希，随后对复制内容计算哈希，并再次校验完整目标目录。清单与公钥字节来自已验证的内存身份；在暂存前，通过持有句柄有界读取的签名须与该清单匹配。VM 策略区分 System/管理员元数据与可读的可执行内容。源介质和暂存父目录仍须防止其他进程并发写入；这些检查不提供从目标可写目录进行通用提权收集的能力。失败暂存仍由调用方持有，用于事务恢复。该模块不激活版本，也不创建服务或账号。普通 Windows CI 测试仅使用无执行能力的签名文件及仅当前用户可访问的临时目录，验证真实共享冲突、重命名拒绝、硬链接拒绝、权限，以及源文件变更、相对根目录和覆盖写入的拒绝路径，不在开发宿主安装任何软件。

显式只读介质夹具可以将签封载荷复制到新临时目录并再次校验，不执行载荷，也不注册系统资源。仅以只读方式挂载指定 ISO，并独立选择其公钥；替换下列占位值。测试自身不挂载或卸载介质，也不执行 VM 安装验收。

```powershell
$env:WUKONG_STAGE_TEST_SOURCE = 'X:\payload'
$env:WUKONG_STAGE_TEST_PUBLIC_KEY = 'PINNED_CONTROLLER_PUBLIC_KEY_HEX'
$env:WUKONG_STAGE_TEST_CONTROLLER_VERSION = 'CONTROLLER_VERSION'
cargo test --locked --manifest-path workers/windows/Cargo.toml --lib stages_explicit_read_only_offline_media_without_execution -- --ignored
```

`wukong-worker.exe verify-package DIRECTORY PINNED_PUBLIC_KEY_HEX CONTROLLER_VERSION` 使用共享[离线包约定](../package/README.zh-CN.md)。该只读命令校验独立信任、兼容性与文件精确内容。`verify-enrollment PACKAGE ENROLLMENT_DIRECTORY PINNED_PUBLIC_KEY_HEX CONTROLLER_VERSION` 还会校验公开登记签名及其载荷绑定。两个校验命令均不安装或激活 worker。该程序与 `wukong-dbgeng.exe` 一起接受静态 CRT 检查。

`toolchain.lock.json` 以大小与 SHA256 固定官方 Python、OpenSSH、Python wheel 和完整 Debugging Tools MSI/CAB 清单。锁文件中的 OpenSSH 版本为上游**预览版**。在控制端准备缓存：

```powershell
python -m scripts.worker.toolchain --cache target/windows-offline-cache --download
python -m scripts.worker.toolchain --cache target/windows-offline-cache
python -m scripts.worker.layout --cache target/windows-offline-cache --output target/windows-offline-layout
```

只有 `--download` 允许网络访问。已有缓存条目不匹配时直接失败，不替换原文件。展开要求 Windows 控制端和全新输出目录，没有联网或下载路径。它以只读方式查询 MSI 表，仅解压回调批准的 CAB 成员，将完整调试器载荷清单与已固定的微软引导程序内部元数据核对，并保留全部原始 MSI/CAB/ZIP/wheel 及许可证。不会运行 MSI 安装、自定义操作或修改已安装工具。解压使用微软 [MSI 数据库](https://learn.microsoft.com/en-us/windows/win32/api/msiquery/nf-msiquery-msiviewfetch)及 [cabinet 回调](https://learn.microsoft.com/en-us/windows/win32/setupapi/spfilenotify-fileincabinet) API。

输出包含已预展开 wheel 的私有嵌入式 Python、Python worker 源码、OpenSSH 和完整 SDK 调试器文件树。Python 忽略用户和注册表路径，仅加载随包库及 worker 目录；VM 无须 pip。归档路径拒绝链接、目录穿越、别名与覆盖写入。缓存和暂存目录应仅供控制端访问：这些检查本身不提供并发写入隔离。失败输出保留用于诊断，不能复用。该命令生成**未签名的控制端展开目录**。普通控制端展开检查与可信样例不代表干净 Windows VM 的依赖覆盖。

组装并签名新的完整载荷前，先提交 worker 改动，再从该干净提交重新构建。组装器要求源码提交与目录哈希匹配，并核对已锁定的原生构建元数据、实际二进制哈希和静态 CRT 导入。包内包含原生二进制及其依赖许可文本、Rust 标准库版权声明和全部原始第三方载荷。不会下载缺失文件。

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

原生调用方须先建立工具包信任，并防止其他进程写入版本目录。该入口不是 worker RPC，也不替代签名校验。它在执行前检查随包探针哈希，无须网络即可测试 OpenSSH 密钥生成，并使用随包运行时检查 x64/x86 dump/live 调试及每种架构三次重放。固定 SSH 探针会额外接收所需的 Windows `ProgramData` 路径，不改变目标作业的环境变量过滤。临时测试 SSH 密钥会删除，日志及结果保存在签名目录之外。成功报告明确仅涵盖离线依赖和可信样例，不代表生产目标隔离或服务/SSH 安装。运行后应再次校验签名目录。

调试组件为结构化 DbgEng 辅助程序。真实 x64/x86 可信样例验收独立于生产 broker 隔离与离线 VM 安装验收。样例套件还检查重放：使用不同的生产者/验证者 ID，每种架构运行三次新进程与工作目录试验，校验输入及目标的精确哈希，并观测 `0xc0000005` 异常。这证明可信样例重放路径，不代表生产元数据保护或利用影响。参见[跨平台环境约定](../../docs/design/cross-platform-environments.zh-CN.md)和[运行时指南](../runtime/README.zh-CN.md)。

打开转储或附加进程后，辅助程序先核对引擎实际报告的调试对象类别，再允许检查。仅接受 Windows 用户态会话及用户态小型/完整转储。内核及其他类别会被拒绝；任何打开失败都会令辅助程序失效并关闭，避免继续使用部分打开的状态。样例套件为 x64/x86 记录实际观察到的用户态类别及限定值；当前验收仍不含真实内核转储拒绝样例。

原生 VM 引导程序与 SCM 服务已有可调用的实现路径。从已签名完整载荷和匹配的签名登记生成新的公开安装介质：

```powershell
python -m scripts.worker.bundle --package ABSOLUTE_PACKAGE --enrollment ABSOLUTE_PUBLIC_ENROLLMENT --output NEW_ABSOLUTE_MEDIA --public-key PINNED_CONTROLLER_PUBLIC_KEY_HEX --controller-version CONTROLLER_VERSION --verifier ABSOLUTE_NATIVE_WORKER_EXE
```

介质包含 `WukongWorkerSetup.exe`、`setup.json`、`payload/` 和两个公开登记记录，不包含控制端私钥。应传入独立 Windows 测试 VM，绝不能安装到开发宿主上。安装界面展示版本、安装身份、允许的控制端地址、SSH 端口及签名指纹。首次由所有者建立信任时，必须通过独立通道与控制端比对指纹。提权后再次核对用户刚确认的精确登记与完整载荷，随后才创建系统资源。当前原生引导界面为英文。生成介质不代表安装验收完成。

已在独立测试环境取得管理员权限的自动化可以运行 `WukongWorkerSetup.exe install-reviewed ABSOLUTE_SETUP_JSON PINNED_CONTROLLER_PUBLIC_KEY_HEX REVIEWED_ENROLLMENT_SHA256`。参数须提供独立固定的控制端公钥，以及在控制端审阅过的精确规范 `enrollment.json` 的 SHA256。该命令采用相同的校验和安装路径，返回 JSON 凭据，不显示对话框，也不会自行提权。它是操作员入口，不是 worker RPC；不能用于覆盖开发宿主上的安装。

安装成功后，仅将公开的 `connection.json` 复制回控制端。在“工作区设置 → 分析环境 → 登记 Windows 虚拟机”中导入记录、输入虚拟机地址、选择已有控制端私钥，并独立输入安装界面显示的主机指纹。预览校验已安装记录的结构、Ed25519 公钥与指纹、安装账户及已有密钥文件路径，不读取私钥内容、不创建文件，也不连接虚拟机。重复环境 ID 会被拒绝。加入草稿保留其他配置修改及注释。保存时在控制端设置目录创建以内容命名、以 LF 结尾的公开主机固定文件；已有内容不同则拒绝覆盖。清理仅登记该精确公开文件；所选私钥保持为外部文件。未完成的连接在关闭设置后保留，保存前须先加入草稿或重置。首个环境自动成为默认环境。登记独立于连通性及生产隔离检查；桌面介质准备与完整 VM 生命周期验收尚未完成。

创建服务前，固定的主机密钥生成及 SSH 配置检查将诊断保存在仅 System/管理员可访问的 `metadata/install-tool-ssh-keygen.log` 和 `metadata/install-tool-sshd.log`。错误会标明失败的程序；SSH 语法检查输出详细诊断，但不监听连接。托管夹具仅将这两个固定日志各自最后 4 KiB 纳入公开报告，不包含私钥或其他元数据。

提权安装工具与 System SSH daemon 使用固定的七变量 Windows 环境：`SYSTEMROOT`、`WINDIR`、`PROGRAMDATA`、`COMSPEC`、`PATH`、`TEMP` 和 `TMP`。路径从 Windows 位置派生，程序查找仅限 Windows 目录和 System32。其临时目录为受保护的 `metadata/system-tmp`，在启动及激活前验证。普通 worker 继续使用独立的 `data/tmp`；不会将控制端/provider 的整个环境复制给提权子进程。

若 SSH 语法检查发生访问违例，独立托管夹具还会比较最小环境与固定的 Windows 运行环境，再使用签封 SDK 中的 CDB，仅检查出错指令指针、不含参数的调用栈和模块身份。探测保留已有程序网络阻断规则，并使用空的本地符号目录，不创建内存转储。报告不包含完整寄存器显示、私钥字节或指定调试区段外的输出；有界诊断输出不代表安装成功。

安装创建独立的管理、元数据 worker、调试器和目标账户，在 Windows ProgramData 已知目录下记录归属，并将生成的 worker 密码存入 LSA。所有账户禁止交互式/RDP 登录，仅 broker 持有的账户取得批处理登录权限。服务验证活动版本签名、目录及自身程序后，以普通账户、独立桌面和移除所有枚举特权的令牌启动 Python。显式进程属性确保子进程在执行前进入不允许脱离的 Job，且只继承输入/日志句柄。worker 代次退出时，服务先终止其所有后代进程再停止。首次就绪前必须通过固定依赖自检，并保存仅 System/管理员可写、绑定清单的凭据；其验证范围仍为可信样例。

随包 SSH 使用独立主机密钥、仅公钥认证、登记的控制端地址和忽略请求命令的固定原生中继。转发、TTY 及用户启动钩子均被禁用。原生防火墙规则限定来源、端口和程序。管道 ACL 不授予客户端创建服务端实例的权限；每帧检查实际 Windows 调用者 SID，中继发送数据前将服务端 PID 与 SCM 比对。回环端点令牌留在受保护 worker 状态中。原生身份绑定覆盖完整签名清单。遇到现有安装、服务或账户名时拒绝采用或覆盖；中断记录保留供显式修复。

`ssh/` 目录从创建起就仅允许 System/管理员访问，因此主机密钥生成期间不会在文件权限最终设置前暴露私钥。管理账户可读的公钥认证文件单独放在 `bootstrap/authorized_keys`。激活前和服务启动时，部署验证会检查这些目录及文件的 ACL，覆盖两个生成的主机密钥文件和服务器配置。

防火墙登记将签封程序解析为规范的 Windows 本地路径。失败会标明 COM 属性或登记步骤；登记错误包含公开的程序、端口、来源地址及配置文件范围设置。本地测试在未登记的规则对象上验证真实 COM 属性设置/读取和路径身份，不添加或修改任何宿主防火墙规则。实际登记仍需独立 VM 验收。

SCM 服务登记及固定原生子进程启动采用相同的本地路径规范化，使服务归属比较与已验证程序保持一致。离线 DbgEng 探测失败时，会将有界的 helper 退出/加载原因保存在受保护自检日志中。这些诊断不会放宽普通账户权限，也不会将失败的自检视为就绪。

账户创建按已知 SID 解析内置 Users 别名，并按 SID 将专用账户加入该组，避免依赖本地化组名。早期恢复严格限于载荷、数据与服务激活前中断的管理账户创建。准备新介质时加上 `--previous-package ABSOLUTE_PREVIOUS_PACKAGE --previous-enrollment ABSOLUTE_PREVIOUS_PUBLIC_ENROLLMENT`，即可携带原始公开签名证据。恢复将证据与受保护安装记录比对，要求更高代次的签名登记，并保留已接受的恢复代次。同时核对账户精确归属注释、普通用户权限和 SID，不重设其密码。该路径拒绝其他现有账户和后续安装阶段。首次激活前仍须由所有者核对指纹。

引导程序在创建账户前，将完整的首次所有者签名登记写入仅 System/管理员可写的 `metadata/authority.json`。恢复与服务启动从这份受保护记录取得签名公钥；清单哈希相同也不能授权替换签名身份。已接受的恢复记录在中断后仍保留递增的最高代次，启动时活动登记必须与已接受的最高记录一致。签名变更、过期/重用信封及非规范准入文件名会被拒绝。若中断的开发期引导仅留有原始安装意图记录，仍须提供原始签名证据并由所有者独立比对指纹，才能首次建立该信任记录；此窄路径不会采用已安装的 worker，也不实现通用迁移。失败更新的回滚需要另行授权的激活凭据，仍属于后续生命周期工作。

服务的 `lease_proxy` 持有面向控制端的 run 隔离令牌，并将递增 epoch 日志保存在仅 System/管理员可写的元数据中。它验证就绪时的安装、启动与包身份，在转发前逐次校验令牌，并替换为独立的 Python 租约，不向控制端披露内层令牌。长调用不会阻止续租。仍在执行、被取消或传输结果不明的业务调用阻止释放及过期接管；不确定调用保留为空闲阻塞项，直到 worker 代次被终止。日志支持最多 100,000 个新令牌且不覆盖记录，续租不消耗记录。本地无目标执行夹具检查真实受保护文件、并发写入拒绝、重启持久性、陈旧请求拒绝、延迟续租及取消，使用受控上游适配器，并非已安装的 VM 服务。原生目标资源归属和完整维护空闲屏障仍需要 broker 及 VM 验收。

服务成功停止及离线自检验收前，会显式终止每个所属 Job，并最多等待 10 秒，直到其活动进程数降为零。主进程退出不能证明其后代也已退出。等待失败时不能返回生命周期成功或写入自检凭据；析构清理仅作为错误情况下的尽力回收。worker 和 SSH Job 回收期间，SCM 的 stop-pending 状态保留等待提示和进度检查点。本地可信夹具在主进程退出后保留活跃子进程，并验证 Job 屏障终止该子进程。这项检查验证进程归属，并非已安装服务或目标隔离验收。

当前开发检查覆盖真实本地管道身份及拒绝路径、无执行能力的受保护暂存、签名恢复准入，以及真实后代进程的原生 Job 归属和统一终止。不会在宿主创建账户、服务或防火墙规则。首次离线 VM 尝试在普通账户校验处停止，尚未创建服务或防火墙规则；完整安装仍待验证。SSH 连通、生命周期恢复/更新/回滚，以及目标隔离 broker 仍须完成实现或验收。生产目标执行保持禁用；该引导程序不证明 Windows 生产支持已经完成。

可选的托管安装验收使用 `tests/acceptance/check_windows_worker.py` 及其 PowerShell 辅助脚本。两者都会在系统级副作用前拒绝开发宿主或自托管环境。夹具先作为控制端获取资源，使用仅留在控制端的临时密钥签封介质，再阻断介质和安装路径内随包程序访问外部地址，保留同一测试机已分配地址的管理连接和 CI agent 的连接。它验证真实原生安装器、普通账户自检、Rust 控制端固定 SSH 传输、不可变文件传输、陈旧令牌拒绝，以及服务重启后递增的 epoch。仅上传有界公开报告。清理前核对精确服务路径、账户归属注释、防火墙说明和安装意图，再删除夹具资源；剩余临时 LSA 密钥随一次性 VM 回收。这并非生产卸载、整台 VM 断网、Windows 10 VM 或目标隔离验收。配置流水线不代表实际运行通过，须查看真实报告与最终结果。
