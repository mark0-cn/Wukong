# 共用 Python 分析

[English](README.md) · 简体中文

`wukong_analysis` 接收明确的字节、参数或文件路径并返回分析结果，包含 classic-BPF 解码/探测、ELF libc 候选比较、Unicorn 模拟与崩溃数据解释。它不发现 Worker、管理租约/作业、启动目标进程或持久化执行记录。调用方须先准入并约束输入路径；Linux 适配器保留系统库选择及真实调试操作。

通过 `python -m pip install ./workers/analysis` 安装。BPF 和崩溃位置归一化仅使用标准库。ELF/core 检查、循环模式解释与模拟使用 Linux 分析工具链（pyelftools、pwntools 和 Unicorn）；导入包不会加载这些可选库。Linux 镜像将该包与 Worker 依赖、分析工具链一起安装。Windows 当前仅通过一次性隔离辅助程序开放 BPF 分析，按 `workers/windows/python-components.json` 显式源码清单打包，不携带 Linux Worker 依赖。

Apache-2.0 许可随 `LICENSE` 提供。安装后的包源码参与 Linux 运行时身份计算，原生包源码身份也包含本目录。Linux 测试入口为 `python workers/tests/run.py unit -p 'test_*.py'`；Windows 组件测试核对实际复制清单，仅使用该清单和标准库执行 BPF 分析。这些测试不能替代真实容器或已安装 Worker 验收。
