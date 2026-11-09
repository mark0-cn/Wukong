# 在线发布与 GitHub Actions

仓库：`mark0-cn/re_agent`。私有镜像：`hlmark0/re-agent-workbench`。只发布 Windows x64 在线安装包，镜像平台为 linux/amd64。

## 仓库配置

Settings → Secrets and variables → Actions：

| 类型 | 名称 | 值 |
| --- | --- | --- |
| Variable | DOCKERHUB_USERNAME | hlmark0 |
| Variable | DOCKERHUB_NAMESPACE | hlmark0 |
| Secret | DOCKERHUB_TOKEN | Docker Hub 私有仓库 Read/Write PAT，不需要 Delete |

PAT 到期后在 Docker 重新创建并替换 Secret。不要将 PAT 写入源码、包、命令参数或发布说明。客户端使用自己的读取权限登录。

默认使用 GitHub 标准托管 `ubuntu-24.04` / `windows-2025`。镜像构建和验收是不同 job，彼此只传 digest 与小文件，不传镜像 tar。磁盘准备脚本仅允许在 GitHub 托管 Linux VM 上运行，删除该 job 不需要的预装 SDK，打印空间并做预检。若空间不足，可以把可选 Variable `WORKBENCH_LINUX_RUNNER` 设置为已配置的大规格 **GitHub 托管** runner 标签；不会自动开通付费资源。

私有 GitHub 仓库的标准 runner 使用计入账号 Actions 配额；镜像首次无缓存构建较慢。缓存写入同一私有 Docker Hub 仓库的 `buildcache` 标签。只保留客户端和报告类 Actions artifacts，默认 7–14 天过期。

## 日常 CI

`ci.yml` 在分支 push、PR 和手动触发时运行。Windows 执行 Rust fmt/clippy/test、前端测试与构建、在线包校验测试；Linux 执行 Python runtime 和分析评测工具测试。外部 PR 不获取 Docker Hub Secret。

Rust 版本由 `rust-toolchain.toml` 固定；Cargo 与 npm 使用锁文件。上游 Debian/Python 的全部传递依赖并未完全锁死；已发布镜像以 digest 为准，不能把重新构建视为逐位可复现。

## 发布版本

1. 同步修改 `Cargo.toml`、`Cargo.lock` 中 re-agent 的版本、`tauri.conf.json`、`ui/package.json`、`package-lock.json` 中 ui 的版本。
2. 执行 `python scripts/release.py check`，提交并推送默认分支。
3. 创建并推送 tag，例如：

```sh
git tag -a v0.2.0 -m "Release v0.2.0"
git push origin v0.2.0
```

tag 必须匹配源码版本，并且 commit 已包含在默认分支。`v0.2.0-rc.1` 发布为预览版，不更新 `latest`。

`release.yml` 自动执行：

1. 校验版本与来源；并行执行完整 CI、Windows Release 编译和镜像构建。
2. 使用 Bake 联合构建基础 worker 和 workbench，不依赖本机镜像；所有外部工具按 `workers/toolchain.lock.json` 下载并校验。发布候选标签 `build-<完整commit>`，附 SBOM 和 provenance。
3. 新 runner 按 digest 拉取候选，运行网络隔离、固定目标转发、LaunchSpec、归档测试，以及 host CLI 的 doctor、题目导入/构建/提取/启动/连接、GDB 附加、更新回滚。
4. 生成 Windows 在线 zip、release.json、SHA256SUMS、doctor.json、validation-summary.json。
5. 上传 GitHub Release 草稿，给同一个镜像 digest 添加固定版本标签，正式发布 Release。
6. 正式版仅在没有更高正式版本时推进 Docker Hub `latest` 与 GitHub latest。发布检查最后核对附件列表与镜像 digest。

固定版本不会覆盖已发布镜像。GitHub 与 Docker Hub 没有跨服务原子事务，失败时可能留有候选或草稿/固定镜像标签；修复后使用 Actions **Re-run failed jobs**，复用原始 artifacts 和候选 digest。已经发布的附件若与重试产物不同，流程拒绝覆盖，应发布新版本。

发布使用全局 concurrency 锁，避免并行更改 latest。GitHub concurrency 对待运行任务不保证 FIFO；不要连续推送多个待发布 tag，若某次等待任务被替换，手动重跑该 tag 对应的工作流。

可启用 GitHub immutable releases；所有附件均在草稿阶段上传。仓库规则可保护 `v*` tag，Docker Hub 如支持也可将精确版本标签设为不可变，但 `latest`、`buildcache` 必须可更新。

## 演练与排错

Actions → Release → Run workflow，选择默认分支。手动演练会推送私有候选镜像并生成 `online-release` artifact，**不创建 GitHub Release、不更新版本标签或 latest**。正式发布由 tag push 触发。

查看失败 job：

- 登录失败：检查 Docker Hub PAT 私有仓库读写权限、有效期和 GitHub 配置。
- 空间预检失败：查看 `df` 输出，改用更大托管 runner 或进一步缩减镜像；不自动降级/跳过验收。
- 依赖下载失败：重试，缓存文件仍按 SHA256 核对。
- 镜像已存在但不匹配：不删除或覆盖已有版本，改发新版本。
- 验收失败：查看 `image-validation` artifact 的 doctor 与失败报告，不升级 latest。

候选标签与 buildcache 共用私有镜像仓库，需按空间情况定期清理无引用的旧候选。CI Token 没有 Delete 权限，因此工作流不删除仓库标签。正式版本及用户正在使用的 digest 应保留。

## 验收边界

Windows runner 编译嵌入前端的正式客户端，校验安装包与篡改拒绝，并用独立进程夹具验证首次安装、doctor 失败后的恢复、升级成功和手动回滚；这些安装器事务测试不调用真实 Docker。Linux runner 执行真实 Docker 集成测试。标准 Windows runner 不等同于用户 Docker Desktop + WSL2 桌面，不能声称完成了该环境的全自动端到端验收。首次及关键安装器改动应在真实 Windows 桌面补充安装/升级确认。当前 Windows 文件未做 Authenticode 签名。

用户安装方法见 [在线安装](online-install.md)。发布包不包含镜像 tar、模型密钥、用户配置或题目数据；原有工作区导入/导出功能与 CI 在线发行独立。
