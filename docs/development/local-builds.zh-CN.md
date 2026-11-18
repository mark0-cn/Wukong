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

`--check` 只校验源码与工具入口。实际构建会运行 `npm ci`；在 Windows 上构建 Worker 安装程序；解析本地 Docker 镜像；写入 `target/local-release.json` 并作为 Tauri 资源 `local-release.json` 打包；最后以 `custom-protocol` 与 `local-release` Cargo feature 构建客户端。`--signed` 仍可用于 updater 签名测试，但已签名的本地发布包依然是开发产物，不会被发布。`--online` 则生成旧的、依赖线上目录的安装包。

CI 发布打包不使用此模式：`scripts/release/build_installer.py` 默认仍是线上配置，发布工作流都不会启用 `local-release` feature。使用 `tauri dev` 或启用该 feature 的 `cargo run` 时，Tauri 可能不会把内置资源复制到开发可执行文件旁；可把 `WUKONG_LOCAL_RELEASE_JSON` 指向描述文件的绝对路径以走同一运行路径。

<a id="docker-image-requirement"></a>

## Docker 镜像要求

打包主机必须已有可供工作台使用的 linux/amd64 镜像。默认标签为 `wukong:local`；可用 `WUKONG_LOCAL_IMAGE` 指定其他标签（例如源码上手文档中的开发标签 `wukong:0.1`）。标签不存在时，打包会依据 `docker-bake.hcl` 构建：

```sh
python -m scripts.toolchain.fetch_worker_deps
docker buildx bake --load --set workbench.tags=wukong:local
```

引导过程会在 Docker 主机上再次检查镜像、读取其真实大小，并在镜像缺失或不是 linux/amd64 时给出包含构建命令的错误。镜像永远不会从仓库拉取，安装包也不会内嵌镜像、仓库凭据或签名私钥。打包后重新构建镜像没有问题，引导只会提示记录的镜像大小已过期。

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
