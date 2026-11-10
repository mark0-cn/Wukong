# 桌面安装与更新

Windows 使用当前用户 NSIS EXE，Linux 使用 AppImage。平台配置由 Tauri 自动合并；在线安装不内置 Docker 镜像。
本地构建：`python -m scripts.release.build_installer --platform windows`（Linux 改为 `linux`）。
签名发布使用 `--signed`，要求提供 `TAURI_SIGNING_PRIVATE_KEY`。

新桌面安装将配置和数据放在 Windows `%LOCALAPPDATA%/Wukong` 或 Linux `$XDG_DATA_HOME/Wukong`（默认 `~/.local/share/Wukong`）。
显式配置、已有便携配置继续优先；卸载程序不默认删除分析数据或 Docker 资源。
Linux 支持范围以桌面安装验收矩阵为准，AppImage 不承诺兼容所有发行版。
