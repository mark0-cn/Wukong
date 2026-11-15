# Execution contract v3 acceptance — 2026-11-15

English · [简体中文](2026-11-15-execution-contract-v3.zh-CN.md)

## Scope and identity

The final installed Windows matrix, service-restart recovery and fresh Wukong workflow passed against source `8004fe427392a6045385ae34d4e5cab348582fb4`. This record qualifies the listed trusted repository fixtures, not arbitrary production targets. Subsequent documentation-only commits do not change the recorded worker source tree. Integration and hosted-check outcomes are tracked in [PR #27](https://github.com/mark0-cn/Wukong/pull/27); this record does not substitute earlier CI results for checks on the final PR revision.

| Identity | Value |
| --- | --- |
| Worker source tree SHA256 | `3d73f7132737ed061a208044d1138a8e06835087789461ade88c9c7218a0dd77` |
| Signed package manifest SHA256 | `91af64bce8723413a4529836f7e6dea11913fc2423a8dd16731071f6ae88a4d5` |
| Fresh installation ID | `25d24f88f5f141a487aae1630a132b94` |
| Controller executable SHA256 | `96a03e16f219be814ce9fe132d468fdf89542c322d80a6f5122324b8a4c64bfe` |
| Trusted probe SHA256 | `cbec6cf17e6240fc296177d92dfc0d2334ba763a00a4462b9a83239f9059a311` |

The dedicated Windows 10 VM (build 17134) had no default route. Native72 used a fresh installation, clean source identity and verified static-CRT binaries. Model credentials and signing private keys remained on the controller. The user's desktop installation was not used for install/uninstall acceptance.

## Installed Windows results

Installation exited successfully and the exact installed service reached `Running`. Startup qualification checked both debugger architectures, contained PDB loading/breakpoints/stacks, live WFP AppContainer network-drop evidence, protected process/thread access after self-DACL replacement, private journal ACLs and the exclusive writer lock.

| Acceptance group | Final result |
| --- | --- |
| Controller discovery, identity, lease and resource admission | Passed |
| Process execution and capacity | x64 and x86 passed |
| WinDbg MCP sessions | x64 and x86 passed |
| Python source execution | 5 cases and packaged analysis helper passed |
| Shell execution | 8 cases passed |
| Interactive sessions | 10 cases passed |
| Crash replay and independent verification | 12 cases passed |
| Input minimization | 20 cases passed |
| Completed-operation recovery after service restart | Process, Python, shell, interaction, replay and minimization all passed |

The restart generated exactly one new schema-4 qualification receipt and preserved the previous receipt byte-for-byte. Enrollment identity did not change. Recovery compared the cached original opener with the separately observed terminal state and original artifacts; it did not mistake a historical `started` response for a new execution.

The final journal contained 73 closed resources and 115 completed effects. Owned execution scratch and debugger/target/Python processes were absent. The new worker, management SSH and the two checked pre-existing worker services remained running. Native68–72 media were detached after their evidence was preserved; installed instances and historical evidence were retained.

## Fresh Wukong workflow

Run `run_89821becedab4dc19e6039d9c7224d56` used a new workspace and the normal discovery, dispatch, evidence and completion paths. The analyst staged the trusted probe, ran `--echo`, read original stdout and ran a Python source job. A distinct verifier inspected the evidence, staged and independently ran the probe again, and recorded a supported `analysis_verify` result. Root successfully called `finish_scan` after both children completed.

All three roles completed: root 9 turns, analyst 16 and verifier 8. Three AppContainer jobs completed with exit code 0 and successful Job/profile/scratch cleanup. There were no tool errors. All 65 local evidence files matched their recorded sizes and SHA256 values. `verify-bundle` validated 67 files totaling 2,595,768 bytes against the probe identity. The run consumed 1,346,887 reported tokens; this fixture is not a model cost or performance benchmark.

## Local and Linux validation

| Check | Result and revision |
| --- | --- |
| Root `cargo test --locked --all-targets`, default desktop features | 232 passed, 31 ignored at `8004fe4` |
| Native Windows all-target tests | 159 passed, 5 ignored at `8004fe4` |
| Root and native all-target Clippy with `-D warnings` | Passed at `8004fe4` |
| Protocol contract corpus and generated schema | 3 tests passed at `8004fe4` |
| Linux Python unit suite in a fresh non-root container | 132 run: 127 passed, 5 skipped at `8004fe4` |
| Real Linux controller adapters | GDB, AFL/QEMU, angr and replay/minimization: 4 passed at `8004fe4` |
| Real Linux controller independent-verification test | Passed at `8004fe4` |
| Automation suite | 152 passed at `289615b`; later changes did not alter automation code |
| Frontend dependency install, unit tests and build | Passed earlier in this branch; 38 unit tests; final hosted CI supplies the revision-bound UI checks |
| Release metadata and bilingual documentation checks | Passed; no version or publication action |

Ignored environment-dependent Rust tests are not counted as unit passes; the installed groups above were run explicitly. Python skips remain visible in the original log. Earlier dynamic and isolation container receipts bind to `b544807`, and the branch's sibling-network replay acceptance also passed. Those earlier observations are retained as historical evidence, not relabeled as reruns at `8004fe4`. The final real adapter and verification runs used the updated image `wukong-worker-acceptance:execution-v3-8004fe4` with no Docker socket or model credentials.

## Corrections and evidence retention

Acceptance exposed and fixed these gaps before the final run: null optional verification predicates, bounded job-inventory pagination, debugger targets supplied through `LaunchSpec`, and missing shared-schema `output_limit` fields for shell/replay/minimization/verification. The angr adapter now uses Explorer. Fixture corrections preserve immutable startup receipts, keep queued requests actively polled, inspect the actually packaged analysis module and distinguish verification-only fields from replay parameters. Unknown execution outcomes still cannot be automatically replayed.

Earlier failed trials and logs were retained. These include the full-disk installation interruption, outdated fixture assumptions, a root/non-root Linux test setup mismatch, a workflow budget pause, and a Windows build blocked by a running executable. The final workflow used a separate copy of the exact controller binary, and final all-target tests passed after the build lock was released. None of these earlier outcomes is counted as a final pass.

Raw evidence stays outside Git under task-owned `target/` storage. Paths below identify this acceptance's artifacts, not machine-specific product requirements:

- `execution-contract-v3-validation.json`: cross-checks all installed/recovery reports, workflow roles/evidence, source identity and final cleanup.
- `windows-vm-native72-*-acceptance.json` and `windows-vm-native72-*-recovery.json`: eight installed groups and six restart-recovery reports.
- `windows-vm-native72-resource-originals.zip`: 794 originals, 886,802 bytes, SHA256 `366d9daa70744bbe9005c98ddee103982329794068d262e750c51c6a1b615a65`. Every archived member was independently checked against its original size/hash. The archive includes protected requests/results, journal and qualification receipts, not credentials.
- `windows-vm-native72-final-receipts.json`: final independent observation, SHA256 `7af464f3da62e3b6c116268df79a3e40a734a111f848096263e863bb2ba6eca2`.
- `wukong-native72-workflow/`: controller identity, fresh state, events, evidence bundle and bundle-verification result; private configuration is not published.
- `tool-evaluation/v3-8004fe4-linux-preserved/wukong-v3-8004fe4-controller/`: 297 checked original files; `raw-states.tar` SHA256 `4ae9d2b64d1152f72b7f40f3737b8b125911cc7bbe1c8493959b7927f837d28e`. The task container was stopped after archive verification. Earlier task containers were likewise archived and stopped without deleting unrelated resources.

## Limits and compatibility

Protocol v3 rejects v2 workers and incompatible state. Enroll a fresh worker installation/state directory and preserve previous directories and evidence; no automatic migration or destructive cleanup is provided.

This acceptance uses trusted repository fixtures and records `production_execution=false`. File hashes do not establish mapped-memory identity. A nonfatal Python executable-location warning remains in original stderr. Historical adversarial, interrupted-operation/fault-injection and later-candidate observer limits in the [Windows validation history](../../../workers/windows/VALIDATION-HISTORY.md) are not newly qualified by these normal and completed-operation recovery runs. Kernel debugging, GUI target interaction and automatic VM/snapshot management remain outside the contract. No official release, version change or tag publication is part of this integration.
