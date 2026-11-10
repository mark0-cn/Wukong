# Agent / Skill capability changes — 2026-11-09

Implemented role-enforced tool visibility, custom skill discovery/metadata, bounded method selection, immutable general verification records, structured method deliverables, reusable differential testing, secondary-target Ghidra queries, paginated instruction/basic-block evidence, and paired experiment scoring.

Validation on Windows:

- `cargo test --offline --no-default-features --lib --tests`: 140 passed, 6 ignored across 10 suites. Ignored tests require external backends.
- `cargo clippy --offline --no-default-features --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `python workers/runtime/test_bridge.py`: 20 passed, 4 skipped (platform/backend dependent).
- `python workers/runtime/test_differential.py`: 3 passed.
- `python fixtures/analysis/test_analysis.py`: 4 passed.
- `python fixtures/analysis/test_compare_runs.py`: 4 passed.
- Python runtime compileall: passed.

The native build used the installed VS vcvars64 environment, CARGO_PROFILE_DEV_DEBUG=0 and CARGO_PROFILE_TEST_DEBUG=0, and a workspace-local target/native-tmp TEMP/TMP directory to avoid local MSVC debug-record/temp-link failures. These are validation-environment settings, not repository build requirements.

Limitations: no Docker/Ghidra JDK was available for live Java bridge execution. `workers/runtime/ghidra_test.py` now includes instruction pagination, module identity and basic-block assertions for a configured worker. No paid-model ablation experiment was run; paired scoring and its validation are implemented, but there is no measured claim of model-quality improvement. Differential testing currently handles bounded stdin vectors; stateful/argv protocols require a separate harness.
