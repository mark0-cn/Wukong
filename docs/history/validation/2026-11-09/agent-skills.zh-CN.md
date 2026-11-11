<a id="agent--skill-capability-changes--2026-11-09"></a>

# Agent / Skill 能力改动 — 2026-11-09

[English](agent-skills.md) · 简体中文

历史记录：已实现角色工具可见性、自定义 Skill 发现/元数据、有界方法选择、不可变通用验证记录、结构化方法产物、复用差分测试、第二目标 Ghidra 查询、分页指令/基本块证据及配对实验评分。

当时 Windows 验证：

- `cargo test --offline --no-default-features --lib --tests`：10 套中 140 通过、6 忽略；忽略项需要外部后端。
- `cargo clippy --offline --no-default-features --all-targets -- -D warnings`：通过。
- `cargo fmt --all -- --check`、`git diff --check`：通过。
- `python workers/runtime/test_bridge.py`：20 通过、4 平台/后端跳过。
- `python workers/runtime/test_differential.py`：3 通过。
- `python fixtures/analysis/test_analysis.py`：4 通过。
- `python fixtures/analysis/test_compare_runs.py`：4 通过。
- Python runtime compileall：通过。

原生构建使用已安装 VS vcvars64、CARGO_PROFILE_DEV_DEBUG=0、CARGO_PROFILE_TEST_DEBUG=0 和工作区 target/native-tmp TEMP/TMP，避开本机 MSVC 调试记录/临时链接失败。这些是当时验证环境设置，不是仓库构建要求。

限制：没有 Docker/Ghidra JDK，未执行真实 Java bridge。当时 `workers/runtime/ghidra_test.py` 已补指令分页、模块身份和基本块断言供配置好的 worker 使用。未运行付费模型消融实验；已有配对评分及检查，但没有质量提升测量结论。差分测试仅处理有界 stdin；状态/argv 协议需要单独 harness。路径与数量按原日期保留，不代表当前提交验证。
