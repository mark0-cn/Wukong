<a id="github-maintenance"></a>

# GitHub 维护功能

[English](github-maintenance.md) · 简体中文

<a id="master-merge-policy"></a>

## 主分支合入规则

`master` 使用两套面向不同对象的规则。CI 与审阅豁免分开配置，保证管理员的审阅权限不能绕过执行检查。

| 规则集 | 适用对象 | 要求 |
| --- | --- | --- |
| `master - PR and required checks` | 所有人，没有豁免对象 | 必须通过 PR；GitHub Actions 的 `summary` 与 `commit-policy` 必须通过；分支必须保持最新；审阅讨论必须解决；禁止强推或删除；允许 merge/squash |
| `master - review for non-admins` | 仓库管理员角色以外的所有人 | 至少一份批准审阅；新提交使旧批准失效；不额外要求 Code Owner、最后推送者或无归属改动的批准 |

仅含审阅要求的规则集使用 `bypass_mode: exempt` 排除仓库管理员角色。GitHub 将其定义为对指定对象跳过规则执行，区别于显式绕过操作。公共规则集没有豁免。非管理员仍需批准审阅；这项配置不会向其他用户授予合并或写入权限。参见 [GitHub 的豁免说明](https://github.blog/changelog/2025-09-10-github-ruleset-exemptions-and-repository-insights-updates/)。

执行已授权合入前，读取分支实际生效规则及每个匹配规则集的 `bypass_actors`、`current_user_can_bypass`。核实 PR 的准确 head SHA、最新 base、可合并状态、讨论已解决，以及 GitHub Actions 的全部必需检查通过。不能只凭 `REVIEW_REQUIRED` 就判断当前账号没有允许的合入路径。

GitHub 自动合并会等待合入条件满足；排队并不等于完成。如果管理员已获授权的普通 PR 具备适用的豁免，却仍因审阅要求而等待，应通过 `PUT /repos/{owner}/{repo}/pulls/{number}/merge`，携带已核实的 `sha` 和 `merge_method: merge` 完成合入。GitHub 仍会强制执行公共规则。响应不确定时先查询 PR 状态，再决定是否重试。不能为解除阻塞而放宽规则、伪造批准或绕过 CI。非管理员缺少必需批准时，必须取得有效审阅。参见[合入 API](https://docs.github.com/en/rest/pulls/pulls#merge-a-pull-request)。

无论使用哪种路径，合入后都须核实 `merged=true`、记录实际 `master` 提交，并跟踪触发的 CI 至最终结果。修改规则必须获得明确授权，且保存修改前后的配置。版本 PR 在启用自动合并或执行合入前仍须取得明确发布授权；审阅豁免不代表发布授权。

<a id="actions-entry-points"></a>

## Actions 入口

仓库维护八份工作流：五个操作入口和三个内部工作流。GitHub 自带的 Dependabot Updates、Dependency Graph 保持启用；旧运行历史保留。

| 入口 | 文件 | 职责与触发 |
| --- | --- | --- |
| Checks | `ci.yml` | PR、master 改动检查；手动完整检查/客户端预热；必需 `summary` 门禁 |
| Acceptance | `acceptance.yml` | 手动 `all`、`apt`、`uninstall`、`windows-worker` 专项验收；Checks 复用选中的 APT/卸载验收 |
| Version | `prepare-release.yml` | 默认 `dry_run=true` 预览；取消勾选后创建五文件版本 PR |
| Release | `release.yml` | `rehearse` 默认演练、`inspect` 只读检查、`tag` 已授权版本合并打标签、`recover` 恢复；tag push 正式发布 |
| Maintenance | `maintenance.yml` | `cleanup`、`cache`、`toolchain` 分任务运行；仅缓存删除 job 有 Actions 写权限 |
| Internal / Policy | `commit-policy.yml` | PR 标题/正文编辑只重新检查规范与发布说明；必需 `commit-policy` 门禁 |
| Internal / Client | `build-client.yml` | 仅供调用的客户端构建 |
| Internal / Installer | `build-installer.yml` | 仅供调用的安装器打包/验收 |

`Acceptance/all` 运行三个专项验收，不替代 `Checks` 的全量组件检查或 `Release/rehearse` 的容器/发布包验收。原生安装卸载只在可丢弃的托管 runner 执行。内部工作流仍可能显示在 Actions 列表，通过名称区分。普通 PR 只有 Checks 与 Policy 两个顶层运行；专项验收作为 Checks 子任务汇总。普通 master 提交不启动 Release。

需要自动合并的普通 PR 在创建后显式开启 GitHub 原生 auto-merge；仓库允许自动合并不等于每个 PR 已开启。检查当前等待的是 CI、review 还是讨论解决，保持管理员审阅豁免与非管理员至少一份批准的规则。

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

Actions → **Maintenance** (`task=toolchain`) → Run workflow 可手动运行；每周一北京时间
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

Windows 原生 worker 安装验收需要显式选择：手动运行 Checks 时启用 `windows_worker_acceptance`，或在推送待验收修订前给 PR 加上 `windows-worker-acceptance` 标签。仅修改标签不会启动 CI。此选项强制完整组件检查，并在原生编译后运行独立托管夹具；所选运行启动后移除标签，避免之后无关修订重复安装。夹具拒绝开发宿主和自托管机器，仅上传公开报告。按 worker 程序阻断外网会保留 CI agent 联网，不能替代最终的 Windows VM 整机断网验收。它不发布 Release，也不证明生产目标隔离或卸载已经完成；参见[原生组件指南](../../workers/windows/README.zh-CN.md)。

原生编译及选中的安装验收使用显式的未取消条件，因此此前无关测试失败不会阻止获取这项独立证据。安装仍要求原生构建和检查步骤通过；取消、原生构建失败或未选择夹具时均不安装。只有夹具实际运行并得到成功或失败结果后才上传报告。此前的失败仍使 Windows 作业及最终 CI 门禁失败；没有步骤使用错误后继续通过设置。参见 GitHub 的[状态检查表达式规则](https://docs.github.com/en/actions/reference/workflows-and-actions/expressions#status-check-functions)。

PR 与 master 使用 `scripts/ci/ci_scope.py` 按完整改动范围选择检查：README、AGENTS、docs Markdown、有效的追加式 `.changes/<slug>.toml` 与翻译核对记录只做轻量检查，整项 Windows/Linux job 均跳过；changes 始终校验文档配对、链接和核对记录。`ui/` 增加前端检查；runtime/tests/fixtures/evals 增加 Linux 与 Windows Python 测试。Rust、公共脚本、锁文件、工作流和未知路径执行完整组件检查；运行时 prompts 不属于普通文档。版本 PR、版本递增的 master 提交、手动 Checks 和发布调用执行完整组件及 APT/卸载验收。PR 使用 merge-base 范围，master 使用 push 的 before/after 全范围；重命名包含新旧路径，计算失败阻止 CI。Windows 发布脚本与安装器事务测试仅随 Rust/共享改动运行。

`summary` 是最终门禁：changes 必须成功，Windows、Linux、复用的 acceptance 和三个客户端预构建结果必须与选择范围一致。APT/卸载按受影响路径或工作流变更选中；子工作流的 acceptance-gate 独立检查每个选中 job。需要运行的任务失败、取消或意外跳过均不能通过；只有未选中的任务允许跳过。始终触发顶层 Checks，在 job 内选择范围，避免 required check 永久 Pending；Policy 的标题/正文编辑不取消平台 CI。

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

CI、客户端构建及安装器构建的 Ubuntu 依赖步骤共用 `python -m scripts.ci.install_apt <packages>`。该入口拒绝在开发机或自托管机器上运行，仅支持一次性的 GitHub 标准托管 Ubuntu 22.04/24.04 x64 runner。先检查软件包安装状态，只下载缺失的软件包，随后探测所需工具和库。若全部已安装，跳过联网并保持 APT 配置不变。Linux worker 检查需要调试工具和测试程序编译器；仅在 Rust 桌面检查时加入桌面开发库，在同一步骤完成安装。

需要下载时，脚本将 kernel.org、MIT 和最后的 Canonical archive/security HTTPS Ubuntu 源作为完整尝试依次处理。每次尝试使用直接签名源及全新的独立索引。索引或软件包下载失败后，本次调用不再使用该镜像，后续文件不会返回失败镜像。选中的配置继续供 Playwright 等间接 APT 调用使用。原始源文件、DNS、凭据和 Ubuntu archive-keyring 签名校验保持不变。每个镜像的索引更新限时 40 秒、软件包下载限时 120 秒，全部网络尝试共用 480 秒预算。APT 传输超时十秒，不进行逐文件重试；结束进程另留五秒宽限。只有下载完整后，才允许进入独立的 `--no-download` 本地安装阶段，限时 180 秒。省略翻译和 command-not-found 索引。依赖步骤外层仍限时十二分钟，前端步骤仍限时十五分钟。全部镜像失败或软件包、工具检查失败时，CI 必须失败，不接受陈旧索引，也不跳过必需测试。此机制限制等待时间，不保证镜像或网络始终可用；产品运行环境和 Docker 镜像的 APT 配置不受影响。

`Acceptance / apt` 在依赖脚本、验收或工作流变化时选中，也可以通过 `suite=apt` 手动运行。Ubuntu 22.04/24.04 矩阵测试真实签名索引和软件下载、独立离线安装及预装软件包复用。回环服务器接受连接后不返回响应头，或返回原始签名 InRelease 后在索引内容传输中途挂起；两者都必须放弃整个失败镜像，并通过官方源恢复。仅有挂起镜像时，即使之前存在有效索引，也必须在期限内失败。验收检查间接 APT 调用继承的配置及原有源文件保持不变。该验收不构建或发布安装器，也不替代平台 CI、Docker 验收和原生卸载测试。

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

`Maintenance / cache` 在 master 的锁文件、下载器或该工作流变更时预热，也可手动运行。
缓存键包含工具锁文件哈希；发布恢复 master 附件缓存，逐个 SHA256 校验后使用。
默认最多并行下载 3 个附件，可用 `--jobs 1` 串行下载；临时文件校验成功才替换正式文件。
网络错误或损坏内容不能作为有效缓存输入。缓存被淘汰时自动重新下载。

workbench 继承独立的稳定 toolchain stage，runtime、prompts、版本标签在工具安装之后加入。
各工具只挂载自己的附件，某个附件变化不会仅因共享目录变化就使所有附件层失效。
Docker Hub `buildcache` 保存完整中间层。pip cache mount 用于同一 builder 内复用；
标准临时 runner 之间的持久化依靠 registry 层缓存和附件缓存，不假定 cache mount 自动跨机器保留。

<a id="closed-pr-cache-cleanup"></a>

### 关闭 PR 后的缓存回收

`Maintenance / cleanup` 每天北京时间 06:41 运行，读取最新 PR 状态后按缓存 ID 删除：
已关闭 PR 的 `refs/pull/N/merge` 缓存，以及对应已关闭 Dependabot PR 的机器人分支缓存。
master、tag、仍打开的 PR 及未知普通分支均保留。定时任务可能延迟。
手动选择 `task=cleanup` 默认只列出候选；勾选 `apply` 才删除。每日清理和每周工具报告分别按自己的 cron 路由，不互相触发。摘要显示数量和回收字节数。
只在 master 上执行，不使用 `pull_request_target` 或运行 PR 代码，不需要个人管理员 Token。

## 提交规范与发布预览

只读 `Internal / Policy` 流程使用固定版本 commitlint 验证 PR 标题与新增非合并提交，并检查发布片段。PR 只需英文。PR 标题或正文编辑时重跑该流程，不重启平台 CI；生成仅本 PR 和累计预览前，会运行真实 git-cliff/commitlint 回归测试。可选本地 hook 和双语发布片段见[提交规范](commits.zh-CN.md)。公共 master 规则集中应同时要求 GitHub Actions 的 `commit-policy` 与 `summary` 状态；二者不能互相替代。审阅豁免遵循[主分支合入规则](#master-merge-policy)，所有人仍需满足讨论解决及严格更新要求。仅有 workflow 文件无法启用服务端合并限制。规范检查不代表发布授权，也不会创建 tag。

发布片段保留 PR 原有的相关检查范围；`.changes/` 中的未知文件仍需完整检查。
