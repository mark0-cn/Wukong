<a id="local-release-builds"></a>

# 本地发布构建

[English](local-builds.md) · 简体中文

`cargo package-local` 生成的是**本地发布（local-release）**桌面安装包：Windows NSIS EXE 或 Linux AppImage。首次运行时，它使用本机已构建的 Docker 镜像完成“准备工具环境”引导，而不读取线上签名的 `latest.json` 目录。它用于端到端验证尚未发布的提交。

<a id="why-it-exists"></a>

## 为什么需要它

线上首次引导会下载当前客户端版本的官方 `latest.json`，并要求签名目录中的 `runtime_protocol`/`data_schema` 与本构建一致。因此，未发布的提交，或协议/数据模式已领先于已发布清单的分支，无法准备环境，只会报 `unsupported release format, runtime protocol or data migration`。本地发布构建仅替换这一处获取逻辑，环境准备、自检与激活仍走原有流程。

<a id="build-one"></a>

## 构建方式

```sh
cargo package-local --check
cargo package-local --output target/installers/local-release-1
```

`--check` 只校验源码与工具入口。实际构建会运行 `npm ci`；在 Windows 上构建 Worker 安装程序；解析本地 Docker 镜像；写入 `target/local-release.json` 并作为 Tauri 资源 `local-release.json` 打包；最后以 `custom-protocol` 与 `local-release` Cargo feature 构建客户端。重复运行会替换只包含旧 `package-local` 安装包的 `--output` 目录；含无关文件或子目录的目录会被拒绝而不会被删除。`--signed` 仍可用于 updater 签名测试，但已签名的本地发布包依然是开发产物，不会被发布。`--online` 则生成旧的、依赖线上目录的安装包。

CI 发布打包不使用此模式：`scripts/release/build_installer.py` 默认仍是线上配置，发布工作流都不会启用 `local-release` feature。使用 `tauri dev` 或启用该 feature 的 `cargo run` 时，Tauri 可能不会把内置资源复制到开发可执行文件旁；可把 `WUKONG_LOCAL_RELEASE_JSON` 指向描述文件的绝对路径以走同一运行路径。

<a id="docker-image-requirement"></a>

## Docker 镜像要求

打包主机必须已有可供工作台使用的 linux/amd64 镜像。`cargo package-local` 按以下顺序解析：

1. 设置了 `WUKONG_LOCAL_IMAGE` 时：精确打包该标签；标签不存在时依据 `docker-bake.hcl` 构建。这仍是显式覆盖手段。
2. 本机已存在的 `wukong:local` 标签。
3. 从 Docker Hub 拉取 CI 构建的 `<namespace>/wukong-workbench:local-<version>`，校验为 linux/amd64 后标记为 `wukong:local`（见下一节）。
4. 本地执行 `workbench` 目标的 `docker buildx bake`。

`WUKONG_LOCAL_IMAGE_REGISTRY` 可覆盖工作台仓库，默认是 `hlmark0/wukong-workbench`。该仓库刻意不同于正式发布镜像 `hlmark0/wukong`，因此本地打包绝不会拉取发布标签或可移动的 `latest` 标签；正式仓库路径会被拒绝。拉取的标签是 `local-<version>`，由当前 `Cargo.toml` 版本生成，因此请从相同版本的检出拉取。仅当 Docker Hub 仓库为私有才需要 `docker login`；公开仓库可匿名拉取。若本机已存在 `wukong:local` 标签，它会优先于拉取，需要删除（`docker image rm wukong:local`）才能强制重新拉取。

引导过程会在 Docker 主机上再次检查镜像、读取其真实大小，并在镜像缺失或不是 linux/amd64 时给出包含构建命令的错误。安装包不会内嵌镜像、仓库凭据或签名私钥。打包后重新构建镜像没有问题，引导只会提示记录的镜像大小已过期。如果打包终端本身无法连接 Docker（例如受限的沙箱终端），打包会记录 `image_size: 0`，把存在性、平台和大小检查推迟到引导阶段；可设置 `WUKONG_LOCAL_IMAGE_SIZE` 显式记录已知大小，镜像尚不存在时请在能访问 Docker 的终端中构建。

<a id="ci-built-workbench-image"></a>

## CI 构建的工作台镜像

本地构建工作台镜像需要编译庞大的工具镜像，因此维护者按客户端版本用**Actions → Workbench image** 手动工作流发布一次，而不是让每个构建者分别编译：

1. 把 `version` 设为所选 ref 的 `Cargo.toml` 已声明的客户端 MAJOR.MINOR.PATCH 版本；输入与检出不一致时工作流会失败。
2. 可选地把 `ref` 设为要构建的分支、标签或提交；默认是启动运行时所选的 ref。
3. 运行。工作流会校验锁定的构建输入，构建与本地相同的 `workbench` Bake 目标，检查载入镜像为 linux/amd64 且工具导入可用，然后推送 `<DOCKERHUB_NAMESPACE>/wukong-workbench:local-<version>`。运行摘要记录操作者、源码 ref、提交、digest 和大小。它绝不会推送正式的 `<namespace>/wukong` 镜像，也不会创建或移动 `latest` 标签；它不是正式发布。

构建者随后只需运行 `cargo package-local`；`wukong:local` 不存在时，打包会拉取匹配的 `local-<version>` 标签并使用。不兼容的变更通过递增客户端版本处理：`runtime_protocol`/`data_schema` 变化需要新版本，也就需要新的不可变 `local-<version>` 标签（工作流拒绝覆盖已存在的版本标签）。如果拉取失败——标签缺失、私有仓库未登录或网络不可达——打包会打印维护者工作流和本地 Bake 命令，然后回退到本地构建。一次性 Docker Hub 配置是在发布账号下创建独立的 `wukong-workbench` 仓库；复用已有的 `DOCKERHUB_NAMESPACE`/`DOCKERHUB_USERNAME` 变量和具备写权限的 `DOCKERHUB_TOKEN` secret，运行不需要 environment 审核人或操作者白名单。

<a id="windows-worker-media-template"></a>

## Windows Worker 媒体模板

若 `target/worker-media-template-gui.zip` 存在（或 `WUKONG_LOCAL_TEMPLATE` 指向一个模板归档），打包会把其解析后的绝对路径记录为 `template_path`。Windows Worker 卡片随后直接使用该模板，不再下载签名发布资产；共享媒体生成器仍会校验归档。未打包模板时该字段省略，界面照常允许选择本地文件。该路径只在打包主机上有效，换机使用前需移动或重新构建模板。

<a id="limitations"></a>

## 限制

- 仅用于开发。不要把本地发布安装包当作正式发布分发、发布或签名；CI 从不生成此类安装包。
- 不支持在线更新：更新检查仍读取签名目录，并可能因已发布版本不兼容而失败。需要新的本地构建时请重新执行 `cargo package-local`。
- 描述文件没有签名。其信任边界就是本地构建；它只能把引导指向本地镜像，绝不会替代线上更新身份。
- 被接受的 Windows Worker 模板路径是主机绝对路径，不能直接迁到另一台机器。
- updates/status 负载带有 `local_build: true`，便于界面以后标记开发构建。
