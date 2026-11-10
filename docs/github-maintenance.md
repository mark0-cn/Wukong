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
