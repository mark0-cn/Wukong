# Skill 覆盖清单

[English](skill-coverage.md) · 简体中文

由 `wukong --locale zh-CN skill-catalog` 生成。关联是维护元数据，不授予权限或证明运行时可用。夹具只表示机制/对照覆盖，不代表原始 WP 复现或 Agent 准确率。未登记表示没有关联记录，不表示不支持该 Skill。

| Skill | 相关工具 | 来源 | 机制夹具 |
|---|---|---|---|
| `algorithms` | 未登记 | 未登记 | 未登记 |
| `binary_diff` | 未登记 | 未登记 | 未登记 |
| `crash` | 未登记 | 未登记 | 未登记 |
| `elf_recovery` | 未登记 | 未登记 | 未登记 |
| `exec_sinks` | 未登记 | 未登记 | 未登记 |
| `exit_handlers` | `runtime_profile`, `debug_command`, `poc_verify`, `verification_record` | [docs/reference/pwn-knowledge.md](../../docs/reference/pwn-knowledge.zh-CN.md) | [pwn/exit-consumer](../../fixtures/pwn/tasks.json) |
| `exploit` | 未登记 | 未登记 | 未登记 |
| `format_string` | 未登记 | 未登记 | 未登记 |
| `formats` | `binary_strings`, `function_decompile`, `differential_test` | [docs/reference/pwn-knowledge.md](../../docs/reference/pwn-knowledge.zh-CN.md) | [pwn/compressed-input](../../fixtures/pwn/tasks.json) |
| `fsop` | `runtime_profile`, `debug_command`, `poc_verify`, `verification_record` | [docs/reference/pwn-knowledge.md](../../docs/reference/pwn-knowledge.zh-CN.md) | [pwn/file-buffer-trigger](../../fixtures/pwn/tasks.json) |
| `fuzz` | 未登记 | 未登记 | 未登记 |
| `heap_ptmalloc` | `runtime_profile`, `debug_command`, `poc_verify` | [docs/reference/pwn-knowledge.md](../../docs/reference/pwn-knowledge.zh-CN.md) | [pwn/heap-lifetime](../../fixtures/pwn/tasks.json) |
| `hunter` | 未登记 | 未登记 | 未登记 |
| `interpreter` | `function_decompile`, `emulate_run`, `debug_command` | [docs/reference/pwn-knowledge.md](../../docs/reference/pwn-knowledge.zh-CN.md) | [pwn/gc-stale-handle](../../fixtures/pwn/tasks.json) |
| `jit_dump` | 未登记 | 未登记 | 未登记 |
| `mitigation_analysis` | 未登记 | 未登记 | 未登记 |
| `musl` | `runtime_profile`, `libc_identify`, `poc_verify` | [docs/reference/pwn-knowledge.md](../../docs/reference/pwn-knowledge.zh-CN.md) | [pwn/musl-format](../../fixtures/pwn/tasks.json) |
| `payload_constraints` | `file_put`, `py_run`, `poc_verify` | [docs/reference/pwn-knowledge.md](../../docs/reference/pwn-knowledge.zh-CN.md) | [pwn/utf8-buffer](../../fixtures/pwn/tasks.json), [pwn/float-roundtrip](../../fixtures/pwn/tasks.json) |
| `pwn` | 未登记 | 未登记 | 未登记 |
| `pwn_environment` | 未登记 | 未登记 | 未登记 |
| `recon` | 未登记 | 未登记 | 未登记 |
| `recovery` | 未登记 | 未登记 | 未登记 |
| `reporter` | 未登记 | 未登记 | 未登记 |
| `reverse` | 未登记 | 未登记 | 未登记 |
| `rop_analysis` | 未登记 | 未登记 | 未登记 |
| `seccomp_io` | `seccomp_analyze`, `mitigation_profile`, `poc_verify` | [docs/reference/pwn-knowledge.md](../../docs/reference/pwn-knowledge.zh-CN.md) | [pwn/filtered-fd](../../fixtures/pwn/tasks.json) |
| `state_machine` | 未登记 | 未登记 | 未登记 |
| `tooling` | 未登记 | 未登记 | 未登记 |
| `types` | 未登记 | 未登记 | 未登记 |
