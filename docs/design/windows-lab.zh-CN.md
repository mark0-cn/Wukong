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

仅支持当前状态格式和工具契约，不提供早期分支内格式的迁移流程或兼容层。系统和工具链说明使用可选字段 `annotations.os_build`、`annotations.toolchain`。

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

管理安装通道消费实验规格中哈希固定的 `install.ps1` 与包字节；Worker 安装介质只由下文的离线部署命令消费。控制器再次校验包，把已校验字节复制为宿主临时快照，传输到 `C:\Windows\Temp\wukong-lab-<install_id>`，再调用固定回执包装脚本。所有管理命令共享一个 1–600 秒总时限。取消会终止宿主命令，但客体影响可能仍然存在，因此保留未知状态。

客体包装脚本校验介质哈希，等待独立 PowerShell 脚本进程退出，记录退出码，并检查声明的目标文件、服务二进制/类型/运行状态及账户存在性。它发布绑定身份的 JSON 回执，控制器取回后保存在安装记录中。宿主 `vmrun` 退出成功不代表安装成功。缺失/不匹配回执保留未知结果；成功完成要求客体退出码为零且后置检查通过。脚本身份、客体回执、清理要求与安装计划哈希分别记录。客体观测是一次性环境提供的证据，不能替代独立利用验证。

## 离线 Worker 部署

`wukong lab provision` 在已登记的一次性实验 VM 关机时一次性安装 Windows Worker。它是仅限操作者的 CLI 操作：任何 Agent 工具、实验规格或模型输出都不能提供其路径或命令。该命令重新校验最终 Worker 介质，通过 WSL `guestfish`/`hivexsh` 把固定的部署包装脚本与计划写入离线系统卷，把 `WUKONG_OFFLINE` ISO 挂到专用 SATA CD 设备并启动 VM，由一次性 LocalSystem 服务安装 Worker 并让客体关机。随后控制器恢复原始 VMX 字节、离线读取客体回执、创建配置的基线快照，并可登记和校验控制器管理的 SSH 环境。客体不会收到控制器密钥或管理员密码。该特性是对 schema-2 实验环境状态的追加：`provisioning` 阶段、`provision` 待定操作类型和 `last_provision` 记录；已有的 idle/failed 状态文件因新增记录默认缺失而可继续加载。

当前验证只覆盖规划逻辑和 fake 宿主/磁盘/ISO/客体网络/环境适配器；CI 与本次特性的文档集成均未执行真实 WSL/libguestfs 注入、`mkisofs` 构建或 VM 启动。真实验收仍需操作者在专用 VM 上执行。

### 前置条件

- Windows 控制器；其他平台会以明确错误失败关闭。WSL 命令以该发行版的 `root` 用户运行。
- `[windows_lab.vms.<alias>.offline]`：`wsl_executable` 与 `mkisofs` 为绝对 Windows 路径，`wsl_distribution` 指定一个已注册 WSL 发行版，`guestfish`/`hivexsh` 为该发行版内的绝对 POSIX 路径，`timeout_seconds`（60–1800）限制单条工具命令。发行版必须提供 libguestfs `guestfish`、`hivexsh`、`wslpath`、`sha256sum`、`tee` 与 coreutils，并能通过 `wslpath` 访问 VM 的 VMDK。缺少该配置段会得到可操作的错误；控制器从不假设宿主工具布局。
- `python` 在 `PATH` 中，且使用 `--template` 时在 wukong 检出目录中执行命令，因为部署过程会自行调用 `python -m scripts.worker.media finalize`。`--media` 部署不使用 Python。
- 已登记且关机的实验 VM，且不存在快照链。只允许存在配置的 `baseline_snapshot`，并且仅当传入 `--replace-baseline`；任何其他快照都会拒绝命令且永不被删除。不存在快照时，`--replace-baseline` 是明确记录的空操作。启动盘必须是 VMX 目录内的完整本地 VMDK，位于 `nvme0:0`、`scsi0:0` 或 `sata0:0`；差异盘、链接盘与外部盘会被拒绝。VMX 必须启用 SATA 控制器并有空闲 CD 槽位。
- 请求 `--register` 时需要 VMware Tools：控制器在写入环境前用 `vmrun getGuestIPAddress` 发现客体 IPv4。
- finalize 规格中的 `management_peer` 必须是客体视角的控制器 IPv4 源地址。安装后的 Worker 把 SSH 精确固定到该地址（授权键 `from=`、`AllowUsers user@address`、自有防火墙规则），其他地址会导致环境 doctor 无法连通。
- `--register` 需要一个活动配置文件：传入全局 `--config`，或在可发现 `wukong.toml` 的位置运行。

```toml
[windows_lab.vms.lab]
baseline_snapshot = "wukong-lab-baseline"

[windows_lab.vms.lab.offline]
wsl_executable = "C:/Windows/System32/wsl.exe"
wsl_distribution = "Ubuntu"
guestfish = "/usr/bin/guestfish"
hivexsh = "/usr/bin/hivexsh"
mkisofs = "D:/VMware/VMware Workstation/mkisofs.exe"
timeout_seconds = 600
```

### 介质输入

只接受一种来源模式：

- `--media DIR` —— 已最终的介质目录。部署在接触客体磁盘前重新校验：schema-1 `media.json` 回执、setup/enrollment/manifest 的 SHA-256、签名登记记录与载荷清单、`disposable_vm` 执行边界及介质 SSH 身份。目录包含 `WukongWorkerSetup.exe`、`setup.json`、密封的 `payload/`、`enrollment/{enrollment.json,enrollment.sig}`、可选 `recovery/` 和 `media.json`；链接、reparse 点、未知条目、超过 20,000 个条目或超过 4 GiB 都会被拒绝。该目录是只读输入，不会被修改或删除。
- `--template DIR --keys DIR --spec FILE` —— 发布介质模板、绝对控制器签名密钥目录和 finalize 规格（schema 1：32 位小写十六进制 `installation_id`、`environment_id`、`generation`、规范化 IPv4 `management_peer`、`ssh_port`、仅允许 `disposable_vm` 边界、可选 `ssh_public_key`）。部署会把 finalize 输出到控制器状态目录下的本次部署目录，在规格省略 `ssh_public_key` 时生成控制器 SSH 身份，然后重新校验生成的介质并与规格交叉核对。密钥目录必须位于模板和输出之外；只有公开介质字节会被发布，成功或调和后最终介质会被删除。

发布附件 `wukong-windows-worker-media-<version>.zip` 是不含登记记录的未签名模板；操作者用控制器密钥在本地完成 finalize。附件说明见[离线 Windows Worker 介质](../development/releases.zh-CN.md#offline-windows-worker-media)，介质布局、规格字段与独立 `finalize`/`pack`/`iso` 命令见[原生 Windows Worker 组件](../../workers/windows/README.zh-CN.md)。桌面安装包内置 `worker-setup/WukongWorkerSetup.exe`（[内置 Worker 安装程序](../development/desktop-updates.zh-CN.md#bundled-worker-setup)），但不包含 Worker 介质；部署始终需要最终介质目录或发布模板。

### 部署顺序与基线

在任何变更之前，命令校验来源介质/模板与规格、VM 电源状态、快照规则以及 VMX CD 设备规划。无快照的原始 VM 还会在持久化任何内容之前预检关机系统卷（Windows 卷、SYSTEM 注册表配置单元、当前 control set、无遗留一次性服务或部署目录）。传入 `--replace-baseline` 时，关机的 VM 仍指向基线快照的差异盘，因此只有在删除该快照之后才能解析和预检客体磁盘：命令会先备份 VMX 并持久化意图，然后只删除配置的基线并校验提交后的磁盘。意图持久化之后的每次失败都可恢复而不会被重放。

变更顺序固定：在任何 VM 或客体变更之前备份原始 VMX 字节并持久化意图；传入 `--replace-baseline` 时，先删除配置的基线快照，随后才解析并预检提交后的客体磁盘；用 `-V WUKONG_OFFLINE` 构建 ISO；把 ISO 挂到空闲 SATA CD 设备；把包装脚本和计划写入固定客体路径；把 `ControlSetNNN\Services\WukongLabProvision-<id>` 补丁为一次性 LocalSystem 服务并校验取值；启动 VM 并在 `--timeout` 内等待客体关机（60–7200 秒，默认 1800；单条工具命令改用 `offline.timeout_seconds`）。客体包装脚本按卷标找到 CD，重新校验 setup/enrollment/manifest 哈希与签名登记记录，运行无需提权的 `verify-enrollment`，再执行经审阅的安装，写入回执，删除自身服务，安排删除自身文件并关闭客体。控制器恢复 VMX，离线确认服务、包装脚本和计划已被删除，重新校验回执身份，创建基线快照并保持 VM 关机。

部署完成后 VM 在新基线上关机。之后通常依次执行 `lab prepare`（为该实验代际从基线创建快照）和 `lab run`（通过已登记的 managed-SSH Worker 运行打包测试程序），`lab restore` 返回基线。`last_provision` 处于 running 或 unknown 时 `lab prepare` 会拒绝。`--register` 只写入 `[environments.<id>]`，不会修改 `[windows_lab.vms.<alias>]`；在运行打包测试程序前，需要把该 VM 的 `environment` 设为登记的环境 id。

只有当配置的基线已存在时才需要 `--replace-baseline`；不存在快照时它是明确记录的空操作（无快照流程会在客体回执通过校验后创建基线）。基线存在时，命令先备份 VMX 并持久化恢复意图，只删除该快照，然后才解析并预检提交后的客体磁盘；基线只在客体回执通过校验后才重建，其他快照永不被删除。删除后失败会留下无基线状态；操作保持 `recovery_required` 并指出 VMX 备份，因此先调和该操作，再重新部署（仅当基线重新存在时才再次需要 `--replace-baseline`）。

### 环境登记与 doctor

`--register` 必须搭配 `--template` 模式。只有当 finalize 规格省略 `ssh_public_key` 时，finalize 才会在控制器状态目录的本次部署目录下生成控制器管理的 SSH 密钥对（`--generate-ssh-key`）；介质模式部署无法提供该身份，携带 `ssh_public_key` 的规格也不能登记。私钥保留在控制器状态目录，配置只引用其路径；它绝不会进入介质、回执或客体。

客体启动后，命令等待 `vmrun` 报告的客体 IPv4，写入 `[environments.<environment_id>]` 作为 managed-SSH Windows 环境（installation id、`wkgm...` 用户、SSH 端口、控制器身份文件路径、由已校验回执主机键推导的公开 known-hosts 固定项），先写出旧配置的时间戳备份，不带 `--force` 时拒绝替换不同的环境，在未设置时写入 `default_environment`，并在替换时撤回旧的主机固定项。`--verify-environment` 要求环境已配置（由 `--register` 或手工写入），并在启动后运行 `EnvironmentRegistry::doctor`，按截止时间重试；它对两种介质模式都有效，也可与 `--register` 组合。成功后 VM 会回滚到新基线。登记或 doctor 失败会单独报告：命令会尝试回滚 VM，基线保留，部署记录显示 `environment_verified: false`。

### 试运行、失败与调和

`--dry-run` 执行相同的只读前置校验并打印精确的宿主机/客体计划动作（重新校验介质、ISO 路径、VMX 备份与 CD 设备、固定客体文件、注册表服务补丁、启动/关机、VMX 恢复、基线创建、回执路径），但不 finalize 模板介质、不构建 ISO、不编辑 VMX、不写客体文件、不创建快照，也不发布部署状态或回执。VM 仍需关机且离线工具必须已配置；无快照计划会只读检查磁盘。传入 `--replace-baseline` 且基线已存在时，只读计划止步于客体磁盘之前：关机的 VM 仍指向快照差异盘，因此计划会报告 VMX 备份、意图持久化和基线删除，并说明磁盘只在删除后才校验；它不会检查客体系统卷。`--iso-tool` 仅对本次命令覆盖配置的 `mkisofs` 可执行文件（必须是绝对路径，配置不变）；`--force` 只能与 `--register` 一起使用。

意图持久化之前的失败不会改动 VM、客体或快照，也不会创建部署记录；模板 finalize 可能在本次部署目录下留下未发布的中间文件。持久化之后的任何失败都会把 VM 置为 `recovery_required`，保留待定 `provision` 操作并写入状态为 unknown 的 `last_provision` 记录；错误会携带操作 id 与回执（`lab_provision_unknown`）。VMX 可能仍引用生成的 ISO，客体也可能残留部分部署状态，因此操作者必须先检查 VM 并在关机后执行 `wukong lab reconcile --vm <alias> --operation <operation_id> --outcome completed|failed|unknown`。`completed` 与 `failed` 会在解决操作前恢复原始 VMX 字节并删除生成的 ISO 和最终介质；`unknown` 保持未解决。调和绝不会重跑客体安装，也不会伪造通过的回执。控制器重启后，下一条会加载状态的 lab 变更命令会把未完成的待定部署操作转为 `recovery_required`；不会自动重放任何操作。

### 安全不变量

- 控制器私钥保留在控制器：`--keys` 只被本地 finalize 步骤消费；只有已签名登记记录、密封载荷和 setup 可执行文件进入介质/ISO；模板和输出会被检查以排除密钥目录和私钥文件名；`--register` 只保存控制器侧身份路径。
- 不使用客体管理员密码，也不经过管理通道：离线路径编辑关机 SYSTEM 配置单元、运行一个固定的一次性 LocalSystem 服务并从 CD 安装；已配置的 admin 通道和密码环境变量只用于管理安装脚本。
- 输入在端到端范围内按哈希固定：介质回执、签名登记记录和载荷清单会在 finalize 前后、启动前宿主侧以及客体包装脚本内分别校验；回读的客体回执绑定部署 id、安装 id、环境 id、代际、清单和 Worker 主机键。`production_isolation_verified` 始终为 false，因为客体回执不能证明 hypervisor 隔离。
- VMX 总会被恢复：原始字节在 CD 编辑前保存，成功时在创建基线快照前恢复，失败时由调和恢复；VM 开机时调和会拒绝执行。
- 路径与命令固定：只写入 `C:\ProgramData\WukongLab\provision\<32 位小写十六进制>` 路径、`WukongLabProvision-<id>` 服务名和固定 PowerShell 镜像路径；宿主工具通过固定参数、不经 shell 调用；介质、模板、密钥、ISO 与 VM 路径会校验为绝对路径；工具与部署等待都有时限；模型或实验输入永远不会进入该模块。
- 重跑受保护：遗留的一次性服务或部署目录会阻止部署，已存在的客体回执会阻止重装，只允许替换配置的基线，其他快照永不被删除。

### CLI 示例

```powershell
# 一次性：finalize 发布模板、部署、登记 managed-SSH 环境并运行 doctor。
wukong --config wukong.toml lab provision --vm lab `
  --template D:/release/wukong-windows-worker-media-<version> `
  --keys D:/wukong-controller-keys --spec enrollment-spec.json `
  --replace-baseline --register --verify-environment

# 校验并打印计划动作，不 finalize 介质、不改动 VM。
wukong --config wukong.toml lab provision --vm lab --media D:/media/windows-worker --dry-run

# 从最终介质部署且不登记环境；单次覆盖 mkisofs。
wukong --config wukong.toml lab provision --vm lab --media D:/media/windows-worker `
  --iso-tool "D:/VMware/VMware Workstation/mkisofs.exe" --timeout 2400

# 检查关机 VM 后解决失败的部署意图。
wukong --config wukong.toml lab reconcile --vm lab --operation <operation_id> --outcome failed
```

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

离线 Worker 部署由使用 fake 宿主、磁盘、ISO、客体网络和环境适配器的单元与规划测试覆盖，包括 VMX CD/磁盘规划、注册表配置单元与包装脚本生成、回执校验、介质/模板校验、部署状态恢复以及登记/配置写入。CI 和本特性的文档集成均未执行真实 WSL/libguestfs 注入、`mkisofs` 构建、VMware 启动、客体服务执行或 managed-SSH `doctor`；专用 VM 上的真实验收仍需操作者执行。

## CLI

```powershell
wukong --config wukong.toml lab status --vm lab
wukong --config wukong.toml lab prepare --vm lab --owner analyst --purpose parser --spec spec.json
wukong --config wukong.toml lab restore --vm lab --lab <lab_id> --generation <n>
wukong --config wukong.toml lab reconcile --vm lab --operation <operation_id> --outcome completed
```

VMware 命令应从正常完整性目录执行，Low 完整性的构建目录可能导致 VMware 无法启动。`start_after_restore` 应符合宿主实际的关机/运行快照行为。
