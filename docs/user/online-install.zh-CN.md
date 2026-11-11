<a id="installation-and-in-app-updates"></a>

# 安装与应用内更新

[English](online-install.md) · 简体中文

[语言设置](languages.zh-CN.md)

Wukong 提供 Windows x64 安装 EXE 和 Linux x64 AppImage。安装文件包含客户端，工具镜像在首次运行时联网下载；不包含模型密钥或题目数据。

## Windows

1. 从正式 GitHub Release 下载 `Wukong_<版本>_x64-setup.exe`（以附件实际文件名为准）。
2. 双击后选择 English 或 SimpChinese，安装到当前用户目录并打开 Wukong。客户端使用 WebView2，安装器可在线准备缺失的运行时。应用语言默认跟随系统，可在设置中独立修改。
3. 首次启动打开环境向导。安装并启动 Docker Desktop，选择 Linux containers。
4. 点击“准备工具环境”，等待镜像下载、自检完成，然后在设置中配置模型。

当前 EXE 未做 Authenticode 签名。应用内更新使用独立的签名校验机制。

## Linux

支持 x86_64 桌面环境，正式构建基线为 Ubuntu 22.04。下载 `.AppImage` 后赋予执行权限并运行：

```sh
chmod +x Wukong_*.AppImage
./Wukong_*.AppImage
```

系统需要支持 AppImage 运行；缺少 FUSE 时可安装发行版对应的 FUSE 兼容库，或使用 `APPIMAGE_EXTRACT_AND_RUN=1 ./Wukong_*.AppImage`。
准备工具环境后，在“更新 → 维护选项”点击“安装到应用菜单并重启”可复制到用户应用目录并添加桌面入口。它是用户级安装，不使用 sudo。
安装 Docker Engine，确认当前用户能访问 Docker，再点击“准备工具环境”。不会自动安装 Docker、调整用户组或修改系统服务。

<a id="first-setup-and-diagnostics"></a>

## 首次准备与错误处理

首次准备按“检查 Docker → 下载工具镜像 → 自检并启用”显示进度；客户端版本和工具环境状态分开显示。首次准备页只保留准备/重试与 Docker 安装说明，目录配置在“工作区设置 → 存储位置”，清理和恢复在“更新 → 维护选项”（待恢复时也会显示）。

失败时显示一次错误，可复制错误，并展开 Docker 日志末尾输出；完整日志位置包含在 Docker 长操作的错误中。如果提示找不到 `docker-credential-desktop`，先完全退出并重新打开 Wukong；仍失败则检查 Docker Desktop 安装及凭据助手所在目录的 PATH。Wukong 会在可确认的安装目录中查找 CLI 和同级助手，不删除登录配置或禁用凭据存储。此错误与数据目录的 Windows 扩展路径前缀无关。

拉取时可查看已就绪/已发现镜像层数、耗时和每层状态。层大小不同，层数不代表下载字节百分比；拉取完成后还需通过环境自检。镜像按签名清单中的 digest 拉取，因此 Docker Desktop 本地标签可能显示 `<none>`，这不影响版本固定；客户端会显示对应的工具镜像版本。

<a id="environment-and-data"></a>

## 环境与数据

- 工具镜像 `hlmark0/wukong` 公开，可以匿名拉取；遇到 Docker Hub 限流可自行登录自己的账号。
- 每个版本清单绑定固定镜像 digest，不因 Docker `latest` 变化而改变运行环境。
- 为 Docker 镜像及临时层至少预留 25 GiB。客户端会检查下载目录空间；Docker Desktop 虚拟磁盘容量仍需在其设置中检查。
- 默认任务与证据位于下列用户数据根目录中的 `.wukong`，更新缓存位于 `updates`；Windows 界面显示常规驱动器/UNC 路径，底层路径身份保留。
- 新桌面安装的数据位于 Windows `%LOCALAPPDATA%/WukongData` 或 Linux `$XDG_DATA_HOME/Wukong`，Linux 默认 `~/.local/share/Wukong`。
- 显式 `--config` 和已有便携配置继续优先；配置、数据库与题目文件不随安装器替换。
- 固定用户数据根目录中的 `preferences.json` 独立于工作区配置保存语言偏好。
- 未准备好工具环境时，可以查看历史和编辑配置；需要 worker 的任务不能运行。

<a id="updates"></a>

## 更新

在“工作区设置 → 存储位置”中可以修改：

- **任务与证据目录**：切换到已有或新的工作区。不会自动搬移原有分析数据，选择原目录即可切回。
- **更新缓存目录**：可选择其他磁盘上的空目录。已有签名清单、安装文件和恢复所需文件会经校验后复制；全部成功后才切换位置，原文件保留。

默认位置仍在上述用户数据目录中。配置入口、维护锁和恢复助手保持固定位置，避免切换缓存后找不到恢复记录。
Docker 镜像及卷的物理磁盘位置由 Docker Desktop / Docker Engine 设置管理，不属于客户端下载缓存。
正在分析、下载或等待更新恢复时不能修改相关目录；使用命令行 `--state` 指定目录时需重新启动后再从界面切换。

应用启动后检查正式版本；检查失败不影响已经可用的环境。打开“更新”：

1. 点击“检查更新”，查看版本说明。
2. 结束当前分析、调试及文件操作，点击“下载并验证更新”。当前实现保守地要求桌面空闲后开始准备；下载不会替换现用环境。
3. 下载新版安装文件、上一版恢复安装文件及配套镜像，验证签名，并在独立容器中自检。
4. 保存草稿，点击“安装并重启”，确认退出。CLI 中仍有分析运行时，切换会被拒绝。
5. 新客户端启动后验证配套环境，通过后确认更新；否则保留恢复记录并阻止新任务启动。

题目容器和题目镜像不会跟随工具镜像升级。当前只接受兼容的数据格式，不执行不兼容数据库迁移。
网络失败后可重试：已验证的安装文件和 Docker 镜像层可复用；客户端下载中断的临时文件会重新下载。

<a id="repair-recovery-and-cleanup"></a>

## 修复、恢复与清理

“工具环境 → 自检并修复环境”使用当前客户端版本的正式签名清单，重新准备工具环境，不升级客户端。

更新前会保存独立恢复程序：

- Windows：`%LOCALAPPDATA%/WukongData/updates/Wukong-Recovery.exe`
- Linux：`~/.local/share/Wukong/updates/Wukong-Recovery.AppImage`（遵循 XDG_DATA_HOME）

在“更新 → 维护选项”中点击“恢复上一版本”，或在无法打开应用时先关闭 Wukong，再运行该文件。恢复需要 Docker 可用，但不需要重新联网下载安装文件。恢复失败会保留记录供重试；镜像回滚不撤销题目数据变化。

可以分别清理旧客户端下载缓存和无容器引用的旧工具镜像。当前版本、上一恢复版本、待安装版本均受保护，不执行全局 Docker prune。
卸载客户端不默认删除分析数据或 Docker 资源；Linux 用户级桌面入口位于用户数据目录的 `applications/io.wukong.desktop.desktop`。

<a id="validation-boundary"></a>

## 验证边界

CI 验证 Windows 安装/卸载、Linux AppImage 与真实 WebView 启动、签名完整性及 Linux Docker 集成。
Windows Docker Desktop 的真实首次安装与跨版本升级仍需桌面实机验收；托管 runner 的安装器检查及语言界面浏览器测试不代替这项验收。

清理 CLI 可以预览应用管理的本机数据，不初始化 Store、模型或 worker；旧版 SQLite 归属检查仅使用不可变只读连接。运行 `wukong cleanup plan --data --output cleanup-review.json`，核对路径、文件数量、大小和保留项，再显式执行 `wukong cleanup execute --review cleanup-review.json --yes --report cleanup-report.json`。这些文件应保存在待清理目录之外。执行时获取维护锁并重新生成清单；内容变化后需要重新预览。未知文件、符号链接和重解析路径会保留；配置指向一个目录不代表允许删除该目录中的所有内容。

Windows 卸载会打开范围预览，本机数据与设置、Docker 工具环境、Docker 题目环境分别选择，Docker 选项默认不勾选。原生数据复选框仅设置初始数据选项，详细预览中的最终选择为准。预览展示实际目录、本机文件大小、归属 Docker 资源及保留原因；取消后程序仍安装。静默卸载默认仅移除程序；显式自动化参数为 `/WUKONG_DELETE_DATA`、`/WUKONG_DELETE_TOOLS` 和 `/WUKONG_DELETE_CHALLENGES`。`/UPDATE` 和被动升级路径忽略清理请求。Linux 的 `wukong cleanup uninstall` 打开相同预览，从已安装的 AppImage 启动时移除受管理的桌面集成；外部下载的便携 AppImage 保留供手动移除。

数据清理包含受管理的任务、对话/检查点/证据文件、生成产物、设置与偏好、框架浏览器数据、本地 worker/Ghidra 缓存与日志、已验证的更新/恢复文件及已记录的自定义目录。未知同级文件、外部原始附件、导出文件、外部自定义 Skill 源文件、进程环境变量 API Key、Docker 凭据、共用运行时和远程环境仍保留。无法确认归属的旧版外部缓存目录也会保留。保留的 Docker 环境会留下最小工作区/目录/启动记录及导入的题目副本；被保留容器引用的镜像、卷和网络也不会删除。本机文件大小合计不包含 Docker 磁盘占用。

CLI 的 `cleanup plan` 还可添加 `--tools` 和 `--challenges`。清理仅支持本地 `unix://`/`npipe://` Docker socket；预览与执行之间不要切换 Docker context。请先完成或取消活动任务，并确认或恢复待完成的更新。已有本地 worker 通过认证后请求关闭；仍有活动 job 时会拒绝。清理获取维护锁和工作区锁、重新核对身份，先删所选容器，再删未共用的卷/网络/镜像，最后移除配置与归属记录。失败会生成未完成报告，原生卸载会保留程序；部分已选文件或资源可能已经删除。修复报告中的原因后重试。原生清理中断时阻止新任务，直到重试卸载器。不会全局 prune，也不删除共用工具。
