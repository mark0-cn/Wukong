# Actions 整理验收 — 2026-11-15

[English](2026-11-15-actions-redesign.md) · 简体中文

## 源码与范围

[PR #30](https://github.com/mark0-cn/Wukong/pull/30) 将 Actions 重构合入 `08886cfde1a6929f1b8e90bab2be13ded2fb9701`。仓库现有五个操作入口（Checks、Acceptance、Version、Release、Maintenance）和三个内部工作流（Policy、Client、Installer）。GitHub 自带的 Dependabot Updates 与 Dependency Graph 保持启用。参见[入口指南](../../development/github-maintenance.zh-CN.md#actions-entry-points)。

该 PR 在必需检查通过后，于 2026-11-15 04:05:57 UTC 通过原生自动合并完成，审阅数为零。公共规则集仍要求 PR、分支保持最新、讨论解决以及 GitHub Actions 的 `summary`/`commit-policy`，没有豁免对象。独立规则集仍要求非管理员取得一份批准，并在新增提交后撤销旧批准，仅豁免管理员角色。这验证了管理员自动合并及非管理员规则配置；未使用另一个非管理员账号实际测试合并。

## 实际检查

本地验证通过：161 项自动化测试、actionlint、版本一致性、PowerShell 在线包防篡改与安装器事务夹具、commitlint、中英文文档配对/链接/核对记录及 diff 检查。本地安装器夹具不会覆盖操作者的应用安装，也不调用真实 Docker 或模型 API。

| 运行 | 实际结果 |
| --- | --- |
| [PR Checks](https://github.com/mark0-cn/Wukong/actions/runs/43673888493) | Windows/Linux、两种 Ubuntu 的 APT 矩阵、真实 NSIS 安装卸载、acceptance-gate 与 summary 均通过 |
| [修改 PR 正文后的 Policy](https://github.com/mark0-cn/Wukong/actions/runs/43674111084) | 通过；原 Checks 持续执行，没有重启 |
| [Master Checks](https://github.com/mark0-cn/Wukong/actions/runs/43675255928) | 准确合并 SHA 上通过；普通合入跳过 release-request |
| [Version 预览](https://github.com/mark0-cn/Wukong/actions/runs/43675373382) | v1.0.1 dry run 通过；App token 及远端分支/PR 创建步骤均跳过 |
| [Maintenance cache](https://github.com/mark0-cn/Wukong/actions/runs/43675255874) | 依赖缓存校验与预热通过 |
| [Maintenance cleanup](https://github.com/mark0-cn/Wukong/actions/runs/43675376381) | apply=false 预览通过；cache/toolchain 作业跳过 |
| [Maintenance toolchain](https://github.com/mark0-cn/Wukong/actions/runs/43675719567) | 报告生成通过，没有升级依赖或发布 |
| [Release inspect](https://github.com/mark0-cn/Wukong/actions/runs/43675869554) | 对先前成功的 master CI 43667394398 进行只读资格检查并通过；未取得打 tag 凭据、未打 tag |
| [Release 演练](https://github.com/mark0-cn/Wukong/actions/runs/43676645599) | 容器验收与最终 summary 失败；复用准确 master Checks；镜像、三个客户端和两种原生安装包均构建成功；package/publish 跳过 |
| [额外 Windows worker 安装](https://github.com/mark0-cn/Wukong/actions/runs/43675380212) | 启动资格验证失败；acceptance-gate 也按要求失败 |

## Windows worker 缺口

额外的 `Acceptance/windows-worker` 使用 Windows Server 2025 build 10.0.26100，runner 镜像为 `windows-2025-vs2026`，版本 `20261103.298.1`。原生编译、单元测试与包契约均通过。随后安装在服务就绪检查中失败，Windows 错误为 1066：受限进程的回环连接超时，但资格验证没有取得匹配的实时 `AppContainerLoopback` WFP 拒绝事件。这是资格验证失败，不能证明网络隔离成功或安装成功。

| 证据身份 | 值 |
| --- | --- |
| 源码提交 | `08886cfde1a6929f1b8e90bab2be13ded2fb9701` |
| 签名包清单 SHA256 | `4e863c78db546ff014c56b520fa355d1e547409f3c47e9ec24782e4ac7406ebf` |
| [公开报告 artifact](https://github.com/mark0-cn/Wukong/actions/runs/43675380212/artifacts/14145829748) ZIP SHA256 | `66a2df3aae7025e49e806db05a430a32591fe8e55565eb43d3ab32cf696aaa48` |
| 解出的 report.json SHA256 | `f8278f22f737dae816c1ba82fa25c81c2bb9b0715e18e1103fb524b912798fb3` |

下载的 artifact 与其公布的摘要一致。公开报告记录 `cleaned_owned_os_resources=true`、`remaining_task_root=false`；临时 LSA secret 随一次性 VM 回收。报告明确不证明生产卸载、生产隔离或整机完全断网。上传的 artifact 保留七天，后续排查应保留已下载的原始报告。

PR #30 没有修改 worker 实现或安装夹具。操作者明确选择先完成 Actions 交付、保留该独立缺口，不修改 worker 隔离行为。后续需检查实时 WFP 订阅/筛选关联、实际拦截来源及托管宿主的事件配置；不能只凭超时判成功、跳过资格验证或把这次运行改记为通过。较早的 [Windows 10 v3 验收](2026-11-15-execution-contract-v3.zh-CN.md) 绑定另一组源码、包与环境，不能覆盖这次 Server 2025 失败。

## Linux 发布验收缺口

演练测试源码 `08886cfde1a6929f1b8e90bab2be13ded2fb9701`，使用候选镜像 `hlmark0/wukong@sha256:c8a150ec9c071d0f719f8aeb5c22916019eb1158b87a00d72868c2b4e4019487`。容器检查通过了兄弟网络隔离、固定上游中继、恢复卷时不保留进程/认证权限，以及统一启动/归档测试。随后 CLI 环境 doctor 在[验收作业 132699545968](https://github.com/mark0-cn/Wukong/actions/runs/43676645599/job/132699545968) 中失败：

```text
workbench_control.py:54: inventory(root, False, True)
TypeError: doctor() missing 1 required positional argument: 'registrations'
```

调用方传入三个参数，而 `worker_capabilities.doctor(root, trusted, isolated, registrations)` 需要四个。PR #30 没有修改这两个文件或发布镜像验收夹具。下载的作业日志 SHA256 为 `057108d83742ba7fe502dd8d9c3d54b06b11d3e5a2fd904e70aacdc901c808fb`。这定位了运行时调用方与签名不匹配的问题；后续 CLI/整包验收尚未通过。运行时修复应独立提交，回归验证该管理入口，并使用新镜像绑定的验收运行复测。不能用空能力集合替代或绕过失败的发布门禁，把这次演练改成绿色。

## 发布与验证边界

PR 检查先于合入，master 检查在合入后启动，必须分别跟踪。选中的 APT/卸载验收现已汇入必需 summary；显式选择的 Windows worker 安装属于独立专项，失败仍保持可见。缓存维护不属于合入门禁。

本任务没有授权新正式版本、远端版本 PR、tag 或 Release。打 tag 请求关联与已验收产物恢复由自动化测试覆盖；未实际执行正式打 tag/发布或发布恢复。发布演练可以推送公开候选镜像并上传已签名候选产物，但不改变正式版本/latest 指针。托管安装检查不能证明 Windows Docker Desktop 或任意生产目标验收通过。
