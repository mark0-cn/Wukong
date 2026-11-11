# 桌面安装与更新

Windows 使用当前用户 NSIS EXE，Linux 使用 AppImage。平台配置由 Tauri 自动合并；在线安装不内置 Docker 镜像。
本地构建：`python -m scripts.release.build_installer --platform windows`（Linux 改为 `linux`）。
签名发布使用 `--signed`，要求提供 `TAURI_SIGNING_PRIVATE_KEY`。

新桌面安装将配置和数据放在 Windows `%LOCALAPPDATA%/WukongData` 或 Linux `$XDG_DATA_HOME/Wukong`（默认 `~/.local/share/Wukong`）。
显式配置、已有便携配置继续优先；卸载程序不默认删除分析数据或 Docker 资源。
Linux 支持范围以桌面安装验收矩阵为准，AppImage 不承诺兼容所有发行版。

## 信任与状态

`updates/manifest.rs` 验证清单和文件签名、版本、大小、SHA256、固定镜像 digest、运行协议与数据 schema。
发布目录中的 `latest.json` 同时供 Tauri 和环境管理使用；应用先验证其独立签名，再将同一份内容传给 Tauri `restore_update`，避免两次查询得到不同版本。
清单仅允许当前项目对应版本的 HTTPS 附件和公开 `hlmark0/wukong` digest。签名密钥不是模型密钥，也不进入镜像。

`desktop/updates.rs` 处理检查、下载、独立候选自检、安装和 GUI 进度。准备阶段不替换现用 worker。
安装前缓存并验证当前版本安装文件，原子保存恢复助手与 `updates/pending.json`。新进程只有在版本匹配且配套环境 doctor 通过后才将记录转为 `previous.json`。
未确认更新阻止新任务启动；`maintenance.lock` 以共享/独占文件锁协调 GUI、CLI 和更新操作。
模型设置及题目环境不参加版本替换；只接受数据 schema 3，不执行不兼容数据库迁移。

`updates/recovery.rs` 不依赖 WebView：Windows 使用经过验证的上一版 NSIS 安装器，Linux 原子替换 AppImage。
恢复后检查实际客户端版本，失败时保留记录。用户通过独立恢复程序确认恢复，不在超时后擅自覆盖仍可能运行的安装器。
恢复配置时只更新工具环境字段，保留用户模型设置。恢复不撤销题目数据变化。

下载缓存清理保留当前、上一恢复与待安装版本；镜像清理还排除任何容器引用和受保护版本共用的 digest，不执行 Docker prune。
Docker 下载按阶段展示；客户端按字节显示进度。客户端临时下载失败重新下载，Docker 层可断点复用。

## 可配置存储位置

任务/证据使用已有的 `state_dir` 配置；界面切换通过配置事务重开工作区，不自动搬移旧数据库。
大体积更新文件通过 `updates/cache-location.json` 指向可配置缓存目录，锁、恢复助手及 pending/previous 记录仍在固定控制目录。
切换缓存获取独占 cache/maintenance 锁，要求空目标目录且与原缓存无包含关系，验证签名后复制清单和安装文件，最后原子写入位置指针。
网络准备和恢复持有共享缓存锁，防止下载途中切换目录。旧缓存保留；用户确认后可手工清理原目录。
镜像和 Docker 卷的位置仍由 Docker 管理。

## 发布与验证

`build-installer.yml` 生成并签名 NSIS/AppImage，Windows 和 Linux 均复用已验证的桌面 Release 二进制；Linux 桌面构建与打包使用 Ubuntu 22.04，验收 CLI 是独立产物。
原生安装/卸载只在隔离 runner 中执行；Linux 用 Xvfb 启动真实 WebView。
`desktop_manifest` 汇总镜像验收和安装文件，`wukong verify-update --directory dist` 校验整个发行集。
正式 Release 发布完成后才推进稳定入口；手动演练不改变用户更新通道。

本地检查覆盖真实签名篡改拒绝、维护互斥、清单来源、前端按钮流程和布局。
Linux Docker 验收与原生包冒烟检查分开报告；不能将它们称为 Windows Docker Desktop 的完整跨版本升级实机验收。
