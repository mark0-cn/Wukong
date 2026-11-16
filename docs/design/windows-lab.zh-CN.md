# Windows 实验 VM 生命周期

[English](windows-lab.md)

## 范围

Windows 能力计划的第一阶段为控制器提供一台已登记 VMware 实验 VM 的有界生命周期：创建基线与每次实验的快照、按代次围栏、恢复到基线，并对中断或超时的宿主操作进行核对。它还锁定并校验控制端依赖包，校验安装与清理计划。操作入口是 `wukong lab`。它尚未在 VM 内编译 harness、安装服务或驱动，也不暴露调试、网络和 fuzz 工具。

## 登记身份与配置

只能寻址 `[windows_lab.vms.<id>]` 条目。`vmx` 和 `vmrun` 是运维提供的绝对路径，所有参数由适配器构造；模型输入永远不提供 VMX、`vmrun` 参数或宿主命令。

```toml
[windows_lab.vms.lab]
vmx = "D:/vm/win10/Windows 10 x64.vmx"
vmrun = "D:/VMware/VMware Workstation/vmrun.exe"
baseline_snapshot = "wukong-lab-baseline"
snapshot_prefix = "wukong-lab"
operation_timeout_seconds = 300
start_after_restore = true
```

ID 为 1–64 个 ASCII 字母、数字、下划线或连字符。快照名允许字母、数字、`.`、`_` 和 `-`；前缀必须与基线不同且足够短。单次操作限制在 30–3600 秒。

## 状态与代次

状态位于 `<state_dir>/windows-lab/<vm_id>.json`，记录 schema、VM 身份、代次、阶段、基线、活动实验、待定操作、最后错误和更新时间。跨进程锁串行化操作，每次替换都原子发布。

阶段为 `idle`、`preparing`、`ready`、`restoring`、`recovery_required` 和 `failed`。

- `prepare` 仅在 `idle` 或 `failed` 时接受。它提升代次、持久化待定意图、仅在缺失时创建基线、创建一份 `<prefix>-<lab_id>` 快照，并进入带活动租约的 `ready`。
- `restore` 要求活动实验 ID 与代次。它先提升代次再请求 VMware 恢复，清空活动实验，恢复到基线，按配置启动 VM，并进入 `idle`。
- 过期代次在任何宿主调用之前被拒绝，因此旧租约即使在 VM 恢复后也无法重新获得执行权。
- 明确的 prepare 失败进入 `failed` 并允许重试；restore 失败和所有超时进入 `recovery_required`。
- 进程启动时发现待定的 `preparing` 或 `restoring`，说明控制器重启中断了宿主副作用，状态转为 `recovery_required`，不会重放。
- `reconcile` 由运维检查 VM 后，按精确操作 ID 将其解决为 `completed`、`failed` 或 `unknown`。

## 实验描述

`ExperimentSpec` 绑定 `target_sha256`、`launch`（`exe`、`dll_harness`、`service`、`gui`、`driver`）、`input_channel`（`argv`、`stdin`、`file`、`network`、`named_pipe`、`com`、`gui`、`ioctl`）、`account`（`user`、`elevated_user`、`system`、`driver`）、`os_build`、`toolchain_sha256` 和有界恢复说明。哈希必须是 64 个小写十六进制字符。规格哈希绑定到租约和活动状态。有效描述只是准入前提，不是执行证据。

## 准备流水线

依赖与安装属于实验描述的一部分：

```json
{
  "dependencies": [
    {"name": "harness.c", "sha256": "<64 个小写十六进制>",
     "source": {"kind": "path", "path": "D:/lab/harness.c"}}
  ],
  "install": {
    "targets": ["C:\\lab\\harness.exe"],
    "cleanup": [{"kind": "file", "target": "C:\\lab\\harness.exe"}]
  }
}
```

- 依赖来源可以是绝对且存在的 `path`，也可以是 `http`/`https` 的 `url`。单件产物上限 64 MiB，整包上限 512 MiB，下载超时 60 秒。
- 每个字节都按声明的 SHA-256 校验。包 ID 是规范清单的哈希，包发布到 `<state_dir>/windows-lab/packages/<id>/`，并包含 `manifest.json` 与 `SHA256SUMS`。
- 复用时会重新校验每个已有字节。不匹配直接失败，不会覆盖；损坏的包由运维显式删除。
- `install.targets` 是不含点段的绝对 Windows 路径。服务二进制必须是其中一个目标；`kernel_driver` 要求 `launch=driver`；每个目标、服务和账号都必须有覆盖它的清理动作（`file`、`directory`、`service` 或 `account`）。
- `wukong lab package` 在不接触 VM 的情况下物化并校验依赖包。`lab prepare` 在改变任何状态之前物化依赖包，并把 `package_sha256` 与 `install_sha256` 绑定到待定意图、活动实验和租约。
- 校验通过的依赖包和合法安装计划只是准入输入，不是资格认证。harness 编译、试运行准入与 guest 安装仍属于后续阶段。

## 宿主操作

适配器只使用 `vmrun -T ws list`、`listSnapshots <vmx>`、`snapshot <vmx> <name>`、`revertToSnapshot <vmx> <name>` 和 `start <vmx> nogui`。每次调用都有超时并捕获 stdout/stderr，从不经过 shell。基线仅在缺失时创建；已有快照从不删除。

## 限制

- CLI 必须从正常完整性目录运行。构建目录可能带有 Low 强制标签，VMware 在该标签下拒绝启动。桌面发布脚本已经把可执行文件复制到工作区之外；手工运行 CLI 也应这样做。
- guest 安装仍需要既定的离线介质和运维/管理员路径。本生命周期不猜密码，也不修改 VM 磁盘或账号。
- `start_after_restore` 必须与宿主恢复运行中快照的行为一致：在本机上运行中 VM 的快照恢复后处于关机状态，因此实验室按配置启动它。
- 实验快照不会自动删除；运维在保留证据后显式删除。
- 第一阶段已用真实 VM 验证：创建基线与实验快照，在第 2、4 代恢复到基线，VM 启动后 worker `doctor` 再次就绪。控制器重启、超时和过期代次行为由使用假宿主的单元测试覆盖。

## 命令行

```powershell
wukong --config wukong.toml lab status --vm lab
wukong --config wukong.toml lab prepare --vm lab --owner analyst --purpose parser --spec spec.json
wukong --config wukong.toml lab restore --vm lab --lab <lab_id> --generation <n>
wukong --config wukong.toml lab reconcile --vm lab --operation <operation_id> --outcome completed
```
