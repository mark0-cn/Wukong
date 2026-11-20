<a id="local-release-builds"></a>

# 本地发布构建

[English](local-builds.md) · 简体中文

`cargo package-local` 生成的是**本地发布（local-release）**桌面安装包：Windows NSIS EXE 或 Linux AppImage。首次运行时，它使用本机已构建的 Docker 镜像，或在引导阶段拉取按版本固定的 CI 工作台镜像，完成“准备工具环境”引导，而不读取线上签名的 `latest.json` 目录。它用于端到端验证尚未发布的提交，也可在另一台机器上使用。

<a id="why-it-exists"></a>

## 为什么需要它

线上首次引导会下载当前客户端版本的官方 `latest.json`，并要求签名目录中的 `runtime_protocol`/`data_schema` 与本构建一致。因此，未发布的提交，或协议/数据模式已领先于已发布清单的分支，无法准备环境，只会报 `unsupported release format, runtime protocol or data migration`。本地发布构建仅替换这一处获取逻辑，环境准备、自检与激活仍走原有流程。

<a id="build-one"></a>

## 构建方式

```sh
cargo package-local --check
cargo package-local --output target/installers/local-release-1
```

`--check` 校验源码与工具入口；在 Windows 上还会发现 x64 MSVC/SDK 环境，把临时文件指向 `target/tmp`，并实际编译、归档和链接一个 SDK 小程序。实际构建会运行 `npm ci`；在 Windows 上构建 Worker 安装程序；解析本地 Docker 镜像；写入 `target/local-release.json` 并作为 Tauri 资源 `local-release.json` 打包；最后以 `custom-protocol` 与 `local-release` Cargo feature 构建客户端。同一个已初始化的 MSVC 环境会传给 Worker 和随后的 Tauri/桌面子进程；独立诊断工具链时使用 `python -m scripts.dev.msvc_cargo --check`。描述文件还会记录 `image_registry`，即另一台机器引导时可按需拉取的按版本固定的 CI 工作台镜像。重复运行会替换只包含旧 `package-local` 安装包的 `--output` 目录；含无关文件或子目录的目录会被拒绝而不会被删除。`--signed` 仍可用于 updater 签名测试，但已签名的本地发布包依然是开发产物，不会被发布。`--online` 则生成旧的、依赖线上目录的安装包。

CI 发布打包不使用此模式：`scripts/release/build_installer.py` 默认仍是线上配置，发布工作流都不会启用 `local-release` feature。使用 `tauri dev` 或启用该 feature 的 `cargo run` 时，Tauri 可能不会把内置资源复制到开发可执行文件旁；可把 `WUKONG_LOCAL_RELEASE_JSON` 指向描述文件的绝对路径以走同一运行路径。

<a id="docker-image-requirement"></a>

## Docker 镜像要求

打包主机必须已有可供工作台使用的 linux/amd64 镜像。`cargo package-local` 按以下顺序解析：

1. 设置了 `WUKONG_LOCAL_IMAGE` 时：精确打包该标签；标签不存在时依据 `docker-bake.hcl` 构建。这仍是显式覆盖手段。
2. 从 Docker Hub 拉取 CI 构建的 `<namespace>/wukong-workbench:local-<version>`，校验为 linux/amd64 后标记为 `wukong:local`，并覆盖更旧的本地标签（见下一节）。
3. 仅在拉取无法进行（标签不存在、缺少认证或网络失败）时，才使用本机已存在的 `wukong:local` 标签。
4. 本地执行 `workbench` 目标的 `docker buildx bake`。

`WUKONG_LOCAL_IMAGE_REGISTRY` 可覆盖工作台仓库，默认是 `hlmark0/wukong-workbench`。该仓库刻意不同于正式发布镜像 `hlmark0/wukong`，因此本地打包绝不会拉取发布标签或可移动的 `latest` 标签；正式仓库路径会被拒绝。拉取的标签是 `local-<version>`，由当前 `Cargo.toml` 版本生成，因此请从相同版本的检出拉取。仅当 Docker Hub 仓库为私有才需要 `docker login`；公开仓库可匿名拉取。CI 镜像优先于陈旧的本地 `wukong:local` 标签：拉取成功时打包会覆盖该标签；只有拉取无法进行时才复用该本地标签，并警告它可能是较旧的构建；删除它（`docker image rm wukong:local`）会让该兜底改为本地 Bake 构建。

引导过程会校验本地镜像的平台，并从 Docker 主机读取其真实大小；镜像缺失时按下一节获取。打包后重新构建镜像没有问题，引导只会提示记录的镜像大小已过期。如果打包终端本身无法连接 Docker（例如受限的沙箱终端），打包会记录 `image_size: 0`，仍记录 `image_registry`，并把存在性、平台和大小检查推迟到引导阶段；可设置 `WUKONG_LOCAL_IMAGE_SIZE` 显式记录已知大小。

<a id="run-on-another-machine"></a>

## 在另一台机器上运行安装包

描述文件会记录 `image_registry`（`<registry>/wukong-workbench:local-<version>`，默认 `hlmark0/wukong-workbench`），即使打包主机无法连接 Docker 也会记录，因此在某台机器上构建的安装包无需预先存在的本地标签也能在另一台机器上准备环境：

1. 引导会检查描述文件中的 `image`（`wukong:local`）。若打包时已拉取或构建该标签（例如安装包在打包机器上运行），则直接使用。
2. 标签缺失时，引导以 `docker pull --platform linux/amd64` 拉取 `image_registry`，校验结果为 linux/amd64 且大小非零，再将其标记为 `wukong:local`，使后续准备流程继续使用描述文件中的镜像标签。设置对话框会显示分层下载进度，准备过程可取消。
3. 拉取失败（标签未发布、网络不可用、私有仓库未登录），或描述文件来自旧包而没有 `image_registry` 时，引导会停止，并给出重试前需执行的确切命令：`docker pull --platform linux/amd64 <image_registry>`、`docker tag <image_registry> wukong:local`，以及本地 `docker buildx bake --load --set workbench.tags=wukong:local`。正式 `hlmark0/wukong` 发布仓库在描述文件校验阶段即被拒绝，绝不会被拉取。

因此，第二台机器需要 Docker（Linux 容器）、可访问镜像仓库的网络，以及足够的镜像空间：大约需下载 4 GiB，磁盘占用约 10.3 GB。仅当仓库为私有时才需要 `docker login`。若记录的 `image_size` 与实际拉取大小不同，只会警告镜像在打包后重新构建过。离线机器请先在联网主机上 `docker pull` 并 `docker tag`，再用 `docker save`/`docker load` 传输镜像，或使用打印的 Bake 命令在本地构建。安装包仍然不会内嵌镜像、仓库凭据或签名私钥。

<a id="ci-built-workbench-image"></a>

## CI 构建的工作台镜像

本地构建工作台镜像需要编译庞大的工具镜像，因此维护者按客户端版本用**Actions → Workbench image** 手动工作流发布一次，而不是让每个构建者分别编译：

1. 把 `version` 设为所选 ref 的 `Cargo.toml` 已声明的客户端 MAJOR.MINOR.PATCH 版本；输入与检出不一致时工作流会失败。
2. 可选地把 `ref` 设为要构建的分支、标签或提交；默认是启动运行时所选的 ref。
3. 运行。工作流会校验锁定的构建输入，构建与本地相同的 `workbench` Bake 目标，检查载入镜像为 linux/amd64 且工具导入可用，然后推送 `<DOCKERHUB_NAMESPACE>/wukong-workbench:local-<version>`。运行摘要记录操作者、源码 ref、提交、digest 和大小。它绝不会推送正式的 `<namespace>/wukong` 镜像，也不会创建或移动 `latest` 标签；它不是正式发布。

构建者随后只需运行 `cargo package-local`，它会拉取匹配的 `local-<version>` 标签并标记为 `wukong:local`，覆盖更旧的本地标签。描述文件记录同一引用，因此在另一台机器上使用安装包时会在引导阶段重复该拉取（[在另一台机器上运行安装包](#run-on-another-machine)）。不兼容的变更通过递增客户端版本处理：`runtime_protocol`/`data_schema` 变化需要新版本，也就需要新的不可变 `local-<version>` 标签（工作流拒绝覆盖已存在的版本标签）。如果拉取失败——标签缺失、私有仓库未登录或网络不可达——打包会打印维护者工作流和本地 Bake 命令，然后在本机已有 `wukong:local` 标签时复用它，否则回退到本地构建。一次性 Docker Hub 配置是在发布账号下创建独立的 `wukong-workbench` 仓库；复用已有的 `DOCKERHUB_NAMESPACE`/`DOCKERHUB_USERNAME` 变量和具备写权限的 `DOCKERHUB_TOKEN` secret，运行不需要 environment 审核人或操作者白名单。

<a id="windows-worker-media-template"></a>

## Windows Worker 媒体模板

若 `target/worker-media-template-gui.zip` 存在（或 `WUKONG_LOCAL_TEMPLATE` 指向一个模板归档），打包会把其解析后的绝对路径记录为 `template_path`。Windows Worker 卡片随后直接使用该模板，不再下载签名发布资产；共享媒体生成器仍会校验归档。未打包模板时该字段省略，界面照常允许选择本地文件。该路径只在打包主机上有效，换机使用前需移动或重新构建模板。

<a id="limitations"></a>

## 限制

- 仅用于开发。不要把本地发布安装包当作正式发布分发、发布或签名；CI 从不生成此类安装包。
- 不支持在线更新：更新检查仍读取签名目录，并可能因已发布版本不兼容而失败。需要新的本地构建时请重新执行 `cargo package-local`。
- 描述文件没有签名。其信任边界就是本地构建；它只能把引导指向本地镜像，绝不会替代线上更新身份。
- 被接受的 Windows Worker 模板路径是主机绝对路径，不能直接迁到另一台机器。
- 第二台机器会在引导阶段下载工作台镜像，因此需要 Docker、镜像仓库网络访问和磁盘空间；离线机器须先用 `docker save`/`docker load` 传输已拉取的镜像，或用打印的 Bake 命令在本地构建。
- updates/status 负载带有 `local_build: true`，便于界面以后标记开发构建。
