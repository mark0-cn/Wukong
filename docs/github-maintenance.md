# GitHub 维护功能

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

## 不可变 Release

仓库 Settings → General → Releases 的 **Enable release immutability** 已启用。
只作用于之后发布的 Release。发布后附件及关联 tag 锁定，升级需要新版本；
标题、说明和 latest 标记仍可调整。不要删除正式版本后尝试复用其 tag。

发布器先核对草稿的全部远端附件名称、大小和 SHA256，再发布；发布后校验
`immutable=true` 才推进 latest。若管理员关闭此设置，正式发布检查会失败，
不会悄悄把可变 Release 当作不可变版本。普通工作流无需管理员权限。

## Actions 运行摘要

### CI 调度与缓存

普通 CI 只在 `master` push 和面向 `master` 的 PR 触发，避免同一开发分支的 push/PR 重复执行。
未开 PR 的分支可手动启动 CI；版本准备工作流仍显式启动完整 CI。
PR 更新取消旧运行；主分支运行允许完成以保存缓存，等待中的请求由 GitHub concurrency 合并调度。

PR 使用 `scripts/ci_scope.py` 选择检查：仅 README/docs Markdown 改动执行轻量检查；
`ui/` 增加前端检查；`workers/runtime/`、`fixtures/analysis/` 增加 Linux 测试。
Rust、公共脚本、锁文件、工作流、未知路径或版本准备 PR 都执行全量检查。
重命名同时检查旧路径和新路径；计算范围失败直接阻止 CI 通过。
Windows 版本一致性、发布脚本和安装器测试始终运行。`master`、手动运行和发布调用始终全量。

`summary` 是最终检查门禁：要求范围计算及 Windows 检查成功，Linux 的结果必须与选择范围一致。
以后配置 required checks 时应包含 `summary`，不要仅依赖可能按规则跳过的 `linux`。
正常范围跳过会明确记录，失败/取消/异常跳过不能通过最终门禁。

Rust 使用固定工具链和参数派生缓存键，主分支负责写入，PR 只恢复。
CI 的 `windows-checks-v2` 缓存与 Release 编译缓存分开。首次或工具链/参数变化仍需重新编译。
摘要显示精确缓存命中状态及恢复、保存、fmt、Clippy、test 的独立耗时。
非精确命中可能是回退缓存或无缓存，实际恢复键见 Rust cache 日志。
Clippy 与测试保持在同一 Windows job 复用编译目录，不为并行而重复下载和编译依赖。
Windows CI 通过 `CARGO_PROFILE_DEV_DEBUG=1`、`CARGO_PROFILE_TEST_DEBUG=1` 使用有限调试信息，
保留调试断言与溢出检查；本地开发及正式 Release 配置不变。CI 故障可用的类型/变量调试信息会减少。
这两个环境变量属于 Rust 缓存键的输入，切换配置后的首次运行不能复用原配置的完整缓存。

优化验收分别记录首次运行、同配置缓存命中运行及仅 Python 改动 PR 的耗时，
同时比较总等待时间和 runner 用量；不把无缓存构建和缓存命中构建直接当作同条件性能对比。

CI 与 Release 在各任务结束后运行 `summary` job，包括前面任务失败的情况。
运行首页可查看 commit、ref、任务结果、耗时和产物链接；Release 额外显示版本、
镜像 digest、镜像解包大小、在线包大小与实际容器验收结果。
手动演练明确注明未正式发布，tag 发布只有 publish 成功才显示正式版本链接。

结论来自 GitHub `needs` 的任务状态；耗时及 artifact 链接通过只读 Actions API 获取。
API 暂不可用时展示缺失提示，不把未知耗时写成 0，也不把缺失验收报告当成通过。
摘要 job 在运行被强制取消或 GitHub 调度异常时仍可能无法执行，原始 job 日志为准。
工具链检查有独立摘要及 30 天报告，Release 的在线包与验收报告保留 14 天。
<!-- Temporary validation of documentation-only CI selection. -->
