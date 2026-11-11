<a id="pwn-方法与机制回归"></a>

# Pwn method and mechanism regressions

English · [简体中文](README.zh-CN.md)

These are original trusted micro-fixtures, without third-party challenge attachments, WP exploits or real flags. They cover reusable invariants/counterexamples, not reproduction of cited competition tasks or Agent solve rates.

On Linux x86_64 install a C compiler, musl-tools and the project's pinned Rust compiler, then run:

```sh
python -m evals.pwn.evaluate --build-dir /tmp/wukong-pwn-fixtures --require-all
python -m evals.tests.test_pwn
```

`--require-all` treats missing compiler/runtime/allocation-reuse prerequisites as incomplete. Without it, record skips individually; do not claim complete success. Windows does not execute these Linux samples. Unknown challenges still require isolation; only trusted source fixtures may use trusted_local.

| Fixture | Actual observation/control |
| --- | --- |
| stream | Real glibc FILE buffer-pointer output, with no-flush/no-overwrite controls |
| exit | Actual callback differences for return/exit, quick_exit and immediate termination |
| seccomp-fd | Native read from an existing FD; openat returns EPERM after filter installation |
| heap-vuln / heap-fixed | Stale alias modifies reused object; realloc-to-zero metadata; fix clears alias; tcache-disabled control |
| lz-vuln / lz-fixed | Valid back-reference, invalid distance, decode overflow and fixed rejection in custom compressed input |
| gc-vuln / gc-fixed | Old generation after VM slot reuse; valid-handle control/fixed rejection |
| utf8 | Actual Rust from_utf8_lossy Cow type/bytes for valid, NUL and invalid input |
| float | C scanf binary64 bit patterns for finite values, negative zero and NaN payloads |
| musl-printf | Actual static-musl output for one valid positional format |

Heap/LZ/GC retain vulnerable/fixed variants. Others deliberately supply primitives or normal mechanism controls, not vulnerability-discovery positives. GC models logical handles, not full moving GC. FILE has no forged vtable; exit does not corrupt libc lists. See evaluator-only [ground truth](manifest.json).

Build output records build.json/results.json, raw inputs, source/binary/input hashes, compile commands, runtime-file hashes, actual stdout/stderr bytes, expectations, skips and failures. Every behavior check links real execution; agent_accuracy remains null. Report buffer effects, logical state changes and native code execution separately. These results do not replace real Docker/GDB/challenge acceptance.

Linux CI uses `--require-all` to prohibit silent mechanism skips, retaining builds, inputs, binaries and measurements in `pwn-fixture-evidence` for 14 days. It is not a release package.

Real sibling-container networking acceptance:

```sh
python tests/acceptance/check_network_verify.py --image python:3.12-slim \
  --output-dir target/pwn-network
```

It mounts current worker source, connects only to its newly created internal Docker network and tests fresh-connection capture/patch, failed responses, configuration-change rejection and transcript hashes. It saves original records, source hashes/image IDs and cleans its containers/network. This verifies replay, not full workbench images/installers/remote runtime equivalence. CI uses a digest-pinned Python image.

Blind runs receive only binaries and [task text](tasks.json). Source, manifest, build metadata and expectations belong to independent reviewers. Reuse evidence-bound scoring:

```sh
python -m evals.analysis.score --tasks fixtures/pwn/tasks.json \
  --build-dir /tmp/wukong-pwn-fixtures --reports reports/pwn-index.json \
  --adjudications reports/pwn-reviewed.json --output reports/pwn-metrics.json
```

Independent labels cover every executed question_id and reference original evidence_id in its report.json. Missing runs remain unresolved; passing the behavior harness never creates correct-answer labels.
