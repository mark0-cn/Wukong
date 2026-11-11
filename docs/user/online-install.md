# 安装与应用内更新

[English](../en/user/online-install.md) · [语言设置](languages.md)

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
应用内点击“安装到应用菜单并重启”可复制到用户应用目录并添加桌面入口。它是用户级安装，不使用 sudo。
安装 Docker Engine，确认当前用户能访问 Docker，再点击“准备工具环境”。不会自动安装 Docker、调整用户组或修改系统服务。

## 环境与数据

- 工具镜像 `hlmark0/wukong` 公开，可以匿名拉取；遇到 Docker Hub 限流可自行登录自己的账号。
- 每个版本清单绑定固定镜像 digest，不因 Docker `latest` 变化而改变运行环境。
- 为 Docker 镜像及临时层至少预留 25 GiB。客户端会检查下载目录空间；Docker Desktop 虚拟磁盘容量仍需在其设置中检查。
- 新桌面安装的数据位于 Windows `%LOCALAPPDATA%/WukongData` 或 Linux `$XDG_DATA_HOME/Wukong`，Linux 默认 `~/.local/share/Wukong`。
- 显式 `--config` 和已有便携配置继续优先；配置、数据库与题目文件不随安装器替换。
- 未准备好工具环境时，可以查看历史和编辑配置；需要 worker 的任务不能运行。

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
4. 点击“安装并重启”，确认退出。CLI 中仍有分析运行时，切换会被拒绝。
5. 新客户端启动后验证配套环境，通过后确认更新；否则保留恢复记录并阻止新任务启动。

题目容器和题目镜像不会跟随工具镜像升级。当前只接受兼容的数据格式，不执行不兼容数据库迁移。
网络失败后可重试：已验证的安装文件和 Docker 镜像层可复用；客户端下载中断的临时文件会重新下载。

## 修复、恢复与清理

“工具环境 → 自检并修复环境”使用当前客户端版本的正式签名清单，重新准备工具环境，不升级客户端。

更新前会保存独立恢复程序：

- Windows：`%LOCALAPPDATA%/WukongData/updates/Wukong-Recovery.exe`
- Linux：`~/.local/share/Wukong/updates/Wukong-Recovery.AppImage`（遵循 XDG_DATA_HOME）

在“更新 → 维护选项”中点击“恢复上一版本”，或在无法打开应用时先关闭 Wukong，再运行该文件。恢复需要 Docker 可用，但不需要重新联网下载安装文件。恢复失败会保留记录供重试；镜像回滚不撤销题目数据变化。

可以分别清理旧客户端下载缓存和无容器引用的旧工具镜像。当前版本、上一恢复版本、待安装版本均受保护，不执行全局 Docker prune。
卸载客户端不默认删除分析数据或 Docker 资源；Linux 用户级桌面入口位于用户数据目录的 `applications/io.wukong.desktop.desktop`。

## 验证边界

CI 验证 Windows 安装/卸载、Linux AppImage 与真实 WebView 启动、签名完整性及 Linux Docker 集成。
Windows Docker Desktop 的真实首次安装与跨版本升级仍需桌面实机验收；托管 runner 的安装器检查不代替这项验收。
