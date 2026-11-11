<a id="github-maintenance"></a>

# GitHub 维护功能

[English](github-maintenance.md) · 简体中文

## Dependabot

默认分支的 `.github/dependabot.yml` 管理 Cargo、npm workspace、
`workers/runtime/requirements.txt` 和固定到 commit 的 GitHub Actions。
每周一北京时间 09:10–09:40 分批检查，版本更新最多同时打开 7 个 PR。
rig 系列依赖一起更新，其余兼容更新按生态分组；安全更新另行分组。
漏洞提醒和自动安全更新在仓库设置中启用，PR 仍需通过 CI 并人工合并。

Python Dockerfile 中内联的依赖、apt/gem、AFL 基础镜像和自定义工具锁文件
不属于这份 Dependabot 配置的覆盖范围。Rust 编译器也不单独自动升级：
`rust-toolchain.toml` 与 `Cargo.toml` 的 `rust-version` 必须一起修改。
Tauri 的 Rust crates 与 npm 包升级需要检查双方兼容性。

Dependabot 的 PR 使用普通 CI，不提供 Docker Hub 或模型密钥。
配置文件不能约束其他 PR 合并方式，也不能替代私有仓库套餐所限制的 Rulesets。

<a id="toolchain-update-checks"></a>

## 工具链更新检查

Actions → **Toolchain updates** → Run workflow 可手动运行；每周一北京时间
10:23 自动检查默认分支，GitHub 的定时任务可能延迟。使用标准 Ubuntu runner，
只读权限，不使用 Docker Hub 或模型密钥，不下载大型安装包。

检查 pwndbg、pwninit、Ghidra、Temurin JDK 25 和 GEF 的最新稳定版本。
报告包含版本差异、上游链接和 SHA256 来源，上传 `toolchain-update-report`
artifact（30 天），包含 `report.md`、`report.json` 和 `candidates.json`。
发现新版本是正常结果；API/资产格式错误或同版本 SHA256 改变会令检查失败。
缺少上游 SHA256 的资产标为 `checksum_review_required`，不生成候选条目。
GEF 使用固定源码 commit，校验 Git blob 身份并计算 SHA256，标为需要源码审查；
这不等于上游独立签名或发布者校验文件。

本阶段只生成升级提案，不自动改锁文件、开 PR、合并或发布。
准备升级时审查 `candidates.json` 和上游变更，更新锁文件及 Dockerfile 中重复的
SHA256/路径设置，执行下载校验，再构建镜像并完整验收。Temurin 保持 JDK 25；
apt、gem、Dockerfile 内联 pip 依赖与 AFL 基础镜像另行维护。

<a id="immutable-releases"></a>

## 不可变 Release

仓库 Settings → General → Releases 的 **Enable release immutability** 已启用。
只作用于之后发布的 Release。发布后附件及关联 tag 锁定，升级需要新版本；
标题、说明和 latest 标记仍可调整。不要删除正式版本后尝试复用其 tag。

发布器先核对草稿的全部远端附件名称、大小和 SHA256，再发布；发布后校验
`immutable=true` 才推进 latest。若管理员关闭此设置，正式发布检查会失败，
不会悄悄把可变 Release 当作不可变版本。普通工作流无需管理员权限。

<a id="actions-summaries"></a>

## Actions 运行摘要

<a id="ci-scheduling-and-caches"></a>

### CI 调度与缓存

普通 CI 只在 `master` push 和面向 `master` 的 PR 触发，避免同一开发分支的 push/PR 重复执行。
未开 PR 的分支可手动启动 CI；版本准备工作流使用发布 App 创建 PR，由原生 PR 事件启动完整 CI。
PR 更新取消旧运行；主分支运行允许完成以保存缓存，等待中的请求由 GitHub concurrency 合并调度。

PR 使用 `scripts/ci/ci_scope.py` 选择检查：仅 README/docs Markdown 及配套翻译核对记录改动执行轻量检查；文档配对、链接与核对记录始终在 changes job 检查；
`ui/` 增加前端检查；`workers/runtime/`、`workers/tests/`、`fixtures/`、`evals/` 增加 Linux 测试。
Rust、公共脚本、锁文件、工作流、未知路径或版本准备 PR 都执行全量检查。
重命名同时检查旧路径和新路径；计算范围失败直接阻止 CI 通过。
Windows 版本一致性、发布脚本和安装器测试始终运行。`master`、手动运行和发布调用始终全量。

`summary` 是最终检查门禁：要求范围计算及 Windows 检查成功，Linux 的结果必须与选择范围一致；选中的版本客户端预构建也必须成功。
以后配置 required checks 时应包含 `summary`，不要仅依赖可能按规则跳过的 `linux`。
正常范围跳过会明确记录，失败/取消/异常跳过不能通过最终门禁。

Rust 使用固定工具链和参数派生缓存键，主分支负责写入，PR 只恢复。
CI 的 `windows-checks-v2` 缓存与 Release 编译缓存分开。首次或工具链/参数变化仍需重新编译。
`windows-client-v1` 和 `linux-client-v1` 在 master 版本预构建或显式预热时写入，tag 只恢复。
这使不同版本 tag 都能读取 master 缓存，避免把大量编译缓存只保存在无法跨 tag 复用的作用域。
摘要显示精确缓存命中状态及恢复、保存、fmt、Clippy、test 的独立耗时。
非精确命中可能是回退缓存或无缓存，实际恢复键见 Rust cache 日志。
Clippy 与测试保持在同一 Windows job 复用编译目录，不为并行而重复下载和编译依赖。
Windows CI 通过 `CARGO_PROFILE_DEV_DEBUG=1`、`CARGO_PROFILE_TEST_DEBUG=1` 使用有限调试信息，
保留调试断言与溢出检查；本地开发及正式 Release 配置不变。CI 故障可用的类型/变量调试信息会减少。
这两个环境变量属于 Rust 缓存键的输入，切换配置后的首次运行不能复用原配置的完整缓存。

优化验收分别记录首次运行、同配置缓存命中运行及仅 Python 改动 PR 的耗时，
同时比较总等待时间和 runner 用量；不把无缓存构建和缓存命中构建直接当作同条件性能对比。

CI、客户端构建及安装器构建的 Ubuntu 依赖步骤共用 `python -m scripts.ci.install_apt <packages>`。该入口拒绝在开发机或自托管机器上运行，仅支持一次性的 GitHub 标准托管 Ubuntu 22.04/24.04 x64 runner。任务专用 APT 配置选择官方 HTTPS archive/security 仓库，使用已安装的 Ubuntu archive keyring 和独立索引，保留签名校验。它绕过 runner 的 Azure 镜像选择器，不改写原有源文件、DNS 或凭据；Playwright 的依赖安装也使用这份配置。网络传输超时 15 秒、重试两次；索引更新与安装进程分别限时 120/480 秒，另留 15 秒强制结束剩余进程。依赖步骤外层限时 12 分钟，Linux 前端步骤限时 15 分钟。索引下载失败会严格报错，不接受陈旧索引。依赖安装与测试分开展示，网络故障不会显示为测试执行卡住。参数见 Ubuntu 的 [APT 配置](https://manpages.ubuntu.com/manpages/jammy/man5/apt.conf.5.html)及 [apt-get](https://manpages.ubuntu.com/manpages/jammy/man8/apt-get.8.html) 手册。这些设置限制失败等待时间，不能保证 Ubuntu 或 runner 网络始终可用。产品运行环境和 Docker 镜像的 APT 配置不受影响。

`Ubuntu dependency acceptance` 在 PR 修改其安装入口或验收文件时运行，也可手动执行。轻量的 Ubuntu 22.04/24.04 矩阵实际安装经过签名校验的软件包，检查间接 APT 调用继承的配置，并确认不可连接的回环仓库返回 APT 错误 100；同时检查原有源文件没有变化。该验收不构建或发布安装器，也不替代平台 CI、Docker 验收和原生卸载测试。

CI 与 Release 在各任务结束后运行 `summary` job，包括前面任务失败的情况。
运行首页可查看 commit、ref、任务结果、耗时和产物链接；Release 额外显示版本、
镜像 digest、镜像解包大小、在线包大小与实际容器验收结果。
手动演练明确注明未正式发布，tag 发布只有 publish 成功才显示正式版本链接。

结论来自 GitHub `needs` 的任务状态；耗时及 artifact 链接通过只读 Actions API 获取。
API 暂不可用时展示缺失提示，不把未知耗时写成 0，也不把缺失验收报告当成通过。
摘要 job 在运行被强制取消或 GitHub 调度异常时仍可能无法执行，原始 job 日志为准。
工具链检查有独立摘要及 30 天报告，Release 的在线包与验收报告保留 14 天。

<a id="docker-and-dependency-caches"></a>

### Docker 与附件缓存

`Cache Worker Dependencies` 在 master 的锁文件、下载器或该工作流变更时预热，也可手动运行。
缓存键包含工具锁文件哈希；发布恢复 master 附件缓存，逐个 SHA256 校验后使用。
默认最多并行下载 3 个附件，可用 `--jobs 1` 串行下载；临时文件校验成功才替换正式文件。
网络错误或损坏内容不能作为有效缓存输入。缓存被淘汰时自动重新下载。

workbench 继承独立的稳定 toolchain stage，runtime、prompts、版本标签在工具安装之后加入。
各工具只挂载自己的附件，某个附件变化不会仅因共享目录变化就使所有附件层失效。
Docker Hub `buildcache` 保存完整中间层。pip cache mount 用于同一 builder 内复用；
标准临时 runner 之间的持久化依靠 registry 层缓存和附件缓存，不假定 cache mount 自动跨机器保留。

<a id="closed-pr-cache-cleanup"></a>

### 关闭 PR 后的缓存回收

`Clean Closed PR Caches` 每天北京时间 06:41 运行，读取最新 PR 状态后按缓存 ID 删除：
已关闭 PR 的 `refs/pull/N/merge` 缓存，以及对应已关闭 Dependabot PR 的机器人分支缓存。
master、tag、仍打开的 PR 及未知普通分支均保留。定时任务可能延迟。
手动运行默认只列出候选；勾选 `apply` 才删除。摘要显示数量和回收字节数。
只在 master 上执行，不使用 `pull_request_target` 或运行 PR 代码，不需要个人管理员 Token。

## 提交规范与发布预览

只读 `Commit policy` 流程使用固定版本 commitlint 验证 PR 标题与新增非合并提交，并检查发布片段。PR 只需英文。PR 标题或正文编辑时重跑该流程，不重启平台 CI；生成仅本 PR 和累计预览前，会运行真实 git-cliff/commitlint 回归测试。可选本地 hook 和双语发布片段见[提交规范](commits.zh-CN.md)。master Ruleset 中应同时要求 GitHub Actions 的 `commit-policy` 和已有 `summary` 状态；二者不能互相替代，保留已有审批、讨论解决和严格更新要求。仅有 workflow 文件无法启用服务端合并限制。规范检查不代表发布授权，也不会创建 tag。

发布片段保留 PR 原有的相关检查范围；`.changes/` 中的未知文件仍需完整检查。
