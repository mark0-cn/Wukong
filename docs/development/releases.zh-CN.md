<a id="online-releases-and-github-actions"></a>

# 在线发布与 GitHub Actions

[English](releases.md) · 简体中文

仓库：`mark0-cn/Wukong`。公开镜像：`hlmark0/wukong`。发布 Windows x64 NSIS 安装 EXE 和 Linux x64 AppImage；镜像平台为 linux/amd64。

<a id="repository-configuration"></a>

## 仓库配置

Settings → Secrets and variables → Actions：

| 类型 | 名称 | 值 |
| --- | --- | --- |
| Variable | DOCKERHUB_USERNAME | hlmark0 |
| Variable | DOCKERHUB_NAMESPACE | hlmark0 |
| Secret | DOCKERHUB_TOKEN | Docker Hub 仓库 Read/Write PAT，不需要 Delete |
| Secret | TAURI_SIGNING_PRIVATE_KEY | Tauri 更新签名私钥内容，仅可信发布工作流使用 |
| Variable | RELEASE_APP_ID | 专用发布 GitHub App 的 App ID |
| Secret | RELEASE_APP_PRIVATE_KEY | 该 App 生成的 PEM 私钥全文 |

PAT 到期后在 Docker 重新创建并替换 Secret。不要将 PAT 写入源码、包、命令参数或发布说明。公开镜像允许客户端匿名拉取；发布凭据只用于 CI。

发布 App 只需一次性配置：

1. 在 [GitHub Apps 设置](https://github.com/settings/apps/new) 创建 App，Homepage 填仓库 URL，关闭 Webhook；仅允许安装到自己的账号即可。
2. Repository permissions 设置 **Contents: Read and write**、**Pull requests: Read and write**；不需要 Administration、Actions write 或 Workflows write。
3. Install App → Only select repositories → `mark0-cn/Wukong`。
4. 将 App ID 保存到上述仓库 Variable，Generate a private key 后将 PEM 全文保存到上述 Secret；不要把私钥提交到仓库或聊天。

工作流按当前仓库签发短期 installation token，结束后撤销。创建 PR 时请求 Contents/PR 写权限；打 tag 时只请求 Contents 写权限。
App 创建的 PR 会触发原生 `pull_request` CI，App 推送的 tag 会触发发布工作流；不再额外 dispatch 一套分支 CI。
这是为避免内置 `GITHUB_TOKEN` 创建 PR 时的工作流批准要求，以及该 token 的 push 事件不触发后续工作流的限制。
参考 [GitHub 触发规则](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow)。

默认使用 GitHub 标准托管 `ubuntu-24.04` / `windows-2025`。镜像构建和验收是不同 job，彼此只传 digest 与小文件，不传镜像 tar。磁盘准备脚本仅允许在 GitHub 托管 Linux VM 上运行，删除该 job 不需要的预装 SDK，打印空间并做预检。若空间不足，可以把可选 Variable `WORKBENCH_LINUX_RUNNER` 设置为已配置的大规格 **GitHub 托管** runner 标签；不会自动开通付费资源。

本项目 GitHub 仓库公开，使用标准托管 runner；镜像首次无缓存构建较慢。缓存写入同一公开 Docker Hub 仓库的 `buildcache` 标签。只保留客户端和报告类 Actions artifacts，默认 7–14 天过期。runner 计费和存储额度按 GitHub 当前账号政策；大规格 runner 与超额存储应另行核对。

<a id="build-native-installers-locally"></a>

## 本地生成原生安装文件

在 Windows x64 或 Linux x64 的源码目录执行 `cargo package-local`。
入口检查源码已提交、工作区干净和构建工具，再调用 Tauri 生成当前平台的 NSIS EXE 或 AppImage，输出到 `target/local-installers/`。
不下载工具镜像、不发布版本。首次运行客户端时从对应正式 Release 获取签名环境清单，所以未发布版本的本地构建不能通过在线向导获取该版本环境。

```sh
cargo package-local --check
cargo package-local --output target/installers/build-1
cargo package-local --signed
```

已有输出目录拒绝覆盖。`--check` 检查源码和工具入口，系统库及安装器工具由实际构建验证。
`--signed` 需要 `TAURI_SIGNING_PRIVATE_KEY`；普通本地构建无需发布密钥。
Windows 需要 MSVC、Rust、Node.js/npm、Python；Linux 还需要 Tauri WebKitGTK、GTK、AppIndicator、patchelf 等构建依赖。
Linux 正式 AppImage 在 Ubuntu 22.04 构建并进行真实 WebView 启动检查，不承诺覆盖所有发行版。
旧 ZIP 包脚本保留用于开发夹具，正式流水线及 `cargo package-local` 不再以 ZIP 为用户安装入口。

独立 launcher 检查：`cargo fmt --manifest-path xtask/Cargo.toml -- --check`。

<a id="everyday-ci"></a>

## 日常 CI

`ci.yml` 在 `master` push、面向 `master` 的 PR 和手动触发时运行。PR 按改动范围选择检查；`master`、手动运行、版本 PR 和发布调用执行全量检查。Windows 执行 Rust fmt/clippy/test、前端测试与构建、在线包校验测试；Linux 执行 Python runtime 和分析评测工具测试。外部 PR 不获取 Docker Hub Secret。范围规则与缓存策略见 [GitHub 维护功能](github-maintenance.zh-CN.md)。

Rust 版本由 `rust-toolchain.toml` 固定；Cargo 与 npm 使用锁文件。上游 Debian/Python 的全部传递依赖并未完全锁死；已发布镜像以 digest 为准，不能把重新构建视为逐位可复现。

<a id="publish-a-version"></a>

## 发布版本

1. 打开 **Actions → Prepare Version PR → Run workflow**，分支选择 `master`，填写完整的新 tag，例如 `v0.2.0`。正常准备版本时不要勾选 `dry_run`。
2. 工作流同步 `Cargo.toml`、`Cargo.lock` 中 wukong 的版本、`tauri.conf.json`、`ui/package.json`、`package-lock.json` 中 ui 的版本；检查格式、版本递增和一致性，运行发布脚本测试，然后创建 `codex/release-v0.2.0` 分支及目标为 `master` 的 PR。
3. App 创建 PR 后，原生 CI 自动启动并显示在 PR 页面。Windows、Linux、summary 通过后，由维护者确认合并；**合并版本 PR 即批准发布**。
4. 合并提交在 `master` 上执行全量 CI，并行预构建 Windows Release 客户端、Linux 验收 CLI 和 Ubuntu 22.04 Linux 桌面 Release 客户端。全部成功后 **Tag Merged Release** 自动校验并推送 tag，指向经过检查的合并提交；即使随后 `master` 前进，也不会误标到新提交。
5. tag 自动触发 **Build and Publish**，完成构建、验收并发布 Docker Hub 镜像和 GitHub Release。无需本地操作或手动推送 tag。

`Prepare Version PR` 使用标准 GitHub 托管 runner 和发布 App，不需要个人 PAT；内置 `GITHUB_TOKEN` 仅承担只读工作。
普通 PR 不会触发自动打 tag。自动发布只接受同仓库、由所配置 App 创建、分支为 `codex/release-vX.Y.Z` 的已合并版本 PR；合并结果必须是五个版本文件的纯版本递增，不能夹带其他改动。
打 tag 前重新查询该合并提交的 CI 及当前尝试，要求 changes、windows、linux、summary 全部成功。CI 未通过就不会打 tag；修复后需准备新的有效版本 PR，或重跑该提交的 CI/打 tag 工作流处理临时故障。
同名 tag 已指向相同提交时重试直接通过；指向其他提交时拒绝覆盖。已有 tag 的发布失败应重跑对应 **Build and Publish**，不删除重建 tag。
旧的 `github-actions[bot]` 版本 PR、手动创建的版本 PR以及本功能安装前已合并的 PR不作追溯发布。
版本准备不创建 tag、不构建镜像、不发布 Release。输入的版本必须高于当前版本；已存在的 tag 会被拒绝。
重试只复用文件树与本次准备完全一致的版本分支，不强推覆盖。若 `master` 或版本分支已有变化导致重试失败，先处理已有 PR/分支，再重新准备。

勾选 `dry_run` 可以查看同步后的 diff 和测试结果，不创建远端分支或 PR。
本地也可以执行 `python -m scripts.release.cli bump --tag v0.2.0` 一次同步全部版本文件用于检查；自动发布请通过上述 App 版本 PR 流程。
该命令不更新依赖，也不运行 Cargo/npm 依赖解析；原有版本不一致、版本倒退或格式错误时拒绝修改。

tag 只允许 `v主版本.次版本.补丁版本`，例如 `v0.1.0`、`v12.3.45`。
每段使用 ASCII 数字，允许单独的 `0`，不允许前导零。
`1.2.3`、`V1.2.3`、`v1.2`、`v01.2.3`、`v1.2.3-rc.1`、`v1.2.3+build` 都会被拒绝发布。
tag 必须同时匹配源码版本，并且 commit 已包含在默认分支 `master`。

工作流监听所有 tag，在 metadata 阶段先校验格式和版本，不通过就不会构建、打包或发布。
工作流校验本身不会拒绝非法 Git push；服务器端强制要求需要适用的 tag Ruleset。
历史提交的 tag 使用该提交中的工作流。仓库当前 Rulesets 应另行核对，不能把工作流过滤当作服务器强制规则。

`release.yml` 自动执行：

1. 校验版本与来源；查询同仓库、同 SHA 的成功 master push CI，核对当前尝试的 changes/windows/linux/summary 后复用结果。无合格记录或 API 查询失败时执行完整 CI。
2. 查找同 SHA 的可信 CI/发布客户端产物；无合格产物就构建。下载后必须核对平台、版本、完整提交、构建配方及二进制 SHA256/大小，再执行或打包。使用 Bake 联合构建稳定 toolchain 和 workbench；外部附件按 `workers/toolchain.lock.json` 校验，包括缓存命中。发布候选标签 `build-<完整commit>`，附 SBOM 和 provenance。
3. 新 runner 按 digest 拉取候选，运行网络隔离、固定目标转发、LaunchSpec、归档测试，以及 host CLI 的 doctor、题目导入/构建/提取/启动/连接、GDB 附加、更新回滚。
4. Windows/Linux 均使用已校验桌面客户端直接打包 NSIS EXE/AppImage，不重复编译；Linux 验收 CLI 单独复用；签名并执行原生安装/启动检查。汇总两个平台文件，生成签名 `latest.json`、`release.json`、SHA256SUMS 与验收报告，再用 Rust 验证全部安装文件签名与哈希。
5. 上传 GitHub Release 草稿，给同一个镜像 digest 添加固定版本标签，正式发布 Release。
6. 正式版仅在没有更高正式版本时推进 Docker Hub `latest` 与 GitHub latest。发布检查最后核对附件列表与镜像 digest。

固定版本不会覆盖已发布镜像。GitHub 与 Docker Hub 没有跨服务原子事务，失败时可能留有候选或草稿/固定镜像标签；修复后使用 Actions **Re-run failed jobs**，复用原始 artifacts 和候选 digest。已经发布的附件若与重试产物不同，流程拒绝覆盖，应发布新版本。

同 SHA 的其他可信运行可提供成功上传、尚未过期的客户端及原生安装器。上一尝试的产物只有在 GitHub 仍保留时才可复用；完整 Re-run all jobs 可能删除旧产物，不能保证复用。优先 Re-run failed jobs；验证跨运行复用时，在同一提交新建一次手动演练，保留成功的来源运行，不要重跑或删除它。仅接受当前所查询尝试中成功的构建 job，PR 产物不进入发布链。元数据不符、文件损坏会直接失败，不执行可疑客户端。安装器复用固定来源运行及 artifact ID，要求对应安装/启动冒烟 job 成功；下载后核对提交、版本、打包配方、公钥配置及安装文件/签名的哈希和大小。旧格式安装器不会复用。复用安装器不重复执行安装测试，但最终 Rust 更新签名验证仍必须通过。产物过期、被删除时需要重建；发现身份或配方不匹配直接失败；缓存只加速编译，不能替代这些校验。

客户端和报告保留 14 天。客户端以低压缩级别上传；原生安装文件不再重复压缩。摘要只下载 `release-report` 小型元数据与验收报告，不下载完整安装包。运行名区分 CI、版本打 tag、正式发布与手动演练，摘要链接到实际复用的运行。

发布使用全局 concurrency 锁，避免并行更改 latest。GitHub concurrency 对待运行任务不保证 FIFO；不要连续推送多个待发布 tag，若某次等待任务被替换，手动重跑该 tag 对应的工作流。

本仓库启用 GitHub immutable releases，后续正式发布会锁定附件及关联 tag。
所有附件均在草稿阶段上传，发布前检查远端附件名称、大小、SHA256 与本地产物完全一致；
发布后确认 `immutable=true` 才推进 latest。草稿中有旧附件时会拒绝发布，先清理草稿再重试。
不向 Actions 提供管理员 Token；仓库设置由管理员开启，流水线只读取 Release 的锁定状态。
这不锁定 Docker Hub 标签；固定镜像版本仍由发布脚本的禁止覆盖检查保护，
Docker Hub 如支持也可将精确版本标签设为不可变，但 `latest`、`buildcache` 必须可更新。

<a id="rehearsal-and-troubleshooting"></a>

## 演练与排错

Actions → Build and Publish → Run workflow，选择默认分支。手动演练会推送公开候选镜像并生成 `online-release` artifact，**不创建 GitHub Release、不更新版本标签或 latest**。正式发布由 tag push 触发。

首次预热或验证客户端复用时，先运行 **CI**，选择 `master` 并勾选 `build_release_clients`；成功后在同一提交上运行发布演练。正常版本合并自动预构建，不需每次手工预热。普通 master push 不额外编译 Release 客户端。Linux 桌面预构建和 AppImage 打包均使用 Ubuntu 22.04；预构建在 master 保存独立的 linux-desktop 编译缓存，tag 复用同 SHA 的二进制，缺失时按相同配方构建。

Actions → Tag Merged Release → Run workflow，选择 `master`，填写一个已成功的 master push CI run ID，只读检查该提交是否符合自动打 tag 条件。手动入口永远不创建 tag，也不签发 App token；只有成功 CI 的 `workflow_run` 事件可以进入自动打 tag 步骤。

查看失败 job：

- 登录失败：检查 Docker Hub PAT 对 `hlmark0/wukong` 的读写权限、有效期和 GitHub 配置。
- App token 失败：检查 App ID/PEM、App 是否安装到该仓库、Contents/PR 权限及新增权限是否获安装批准。
- PR 未显示 CI：核对 PR 作者是否为发布 App，查看 PR Checks 页面；不要将 Prepare Version PR 成功视为全部 CI 通过。
- 空间预检失败：查看 `df` 输出，改用更大托管 runner 或进一步缩减镜像；不自动降级/跳过验收。
- 依赖下载失败：重试，缓存文件仍按 SHA256 核对。
- 镜像已存在但不匹配：不删除或覆盖已有版本，改发新版本。
- 验收失败：查看 `image-validation` artifact 的 doctor 与失败报告，不升级 latest。

候选标签与 buildcache 共用公开镜像仓库，需按空间情况定期清理无引用的旧候选。CI Token 没有 Delete 权限，因此工作流不删除仓库标签。正式版本及用户正在使用的 digest 应保留。

<a id="acceptance-boundary"></a>

## 验收边界

Windows runner 编译嵌入前端的正式客户端，校验安装包与篡改拒绝，并用独立进程夹具验证首次安装、doctor 失败后的恢复、升级成功和手动回滚；这些安装器事务测试不调用真实 Docker。Linux runner 执行真实 Docker 集成测试。标准 Windows runner 不等同于用户 Docker Desktop + WSL2 桌面，不能声称完成了该环境的全自动端到端验收。首次及关键安装器改动应在真实 Windows 桌面补充安装/升级确认。当前 Windows 文件未做 Authenticode 签名。

用户安装方法见 [在线安装](../user/online-install.zh-CN.md)。发布包不包含镜像 tar、模型密钥、用户配置或题目数据；原有工作区导入/导出功能与 CI 在线发行独立。

<a id="in-app-update-trust-and-recovery"></a>

## 应用内更新信任与恢复

客户端读取正式 Release 的 `latest.json` 和 `latest.json.sig`，先验证清单签名，再读取版本、平台、镜像 digest 与兼容性。
下载 EXE/AppImage 后再次验证文件签名、SHA256 与大小；不使用 Docker `latest` 决定运行环境。
公钥位于 `src/desktop/update-public.key` 和 Tauri 配置，必须保持一致。私钥存于 Actions Secret，并由管理员在仓库外备份；丢失密钥不能通过重新生成密钥为已有客户端无缝更新。
更新签名不替代 Windows Authenticode；当前安装文件没有系统代码签名。

更新的检查、准备、切换和确认规则见[桌面更新设计](desktop-updates.zh-CN.md)。手动演练签署候选产物，但不修改正式 Release 或应用稳定更新入口。

<a id="recover-publication-of-accepted-artifacts"></a>

## 已验收产物的发布恢复

如果 tag 发布已完成构建、原生安装器检查、容器验收及 package，但 publish 被跳过或发布操作临时失败，可以在 master 手动执行 **Recover Accepted Publication**，填写原始 tag 发布 run ID。执行此入口表示批准发布该已有版本。
入口只接受同仓库 tag push 的已结束运行，逐项要求验收 job 成功，固定该尝试上传的 online-release artifact ID，并核对 tag 未移动、提交仍属于 master。下载原始安装文件、签名和验收清单后，使用原提交的发布逻辑校验并发布原始字节和镜像 digest。
此入口不编译、不重新签名、不移动或新建 tag，不需要更新签名私钥。已发布且内容不同的附件或固定镜像仍拒绝覆盖。产物过期或缺失时不能使用此入口。

完整重跑导致来源 artifact ID 返回 404 时，客户端和安装器会正常回退构建；这不是跳过校验或复用错误。Docker 候选镜像存于 registry，不随 Actions 产物删除，因此仍可按候选 digest 复用。

## 发布描述

发布描述从仓库内已提交的内容生成，不依赖可能变化的 PR 正文。PR 只需英文；面向用户的修改按[提交指南](commits.zh-CN.md)增加经评审的双语 `.changes/<slug>.toml` 片段。上一个可达稳定 tag 之后新增的片段用于版本亮点；npm 锁文件固定的 `git-cliff` 生成技术变更列表，保留不符合规范的历史提交，排除合并和版本准备产生的干扰条目。不兼容变化即使属于通常被过滤的类别也会保留。此流程不代表 AI 翻译或语义验证。

打包任务检出完整历史、执行 `npm ci`，先将准确的源码、版本、范围、片段和变更列表固定到 `release-content.json`，再生成并签名 `latest.json`。已签名目录包含限制在 32 KiB 内的双语亮点、必要时指向完整说明的链接，以及绑定完整快照的 SHA256。应用内更新与 GitHub Release 复用同一份描述。发布器无需安装 git-cliff 或重新查询 GitHub PR，重试和已验收产物的发布恢复均渲染固定快照。不重写既有正式 Release。

页面先展示亮点、经核对的 Windows/Linux 安装程序链接和大小，再展示升级指引与验证限制。中文内容和原始技术详情可以展开。链接、大小和 SHA256 来自已验收安装程序；源码、镜像标识、验收状态、已签名目录中的描述在发布前必须一致。预览不声称已验收，也不提供未经验证的下载链接。`release-content.json` 是新增的受校验和覆盖的 Release 附件；版本 PR 只改五个文件的规则、固定版本镜像和不可变附件保护均保持有效。

执行 `npm ci` 后可在本地预览：

```sh
python -m scripts.release.release_notes --output target/release-preview.md
```

仅从已提交的 Git 树读取内容。需先提交片段，暂存或未暂存的修改均不会纳入。对应 JSON 快照写入 Markdown 旁边。生成预览不会发布 tag、镜像或 Release。
