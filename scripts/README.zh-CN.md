<a id="repository-automation"></a>

# 仓库自动化

[English](README.md) · 简体中文

Python 入口从仓库根目录使用 `python -m` 调用，避免依赖脚本同目录导入或调用者的 `PYTHONPATH`。

本地 Windows / Linux 原生打包使用 `cargo package-local`，由独立 `xtask/` launcher 调用 `scripts.release.native_package`。参数和产物见[发布文档](../docs/development/releases.zh-CN.md#本地生成原生安装文件)。

| 目录 | 职责 / 入口示例 |
| --- | --- |
| `ci/` | 范围选择、运行摘要、关闭 PR 缓存回收及 runner 磁盘准备 |
| `release/` | `python -m scripts.release.cli check`；版本、打包、发布与产物复用 |
| `toolchain/` | `python -m scripts.toolchain.fetch_worker_deps`；附件下载与更新检查 |
| `dev/` | PowerShell 开发检查、发布本机 GUI、启动独立 worker |
| `_lib/` | 仓库定位、版本/哈希规则、GitHub API 等公共实现 |

PowerShell 包装入口会定位仓库；用户提供的路径按调用时的工作目录解释。
原生安装器由 Tauri 平台配置生成；`installer_artifact` 的安装/卸载冒烟测试只在隔离 runner 中运行，避免改动开发机已有安装。`packaging/windows` 中的脚本用于保留的开发夹具。
脚本与安装器测试位于 [tests/automation](../tests/automation)，真实镜像验收位于 [tests/acceptance](../tests/acceptance)。

```sh
python -m unittest discover -s tests/automation -p 'test_*.py'
```
