<a id="acceptance-evidence"></a>

# 验收证据

[English](README.md) · 简体中文

这是 2026-11-06 的历史原始验收及交付审计，原文所指契约路径为 `docs/implementation-contract.md`。以下命令保留当时环境，当前入口见[维护约定](../../../development/maintenance.zh-CN.md)。记录确实运行相应工具、worker 或 harness；Linux/Ghidra 不是 mock，而是真实 GDB/MI、AFL++ QEMU、Ghidra headless 及 Docker。未记录的提交身份仍未知，不代表当前提交通过。

<a id="files"></a>

## 文件

| 文件 | 内容 |
| --- | --- |
| `build-and-tests.log` | Windows fmt、clippy（`-D warnings`）、全 Rust 测试、Python 语法及可移植协议测试 |
| `runtime-linux.json` | doctor、GDB/MI 断点/栈/内存、重连、重放、3/3 独立验证、缩减及 AFL++ QEMU 统计 |
| `ghidra-linux.json` | 导入/分析、函数搜索、反编译、xref、身份缓存命中、错误哈希拒绝 |

<a id="how-to-reproduce"></a>

## 当时的复现命令

```powershell
# Windows：仅 DSH 沙箱需要 TMP/TEMP 重定向，见环境说明
$env:TMP = "$pwd\work\tmp"; $env:TEMP = "$pwd\work\tmp"
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass -Force
.\scripts\check.ps1 2>&1 | Tee-Object -FilePath work\full-check.log
```

```powershell
# 具备所需 capabilities 的隔离容器：Linux 运行时验收
$docker = 'C:\Program Files\Docker\Docker\resources\bin\docker.exe'
& $docker exec re-agent-worker python3 /opt/re-agent/runtime/integration_test.py --state /state/acceptance-<id> --fuzz
& $docker exec re-agent-worker sh -c 'cat /state/acceptance-<id>/acceptance-results.json' > docs\validation\runtime-linux.json

# Ghidra 导入、分析、反编译、xref、缓存与拒绝
& $docker exec re-agent-worker python3 /opt/re-agent/runtime/ghidra_test.py --state /state/ghidra-acceptance
& $docker exec re-agent-worker sh -c 'cat /state/ghidra-acceptance/ghidra-acceptance-results.json' > docs\validation\ghidra-linux.json
```

<a id="results-recorded-on-2026-11-06"></a>

## 2026-11-06 的结果

| 套件 | 结果 |
| --- | --- |
| Rust fmt / clippy | 通过 |
| Rust `cargo test --all-targets` | 44 通过、2 环境门控忽略：`real_ghidra_import_decompile_xrefs_and_cache` 需本地 Ghidra/JDK25，`docker_independent_verification_uses_real_trials_and_limits_impact` 需 worker；相应路径由上述真实 worker 套件覆盖 |
| Python `py_compile` | 通过 |
| `workers/runtime/test_bridge.py` | 6/6 通过 |
| Linux runtime | main 断点、栈/内存、relay 重启保留调试 owner/job、crash 重放、3/3 验证、精确三字节 BAD 缩减、AFL++ QEMU 17,059 次并保存 SIGABRT |
| Ghidra | 12.2 headless 导入/分析，main/noinline helper 搜索，helper 反编译含 memcpy，main 含 copy_input，xref 含 main 内 UNCONDITIONAL_CALL，重复命中身份缓存、错误哈希拒绝 |
| 夹具真值 | 容器内 `fixtures/build.sh` + `fixtures/evaluate.py --repetitions 3`：33/33 行为检查，正常/漏洞/修复路径退出/信号及样本/输入/源码哈希 |

<a id="delivery-audit"></a>

## 交付审计

| 项目 | 当时证据 |
| --- | --- |
| CLI / config | `tests/cli_workflow.rs` 两次端到端、`re-agent init/doctor/inspect`、`re-agent.example.toml` |
| 动态调度 | `tests/coordinator_state.rs` 12 项：异步创建、scope 去重、邮箱唤醒、单 owner、停止传播 |
| 持久化/恢复 | llm 检查点 10 单测、`cli_budget_pause_resume_preserves_model_and_tool_progress`、持久回执/重放 |
| 静态分析 | `tests/binary_analysis.rs`：真实 ELF/PE/Mach-O、字符串分页、缓解证据、地址校验 |
| Ghidra | `ghidra-linux.json`、`workers/runtime/ghidra_test.py`、`workers/ghidra/ReAgentBridge.java` |
| GDB/MI | runtime-linux.json 的 gdb |
| AFL++ | runtime-linux.json 的 fuzz，QEMU、fuzzer_stats |
| 缩减 | runtime-linux.json 的 minimization，三字节 |
| 独立验证 | runtime-linux.json 的 verification，3/3、不同生产/验证 Agent |
| Skills | prompts 分类资源，src/prompts.rs 注册，agents.rs 装配 |
| MCP | `unknown_mcp_side_effect_is_never_blindly_replayed`、`worker_filters_model_credentials_and_declares_missing_capabilities` |
| GUI | gui.rs 视图及 CLI 测试 |
| JSON/Markdown/HTML | tests/report_export.rs 四项：完整性、脱敏、禁止无证据影响、篡改拒绝 |
| 重放产物 | 报告包、replay.json、verify-bundle |
| Docker | workers/Dockerfile 固定基础/校验 Ghidra/Temurin，真实加固容器 |
| 评测夹具 | fixtures manifest/build.sh/evaluate.py/score_reports.py，build/results.json 33 项 |
| 回归 | 44 Rust、6 Python、2 真实 Linux worker 套件 |

<a id="environment-notes"></a>

## 环境说明

- **Windows DSH 沙箱：**工作区程序不能写真实 `%TEMP%`，tempfile::tempdir 失败；将 TMP/TEMP 指向如 work\tmp。普通终端无需如此。
- **执行策略：**禁止未签名脚本的机器运行 scripts/check.ps1 前需 `Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass`。
- **Capabilities：**原记录写作 `CAP_CHOWN/SEGUID/SETGID/SYS_PTRACE/DAC_OVERRIDE`（SEGUID 为原文拼写；现行要求见 worker 指南）。配合 no-new-privileges，缺失时明确报错。
- **AFL++ / Docker Desktop：**外部 core handler 默认被 AFL++ 拒绝。该次显式 `AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES=1`，记录在环境中，只证明 QEMU/统计；完整收集需合适 core pattern 的专用 VM 或显式每 job 决策。
- **Ghidra JDK：**12.2 需要 Java25，镜像安装 SHA256 固定的 Temurin25。更早 JDK21 尝试因 unsupported java version 失败，不计入证据。
