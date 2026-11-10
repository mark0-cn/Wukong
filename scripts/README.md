# 仓库自动化

Python 入口从仓库根目录使用 `python -m` 调用，避免依赖脚本同目录导入或调用者的 `PYTHONPATH`。

| 目录 | 职责 / 入口示例 |
| --- | --- |
| `ci/` | 范围选择、运行摘要、关闭 PR 缓存回收及 runner 磁盘准备 |
| `release/` | `python -m scripts.release.cli check`；版本、打包、发布与产物复用 |
| `toolchain/` | `python -m scripts.toolchain.fetch_worker_deps`；附件下载与更新检查 |
| `dev/` | PowerShell 开发检查、发布本机 GUI、启动独立 worker |
| `_lib/` | 仓库定位、版本/哈希规则、GitHub API 等公共实现 |

PowerShell 包装入口会定位仓库；用户提供的路径按调用时的工作目录解释。
用户安装器源码位于 [packaging/windows](../packaging/windows)，发行包中的安装器名称保持不变。
脚本与安装器测试位于 [tests/automation](../tests/automation)，真实镜像验收位于 [tests/acceptance](../tests/acceptance)。

```sh
python -m unittest discover -s tests/automation -p 'test_*.py'
```
