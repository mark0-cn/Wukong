# 架构整理验收 — 2026-11-15

[English](2026-11-15-architecture-cleanup.md) · 简体中文

## 范围与身份

已批准的文件整理与 Python 设计工作已实施。最终控制器/Worker 实现提交为 `c7ed9bcf76c25adae08b6bbab3e63e70eff99a11`；随后 `d9af37fa321fd02451508f9ce77634247404787a` 只修正 doctor 测试的 mock，最终工作台验收使用包含该修正的镜像。原生输入哈希确认打包源码输入未变，后续纯文档提交也不改变这些执行身份。最终集成及绑定提交的托管检查见 [PR #29](https://github.com/mark0-cn/Wukong/pull/29)。

| 身份 | 值 |
| --- | --- |
| 原生输入树 SHA256 | `d6c91e7fe2a48fbaa46e7d35f3982d3dd11e6f17db335e1dbbb6a8c5fed0fc4e` |
| 签名包清单 SHA256 | `e697990f6aa1afb3e1fd44a3f68dfca58d76f2888a9b7ff9f88500093d762be1` |
| 新 native73 安装实例 | `958ef753fc7646598935ce8f79c53940` |
| 控制器可执行文件 SHA256 | `1032d7f1948192a0b72141c0325865fbae742b40ac9fdac5ea9e29a9e731189c` |
| 可信探针 SHA256 | `ae5159ab60fd19b506a957cc555cea65bac6dff6075428086dfbc4437ce06f56` |
| `c7ed9bc` 的 Linux 工作台镜像 | `sha256:ec8adb211a48b83913540ed4ab2336c0c22345b0b96a17c4e89ddaa47cb7503e` |

## 已交付的结构与行为

批准的基线清单中，965 个文件均已核对实际去向，无缺失。控制器根层 Rust 文件从 32 个降为 5 个，实现分别归属 domain、queries、application、storage、adapters 与 interfaces。存储层采集一次中立 SQL 快照，桌面展示在事务结束后消费快照。文件系统所有权独立于维护策略，bootstrap 负责装配应用依赖。公开 Rust 路径保留集中兼容导出。

前端实现和测试按功能归组，共用 API、UI 和测试基础设施。runner 私有测试与 SSH 安装测试靠近所属实现。Python runtime 顶层仅保留 `bridge.py` 一个 Python 入口，内部使用职责明确的 `wukong_worker` 包；部署身份覆盖嵌套路径和文件字节。共享分析保持独立于 Worker 状态。编辑器隐藏生成产物；维护文档约定新任务产物使用 `target/tasks/<task-id>`，未删除旧证据。

Python 工作包括固定 Ruff 格式和静态检查、不可变作业准入计划、明确的回调及分析输入类型、显式导入和启动依赖顺序、合并薄分析适配器、有界测试传输以及资源所有权文档。格式提交保留了全部 197 个基线 Python 文件的 AST；768 组准入差异比较保留了规范化参数、指纹和错误。本验收提交的 214 个 Python 文件均有 AST、导入与风险点清单，这不表示对每个算法完成了穷尽式人工正确性证明。

三个单独提交的行为修复带有双语发布片段：不可读恢复元数据产生有界脱敏诊断；工作台 doctor 使用临时状态采集实际注册表；交互 stdin 阻塞时遵守期限并允许终止会话。部分送达或结果不确定的输入不会重试。协议 v3、控制器 schema 5、原生账本 2 和包 schema 2 均保持不变。

## 验证结果

| 检查 | 实际结果 |
| --- | --- |
| 默认桌面功能 Rust 全目标测试与 Clippy | 232 项通过，31 项依赖环境的测试忽略；包路径更新后 Clippy `-D warnings` 通过 |
| 前端 | 功能迁移后 38 项单元测试、36 项 Playwright 流程及生产构建通过 |
| Windows Python | 分析类型注解后运行 142 项，22 项因平台跳过；后续交互真实管道回归与 bridge 套件通过；托管 CI 覆盖最终 143 项测试源码 |
| 新建非 root Linux 容器中的 Python | `c7ed9bc` 运行 143 项：138 项通过、5 项跳过 |
| 自动化与安装脚本 | 158 项测试通过；发布元数据检查及临时在线包、安装器夹具通过 |
| 真实 Linux 工具 | GDB、AFL/QEMU、重放、最小化、隔离、Ghidra 和启动夹具通过 |
| Linux 控制器集成 | GDB、AFL、angr、重放/最小化四类执行器测试通过；独立 verifier 测试通过 |
| Linux 网络与工作台 | 专用兄弟容器网络的采集、补丁、反例、绑定和 transcript 检查通过；真实服务、relay、归档、PTY 和启动检查在 `d9af37f` 通过 |
| 工作台 doctor | 13 项真实探测全部通过，包括 i386 执行、GDB、pwntools、pwninit、分析与 Ghidra |
| 专用 Windows 安装 | 新 native73 安装器退出码 0，准确绑定的安装服务进入 `Running` |
| 已安装 Windows 矩阵 | 控制器、x64/x86 进程与调试器、Python、shell、交互、重放和最小化各组通过 |
| Windows 服务重启 | 进程、Python、shell、交互、重放和最小化的已完成副作用恢复全部通过，连接身份未变 |
| 文档与生成目录 | 双语链接和审阅记录通过；四份生成工具目录字节不变 |

绑定首轮提交的 [CI](https://github.com/mark0-cn/Wukong/actions/runs/43672859356)、[依赖验收](https://github.com/mark0-cn/Wukong/actions/runs/43672859370)和[隔离卸载验收](https://github.com/mark0-cn/Wukong/actions/runs/43672859383)均在 `c7ed9bc` 通过。这些结果不冒充后续 PR 或 master 提交的检查；忽略的测试和跳过的发布客户端构建不计为通过。

专用 Windows VM 没有默认路由。启动和重启资格检查验证了受约束的 x64/x86 PDB 加载、断点和栈，实际 WFP 网络丢弃，更换自身 DACL 后的受保护进程/线程访问，私有账本 ACL 与独占写锁。模型凭据留在控制器；安装和卸载验收没有修改用户的桌面安装。

## 新工作流与证据

`run_98bcdf8617874858ac1598b779279c74` 使用新工作区。analyst 暂存可信探针、执行 `--echo`、读取原始输出并运行 Python 源码；另一位 verifier 读取证据、独立暂存并执行探针，随后记录 supported 的 `analysis_verify`。两个子任务结束后，root 调用 `finish_scan`。root、analyst、verifier 分别以 9、20、10 轮完成，工具错误为零。

三个 AppContainer 作业以退出码 0 完成，一个探索作业取消并保留记录。四个作业的 Job、profile、scratch 清理均成功。72 份证据的大小和 SHA256 全部与记录一致；`verify-bundle` 验证了 72 个文件，共 2,582,259 字节。报告用量为 1,525,356 tokens，不作为性能基准。

原生账本最终有 74 个已关闭资源和 114 个已完成副作用。不存在归属本实例的执行 scratch、调试器、目标或 Python 进程。管理 SSH 和核对的两个旧 Worker 服务保持运行。797 文件的原始归档逐文件验证通过，归档 SHA256 为 `415b80d4e042fdf079aa0fe1c49c8863ab52942f09451e38bafb3d722f859d70`。采集结束后已卸载 native73 安装介质，合格实例与证据继续保留。

583 份原始文件完成归档和哈希验证后，清理了七个带明确任务标签的 Linux 验收容器及其七个命名状态卷；没有剩余任务标签容器。未确定归属的卷保留，清理未使用全局或按日期删除。

本地原始记录保留在本任务的 `target/architecture-plan` 与 `target/windows-vm-native73-*` 路径；新工作流位于 `target/tasks/architecture-cleanup/workflow`。迁移核对、Python 设计决策、验收汇总和清理回执均引用保留的原件。这些私有本地记录不是 Release 附件或 CI 制品。

## 修正与限制

集成前，master 已前进到包含独立 Actions 整理的 `08886cf`。合并保留其 Checks/Acceptance/Release/Maintenance 行为，将集中管理的卸载验收选择器更新到整理后的路径。新合入 Python 文件的格式调整保留 AST，回归用例覆盖已移动的清理、所有权、恢复和 UI 入口。上游新增三个 Python 文件后，合并清单共 217 个文件。最终 PR/master 检查对应这棵集成文件树；这些基础设施变化不改变已记录的控制器/Worker 输入。

首次集成版 Windows 运行中，两个多 Agent 调度夹具触发了共用的十秒测试期限。同一提交在本机通过全部 232 项 Rust 测试，31 项依赖环境的测试忽略。第一次仅修改测试，为完整多轮场景提供有上限的 30 秒，并在超时时报告角色、状态、轮数和代次。语义断言及单独的取消/等待时限检查保持原样，生产执行期限未变。失败的托管尝试保留在[运行 43676701828](https://github.com/mark0-cn/Wukong/actions/runs/43676701828)。

下一次集成[运行 43678220841](https://github.com/mark0-cn/Wukong/actions/runs/43678220841) 中，Linux Ghidra 锁测试和 Windows 多 Agent 夹具失败，剩余构建已取消。锁测试原先假定释放一个句柄后内核锁立即释放，但 Unix 中的复制描述符或短暂继承的描述符仍可能持锁。测试现明确保留一个副本来确定性复现延迟释放，并在最多五秒内等待释放，同时继续检查互斥、同一锁文件以及不确定分析标记的保留。本地 Windows 锁测试及 Clippy 通过，另有 Linux 原语级复现实验通过，后者不替代仓库 Linux 套件。生产加锁与进程回收逻辑未变。

后一次 Windows 超时采样到 root 为第五轮、第二代 analyst 已完成，仅凭该快照不能区分进度延迟与交接通知丢失。本地并发重复运行全部 16 个调度场景 30 次均通过，限制双核运行也通过。夹具现分别限制 Agent 身份、状态、轮数、代次均无变化最多 30 秒，以及整个场景最多 120 秒；失败时保留带时间戳的进度历史与最近事件标识。心跳不会延长进度记录。这是测试诊断和调度余量调整，不声称修复了生产竞态；生产期限与取消断言保持原样。

失败观察单独保留：首次镜像构建遇到 Ubuntu HTTP 502，重试通过；首次网络执行误选了带隐式卷的工作台镜像，未满足网络挂载清单检查，随后构建专用网络 Dockerfile 并通过，未放宽检查。Linux doctor 冷启动测试暴露 mock 意外拦截 `platform.platform()`；修正后仅拦截目标探测，冷启动测试与完整工作台套件通过。验收脚本中的旧包路径和任务本地归档命令也在最终成功观察前修正。

本记录证明可信仓库夹具及所列组合通过，不证明任意目标利用成功或所有故障交错情况。文件哈希不证明映射内存身份；随包 Python 的可执行文件定位警告仍保留在原始 stderr。本次未重跑真实 Windows Docker Desktop 安装，Linux 容器验收不能覆盖它。本记录描述实现与验收，没有版本变更、标签或正式发布。
