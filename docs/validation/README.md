# Acceptance evidence

Raw acceptance records for the implementation contract (`docs/implementation-contract.md`)
and the delivery audit. Regenerate with the commands listed below; every record here was
produced by actually running the tool, worker, or harness it describes. None of the
Linux/Ghidra rows below are mocks: they exercise real GDB/MI, real AFL++ QEMU, real
Ghidra headless analysis and real Docker worker containers.

## Files

| File | Contents |
| --- | --- |
| `build-and-tests.log` | Windows controller checks: fmt, clippy (`-D warnings`), full Rust test suite, Python syntax check, portable worker protocol tests |
| `runtime-linux.json` | Raw Linux worker records: doctor, GDB/MI breakpoint/stack/memory, transport-restart reconnect, crash replay, 3/3 independent verification, input minimization, AFL++ QEMU fuzz job with parsed statistics |
| `ghidra-linux.json` | Raw Ghidra worker records: headless import and auto-analysis, function search, decompile, xrefs, identity-bound cache hit, hash-mismatch rejection |

## How to reproduce

```powershell
# Windows controller (TMP/TEMP redirection only needed under the DSH sandbox,
# which denies workspace binaries writes to the real %TEMP%; see notes below)
$env:TMP = "$pwd\work\tmp"; $env:TEMP = "$pwd\work\tmp"
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass -Force
.\scripts\check.ps1 2>&1 | Tee-Object -FilePath work\full-check.log
```

```powershell
# Linux runtime acceptance (GDB, replay, verification, minimization, AFL++)
# inside an isolated worker container that has the required capabilities:
$docker = 'C:\Program Files\Docker\Docker\resources\bin\docker.exe'
& $docker exec re-agent-worker python3 /opt/re-agent/runtime/integration_test.py --state /state/acceptance-<id> --fuzz
& $docker exec re-agent-worker sh -c 'cat /state/acceptance-<id>/acceptance-results.json' > docs\validation\runtime-linux.json

# Ghidra worker acceptance (import, analysis, decompile, xrefs, cache, rejection)
& $docker exec re-agent-worker python3 /opt/re-agent/runtime/ghidra_test.py --state /state/ghidra-acceptance
& $docker exec re-agent-worker sh -c 'cat /state/ghidra-acceptance/ghidra-acceptance-results.json' > docs\validation\ghidra-linux.json
```

## Results recorded on 2026-11-06

| Suite | Result |
| --- | --- |
| Rust fmt / clippy | clean |
| Rust tests (`cargo test --all-targets`) | 44 passed; 2 environment-gated tests ignored (`real_ghidra_import_decompile_xrefs_and_cache` needs local Ghidra + JDK 25, `docker_independent_verification_uses_real_trials_and_limits_impact` needs a configured worker container). Both paths are instead exercised live by the worker suites above |
| Python `py_compile` | clean |
| Portable worker protocol tests (`workers/runtime/test_bridge.py`) | 6/6 passed |
| Linux runtime acceptance | GDB break at `main` + stack/memory reads via GDB/MI, relay restart preserves debugger ownership and running jobs, crash replay passes with observed `crash` stage, independent verification 3/3 trials, minimization to the exact 3-byte `BAD` input, AFL++ QEMU fuzz completes with 17,059 executions and a saved `SIGABRT` crash |
| Ghidra worker acceptance | Ghidra 12.2 headless import + auto-analysis, `functions` search resolves `main` and the noinline helper, decompile of the helper contains the `memcpy` sink, decompile of `main` shows the `copy_input` call, xrefs contain a `UNCONDITIONAL_CALL` from inside `main`, identical repeat query hits the identity-bound cache, mismatched target hash rejected |

## Delivery audit

Mapping of the implementation-contract audit items to evidence:

| Item | Evidence |
| --- | --- |
| CLI / config | `tests/cli_workflow.rs` (2 end-to-end runs), `re-agent init/doctor/inspect` smoke tests, `re-agent.example.toml` |
| Dynamic scheduling | `tests/coordinator_state.rs` (12): async child creation, scope dedup, mailbox wakeups, single scope owner, stop propagation |
| Persistence / resume | `llm` checkpoint tests (10 unit), `cli_budget_pause_resume_preserves_model_and_tool_progress`, durable receipts and replay tests |
| Static analysis | `tests/binary_analysis.rs`: real ELF/PE/Mach-O parsing, strings pagination, mitigation evidence, address space validation |
| Ghidra | `ghidra-linux.json` live acceptance + `workers/runtime/ghidra_test.py` + `workers/ghidra/ReAgentBridge.java` |
| GDB/MI | `runtime-linux.json` `gdb` section |
| AFL++ | `runtime-linux.json` `fuzz` section (QEMU mode, parsed `fuzzer_stats`) |
| Crash reduction | `runtime-linux.json` `minimization` (delta debugging to 3 bytes) |
| Independent verification | `runtime-linux.json` `verification` (3/3 trials, distinct producer/verifier agent ids) |
| Skills | `skills/*.md` role prompts composed by `agents.rs` |
| MCP | `unknown_mcp_side_effect_is_never_blindly_replayed`, `worker_filters_model_credentials_and_declares_missing_capabilities` |
| TUI | `interface.rs` TUI + CLI workflow tests |
| JSON / Markdown / HTML reports | `tests/report_export.rs` (4): bundle integrity, redaction, no unverified impact claims, tamper rejection |
| Replayable artifacts | report bundle + `replay.json` + `verify-bundle` command |
| Docker runtime | `workers/Dockerfile` (pinned base + checked Ghidra/Temurin checksums), hardened container acceptance above |
| Evaluation fixtures | `fixtures/` manifest, `build.sh`, `evaluate.py`, `score_reports.py`; 33 behavior checks in `fixtures/build/results.json` |
| Regression tests | 44 Rust tests + 6 Python protocol tests + 2 live Linux worker suites |

## Environment notes

- **DSH sandbox on Windows:** binaries located under the workspace are denied writes to the
  real `%TEMP%`, so `tempfile::tempdir()` fails for the test suite. Point `TMP`/`TEMP` at a
  workspace directory (e.g. `work\tmp`) when running `cargo test` from inside the harness.
  Runs from a normal terminal do not need this.
- **Execution policy:** `scripts/check.ps1` needs `Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass`
  on hosts that block unsigned scripts.
- **Worker capabilities:** the Linux worker requires `CAP_CHOWN/SETUID/SETGID/SYS_PTRACE/DAC_OVERRIDE`
  with `no-new-privileges`; a container started without them reports a clear capability error
  instead of failing silently (see `workers/runtime/README.md`).
- **AFL++ vs Docker Desktop core pattern:** Docker Desktop's host core pattern is an external
  handler, which AFL++ rejects by default. The acceptance run sets
  `AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES=1` (recorded in the job environment) to prove QEMU
  execution and statistics; crash-complete campaigns need a dedicated Linux VM with an
  appropriate core pattern or an explicit per-job decision about the same override.
- **Ghidra JDK:** Ghidra 12.2 requires a Java 25 JDK. The worker image installs Temurin 25 LTS
  with a pinned SHA256; earlier JDK 21-based attempts failed with the launcher's
  "unsupported java version" error and are not part of the evidence.
