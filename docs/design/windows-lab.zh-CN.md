# Windows 实验环境生命周期

[English](windows-lab.md) · 简体中文

## 范围与归属

控制器管理已登记的一次性 VMware VM、校验过的离线包、预编译用户态测试程序和管理安装脚本。宿主路径与凭据来自操作者配置。模型不能提供 VMware 命令、控制器文件路径或客体管理员凭据。该生命周期不负责在 VM 内编译测试程序或生成服务/驱动实现。

实现按类型记录（`types`）、原子持久化（`store`）、宿主生命周期、包执行（`execution`）、包/介质校验和 worker/admin 适配器拆分。工具处理器接收具体服务。评估执行复用普通任务账本，异步宿主操作复用现有持久化副作用账本。

```toml
[windows_lab.vms.lab]
vmx = "D:/vm/win10/Windows 10 x64.vmx"
vmrun = "D:/VMware/VMware Workstation/vmrun.exe"
environment = "windows"
baseline_snapshot = "wukong-lab-baseline"
snapshot_prefix = "wukong-lab"
operation_timeout_seconds = 300
start_after_restore = true

[windows_lab.vms.lab.admin]
user = "Administrator"
password_env = "WUKONG_LAB_ADMIN_PASSWORD"
```

VM 别名包含 1–64 个 ASCII 字母、数字、下划线或连字符。快照名还允许点号，实验前缀必须与基线不同。宿主命令的配置时限为 30–3600 秒。适配器构造固定参数，不经过 shell，并限制返回输出。密码只从指定环境变量读取，不写入状态或回执；可信控制器上的 `vmrun` 必须通过命令行接收密码。

## 状态、身份与资源隔离

Schema-2 状态位于 `<state_dir>/windows-lab/<vm_id>.json`，绑定规范化 VMX 身份与配置指纹、基线、代际、活动实验、待定宿主操作和执行回执。原子替换在发布失败时保留旧状态。物理 VM 身份拥有操作锁和别名登记；重复别名及共享环境映射会被拒绝。修改 VMX、worker 环境或其他绑定配置不能复用原租约。

评估 worker 绑定在规范化 VMX 旁持有共享锁（`.wukong-environment.lock`）；prepare/install/restore 要求独占准入。维护前，调用者只能在所有任务证据同步后释放本评估的空闲租约。新的原生 worker 租约阻止在遗留任务或调试/交互资源存活时接管。关机 VM 不需要 worker 连接。Lab 工具不会关闭其他评估的连接和租约。基线恢复后，旧评估的 worker 身份仍保持固定；若启动身份变化，后续执行需要新建评估。

- `prepare` 只接受 `idle` 或 `failed`，校验并准备输入、推进代际、持久化意图、创建缺失的基线和实验快照，然后发布 `ready` 租约。
- `restore` 在恢复写入前校验调用者、活动 lab ID 和代际；回滚前使旧代际失效。只有确认恢复成功后，未知运行/安装记录才变为 `lost`。失败或超时保留不确定性。恢复成功后尝试清理该实验快照，并单独报告清理失败。
- `prune` 拒绝待恢复状态，保留基线/活动快照，只删除配置前缀下的闲置快照。
- `status` 读取快照，不调和或重写状态；有时限的宿主探测只是观测。其他操作持有修改锁时仍可查询。
- 中断的宿主影响不会重放。修改型恢复把中断意图归为 `recovery_required`；操作者须检查客体并调和准确的 operation ID。未解决的维护影响会阻止普通 worker 新任务，但观测与清理仍可使用。

Schema-1 文件会被明确拒绝：先用原版本完成/调和旧实验、保留证据，再明确归档旧 lab 状态并创建 schema-2 状态。不要把旧租约复制给另一 VM 或静默修改身份。旧的 `os_build`、`toolchain_sha256` 声明需移入可选说明 `annotations.os_build`、`annotations.toolchain`。

## 实验与包契约

CLI `ExperimentSpec` 包含校验的 `target_sha256`、有类型的启动/输入/账户要求、可选描述性 `annotations`、依赖、安装计划和恢复说明。说明不代表强制环境要求或已观测事实。测试程序运行准入只支持 `launch=exe`、`account=user`，以及 `input_channel=stdin` 或空 `argv`；其他要求在分发前被拒绝。所选可执行文件必须匹配绑定的目标哈希。

Agent 的 `lab_prepare` 使用有类型的 `spec`。归属由评估/Agent/代际推导，未传 `target_evidence_id` 时使用评估目标，依赖格式为 `{name,evidence_id}`。服务在该评估内解析证据、计算哈希并生成控制器副本。模型无需编造目标哈希，也不能传入 `owner`、URL 或控制器路径。新增工具参数对象均拒绝未知字段。

CLI 操作者可提供绝对现存路径或 HTTP(S) URL 以及 SHA-256。限制为 256 个成员、单文件 64 MiB、整包 512 MiB、单次下载 60 秒。包 ID 是规范 manifest 哈希。每次执行或准备介质前重新打开包时，都会校验该身份、成员名、大小预算和字节，拒绝链接/reparse 路径。同时修改成员和 manifest 也无法重新绑定原包 ID。

安装计划声明 Windows 绝对目标路径及可选服务/账户后置条件。服务二进制必须是声明目标，内核服务要求 `launch=driver`。清理动作是可选归属说明；基线恢复是清理边界，不自动反向执行脚本。

## 工具句柄与执行

`lab_list` 列出配置别名；`lab_status` 必须提供 `vm`，返回单 VM 状态。子 Agent 只能访问分配给自己的 VM/环境，并使用评估创建时记录的配置。其他 Agent 或代际不能复用租约。

`lab_prepare`、`lab_install`、`lab_restore` 会及时返回 `operation_id`。用 `lab_operation_status` 查询结果和原始证据；准备结果包含不透明的 `lab_ref` 及租约。run/install/restore 只接收绑定当前评估、Agent 代际和 VM 租约的 `lab_ref`，模型无需重建租约字段。这些变更操作需要 `windows_lab` 任务授权。快照裁剪只保留为操作员 CLI 操作。`lab_operation_cancel` 请求中断自己拥有的操作，取消不证明回滚成功。控制器重启后，缺失实时归属的操作会显示为未知，不重放。活动/未知宿主影响会阻止所属 Agent 和评估完成。检查客体后，按需用 `wukong lab reconcile` 调和 VM 意图，再使用现有操作者命令 `wukong reconcile <operation_id> --result-file ... --evidence-id ...` 解决其持久化副作用回执。

`lab_run` 返回普通评估任务账本中的 `run.job_id`。使用 `job_status`、`job_cancel`、`job_artifacts`；常规监督续期相同的评估/控制器租约。完成检查和证据导入遵循现有任务规则。Lab 回执记录准入身份，任务回执承载最终执行证据。后续恢复会先导入已稳定结束的任务观测。

CLI `lab run` 等待结果，每 15 秒续期独立租约，只释放自己的已稳定作用域。执行限制为 1–600 秒、单个可执行文件上传 32 MiB、stdin 1 MiB、输出预览 1 MiB。传输/轮询不确定时保留原运行身份并阻止替代执行。描述性规格不代表支持特权账户、GUI、网络或驱动执行。

## 管理安装证据

介质必须包含固定哈希的 `install.ps1`。控制器再次校验包，把已校验字节复制为宿主临时快照，传输到 `C:\Windows\Temp\wukong-lab-<install_id>`，再调用固定回执包装脚本。所有管理命令共享一个 1–600 秒总时限。取消会终止宿主命令，但客体影响可能仍然存在，因此保留未知状态。

客体包装脚本校验介质哈希，等待独立 PowerShell 脚本进程退出，记录退出码，并检查声明的目标文件、服务二进制/类型/运行状态及账户存在性。它发布绑定身份的 JSON 回执，控制器取回后保存在安装记录中。宿主 `vmrun` 退出成功不代表安装成功。缺失/不匹配回执保留未知结果；成功完成要求客体退出码为零且后置检查通过。脚本身份、客体回执、清理要求与安装计划哈希分别记录。客体观测是一次性环境提供的证据，不能替代独立利用验证。

## 驱动安装、测试签名与 Verifier

管理通道可以用同一个哈希固定的 `install.ps1` 安装内核服务：

```powershell
New-Item -ItemType Directory -Force C:\lab | Out-Null
Copy-Item (Join-Path $PSScriptRoot 'driver.sys') C:\lab\driver.sys
sc.exe create WukongLabDrv type= kernel start= demand binPath= C:\lab\driver.sys
sc.exe start WukongLabDrv
```

测试签名驱动还需要 guest 内的测试签名和受信任的测试证书：

```powershell
Import-Certificate C:\lab\test.cer -CertStoreLocation Cert:\LocalMachine\Root
Import-Certificate C:\lab\test.cer -CertStoreLocation Cert:\LocalMachine\TrustedPublisher
bcdedit.exe /set testsigning on
shutdown.exe /r /t 3 /f
```

重启后创建并启动服务，然后为该驱动启用标准 Driver Verifier 并再次重启：

```powershell
sc.exe config WukongLabDrv start= system
verifier.exe /standard /driver WukongLabDrv.sys
shutdown.exe /r /t 3 /f
verifier.exe /querysettings
```

测试签名足以加载测试驱动。本流程不会绕过或关闭 PatchGuard，第三方 PatchGuard 绕过工具不属于实验室；若某个实验确实需要，应作为独立的操作步骤并在结束后恢复基线。恢复基线会同时移除测试证书、testsigning 标志、服务和 Verifier 设置。

## 目录包与 Python 脚本

目录分析最多发现 256 个文件、512 MiB 和 16 层目录，计算成员哈希，推断可执行文件/库/数据类型及导入边。`bundle_manifest` 提供发现结果；`bundle_evidence` 导入校验成员用于静态分析。入口推断只是建议。

`launch_prepare` 的 `source.kind=bundle` 接受可选 `source.entry` 和 `source.members`（必须包含入口，最多 64 个），以及 `args`、`cwd`、`env`、`input_mode`（`stdin_pipe` 或 `argv_file`）和必填的 1–3600 秒时限。所选成员总计最多 32 MiB，相对路径最多五层。非根 cwd 必须存在于布局中；`launch.json` 为保留名称。`argv_file` 要求 `@@`，完整 LaunchSpec 不超过 64 KiB。无效选择和启动选项在上传 bundle 前被拒绝。更换入口使用独立的执行准入目录，更改启动配置生成不同的内容寻址路径。调用 `job_start` 时传返回的 `launch_spec`，省略 argv/timeout_secs/cwd/env 覆盖项。

`py_run` 必须提供且只提供一个非空 `source` 或 `source_file`；schema 拒绝缺失、空值和组合形式。Linux 与原生 Windows 均执行 256 KiB UTF-8 字节限制。原生文件输入在已准入的拥有者作用域内解析，按内容哈希封存后执行。Linux 也把脚本保留在内容派生路径下。两条路径都不能绕过普通任务归属或隔离。

## 静态后端选择

```toml
[analysis]
static_environment = "linux-static"
```

Ghidra 只探测和绑定选定的静态环境；普通执行/状态/清理只绑定主环境。两者都校验评估配置快照和持久化 worker 身份。只有观测到该后端能力后才公布静态工具，不能只凭配置名称。静态连接失败不会阻断普通 worker 清理；Windows worker 故障也不会阻断另一个可用静态环境的查询。

## 验证边界与历史证据

审计修复前的分支历史（`231d180`）记录过真实 VMware 快照/恢复、预编译 EXE 运行、服务安装/移除、自签名传统 WDM 驱动在测试签名及标准 Driver Verifier 下运行、Windows 到 Linux 的 Ghidra 分析，以及 `vulnserver.exe`/`essfunc.dll` 两成员 LaunchSpec 运行和取消。这些属于旧实现的历史观测，不是新 schema、异步句柄、租约隔离或客体回执包装脚本的验收证据。

当前回归覆盖过期/失败恢复、调用者/配置约束、包篡改、接口 schema 和原生/Python 源码准入。最终提交仍需重新进行真实 VMware 客体安装及重启验收。驱动覆盖不含 PnP/设备 I/O、生产签名或 PatchGuard 绕过。原生安装器测试应在隔离 runner 上运行，不能覆盖用户安装。

## CLI

```powershell
wukong --config wukong.toml lab status --vm lab
wukong --config wukong.toml lab prepare --vm lab --owner analyst --purpose parser --spec spec.json
wukong --config wukong.toml lab restore --vm lab --lab <lab_id> --generation <n>
wukong --config wukong.toml lab reconcile --vm lab --operation <operation_id> --outcome completed
```

VMware 命令应从正常完整性目录执行，Low 完整性的构建目录可能导致 VMware 无法启动。`start_after_restore` 应符合宿主实际的关机/运行快照行为。
