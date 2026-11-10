# Wukong 在线安装

支持 Windows x64 客户端和 linux/amd64 工具镜像。安装包不包含镜像或模型密钥。

1. 安装并启动 Docker Desktop，启用 Linux containers。
2. 工具镜像 `hlmark0/wukong` 为公开仓库，可以匿名拉取。若遇到 Docker Hub 匿名拉取限额，可用自己的 Docker 账号登录；不要使用 CI 发布 Token。
3. 从 GitHub Release 下载在线 zip，核对 `SHA256SUMS`，解压到临时目录。
4. 在 PowerShell 中进入解压目录，执行 `powershell -ExecutionPolicy Bypass -File .\install-workbench.ps1`。
5. 安装器按版本清单的 digest 拉取镜像、检查环境并创建快捷方式。打开客户端后配置模型。

默认安装目录为 `%LOCALAPPDATA%\Programs\Wukong Workbench`。可用 `-Destination` 指定目录，或用 `-DockerExecutable` 指定 Docker CLI。至少为镜像与 Docker 临时层预留 25 GiB；安装器另检查客户端所在盘的空间。Docker Desktop 的虚拟磁盘容量需在其设置中检查。

失败后修复 Docker 登录、网络或磁盘问题，再运行相同安装命令即可。Docker 会复用已下载的层。`-ValidateOnly` 只验证包内校验值，不拉取或安装。

## 更新与回滚

关闭客户端并结束分析任务，将新版包解压到独立临时目录，运行安装脚本并指向相同安装目录。模型配置与工作数据保留；已有题目容器不自动升级。新环境通过 doctor 后才安装客户端。失败时尝试恢复旧环境与客户端。

安装目录的 `.previous` 保存安装备份，`rollback.json` 指向上一次可恢复版本。回滚时关闭客户端，在安装目录运行 `powershell -ExecutionPolicy Bypass -File .\rollback-workbench.ps1`。不要手工删除旧镜像后再回滚。

备份中可能包含本机模型配置，不应分享。当前版本不执行不兼容的数据格式迁移；镜像回滚不等于恢复被修改的题目数据。

## 验证范围

CI 在 Linux 上验证工具、容器通信、GDB 和运行环境，在 Windows 上构建客户端并验证安装脚本。标准 Windows runner 不等同于 Windows Docker Desktop 桌面环境，完整桌面安装联调需在实际电脑确认。

每个包绑定固定镜像 digest，不会因 Docker Hub 的 `latest` 更新而替换正在使用的环境。Windows 可执行文件目前没有 Authenticode 签名。
