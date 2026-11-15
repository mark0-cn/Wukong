# Architecture cleanup acceptance — 2026-11-15

English · [简体中文](2026-11-15-architecture-cleanup.zh-CN.md)

## Scope and identities

The approved file organization and Python design work is implemented. The final controller/worker implementation revision is `c7ed9bcf76c25adae08b6bbab3e63e70eff99a11`; `d9af37fa321fd02451508f9ce77634247404787a` subsequently corrects only a doctor test mock. The final workbench acceptance used that corrected test image. Native input hashing confirmed unchanged packaged source inputs. Documentation-only followups do not change these execution identities. Final integration and revision-bound hosted checks are tracked in [PR #29](https://github.com/mark0-cn/Wukong/pull/29).

| Identity | Value |
| --- | --- |
| Native input tree SHA256 | `d6c91e7fe2a48fbaa46e7d35f3982d3dd11e6f17db335e1dbbb6a8c5fed0fc4e` |
| Signed package manifest SHA256 | `e697990f6aa1afb3e1fd44a3f68dfca58d76f2888a9b7ff9f88500093d762be1` |
| Fresh native73 installation | `958ef753fc7646598935ce8f79c53940` |
| Controller executable SHA256 | `1032d7f1948192a0b72141c0325865fbae742b40ac9fdac5ea9e29a9e731189c` |
| Trusted probe SHA256 | `ae5159ab60fd19b506a957cc555cea65bac6dff6075428086dfbc4437ce06f56` |
| Linux workbench image at `c7ed9bc` | `sha256:ec8adb211a48b83913540ed4ab2336c0c22345b0b96a17c4e89ddaa47cb7503e` |

## Delivered structure and behavior

All 965 files in the approved baseline inventory have reconciled destinations; none are missing. The controller root now has five Rust files instead of 32. Implementation belongs to domain, queries, application, storage, adapters and interfaces. Storage captures one neutral SQL snapshot; desktop presentation consumes it after the transaction. Filesystem ownership is independent of maintenance policy, and bootstrap assembles application dependencies. Public Rust paths retain centralized compatibility exports.

Frontend implementation and tests are grouped by feature, with shared API/UI/testing infrastructure. Private runner and SSH installation tests are adjacent to their owners. Python runtime has one top-level Python entry, `bridge.py`, and explicit `wukong_worker` responsibility packages. Deployments hash nested runtime paths and bytes. Shared analysis stays independent of worker state. Editor settings hide generated artifacts; maintenance documentation defines `target/tasks/<task-id>` for new task artifacts without deleting old evidence.

Python work includes pinned Ruff formatting/static checks, immutable job admission plans, typed callbacks and analysis inputs, explicit imports and startup dependency order, consolidated thin analysis adapters, bounded test transport and documented resource ownership. Formatting preserved all 197 baseline Python ASTs. A 768-case admission comparison preserved normalized specifications, fingerprints and errors. The 214 Python files at this acceptance revision have an AST/import/risk inventory; this is not an exhaustive manual proof of every algorithm.

Three separately committed behavior fixes have bilingual release fragments: unreadable recovery metadata produces bounded sanitized diagnostics; workbench doctor collects the live registry using disposable state; blocked interactive stdin respects a deadline and permits session termination. Partial or uncertain input delivery is not retried. Protocol v3, controller schema 5, native journal 2 and package schema 2 are unchanged.

## Validation results

| Check | Actual result |
| --- | --- |
| Default-desktop Rust all-target tests and Clippy | 232 passed, 31 environment-dependent tests ignored; Clippy passed with `-D warnings` after package-path updates |
| Frontend | 38 unit tests, 36 Playwright flows and production build passed after feature moves |
| Python on Windows | 142 tests run, 22 platform skips after analysis annotations; later interactive real-pipe regression and bridge suite passed; hosted CI covers the final 143-test source |
| Python in a fresh non-root Linux container | 143 tests run: 138 passed, 5 skipped at `c7ed9bc` |
| Automation and installers | 158 tests passed; release metadata check and temporary online-package/installer fixtures passed |
| Real Linux tools | GDB, AFL/QEMU, replay, minimization, isolation, Ghidra and launch fixtures passed |
| Linux controller integration | Four executor tests passed: GDB, AFL, angr, replay/minimization; independent-verifier test passed |
| Linux network and workbench | Dedicated sibling-network capture/patch/negative/binding/transcript checks passed; real workbench service, relay, archive, PTY and launch checks passed at `d9af37f` |
| Workbench doctor | All 13 real probes passed, including i386 execution, GDB, pwntools, pwninit, analysis and Ghidra |
| Dedicated Windows installation | Fresh native73 installer exited 0; exact installed service reached `Running` |
| Installed Windows matrix | Controller, x64/x86 process/debugger, Python, shell, interaction, replay and minimization groups passed |
| Windows service restart | Process, Python, shell, interaction, replay and minimization completed-effect recovery all passed; connection identity unchanged |
| Documentation and catalogs | Bilingual links/review records passed; four generated catalogs remained byte-identical |

The first revision-bound [CI run](https://github.com/mark0-cn/Wukong/actions/runs/43672859356), [dependency acceptance](https://github.com/mark0-cn/Wukong/actions/runs/43672859370) and [isolated uninstall acceptance](https://github.com/mark0-cn/Wukong/actions/runs/43672859383) passed at `c7ed9bc`. These results are not relabeled as checks on later PR or master revisions. Ignored tests and skipped release-client builds are not counted as passes.

The dedicated Windows VM had no default route. Startup and restart qualification verified contained x64/x86 PDB loading, breakpoints and stacks, live WFP network drops, protected process/thread access after self-DACL replacement, private journal ACLs and an exclusive writer. Model credentials stayed on the controller. Installer/uninstaller acceptance did not modify the user's desktop installation.

## Fresh workflow and evidence

Run `run_98bcdf8617874858ac1598b779279c74` used a new workspace. The analyst staged the trusted probe, executed `--echo`, read original output and ran Python source. A distinct verifier read the evidence, staged and executed the probe independently, then recorded a supported `analysis_verify`. Root called `finish_scan` after both children completed. Root, analyst and verifier completed in 9, 20 and 10 turns respectively, with no tool errors.

Three AppContainer jobs completed with exit code 0. One exploratory job was cancelled and retained in the record. All four had successful Job/profile/scratch cleanup. All 72 evidence files matched their recorded sizes and SHA256 values; `verify-bundle` validated 72 files totaling 2,582,259 bytes. Reported usage was 1,525,356 tokens, which is not a performance benchmark.

The native journal ended with 74 closed resources and 114 completed effects. Owned execution scratch and debugger/target/Python processes were absent. Management SSH and the two checked older worker services remained running. The 797-file original archive was verified file-by-file; its SHA256 is `415b80d4e042fdf079aa0fe1c49c8863ab52942f09451e38bafb3d722f859d70`. Native73 installation media was detached after collection; the qualified instance and evidence remain available.

Seven explicitly labelled Linux acceptance containers and their seven named state volumes were removed after 583 original files were archived and hash-verified. No task-labelled container remained. Unattributed volumes were preserved; cleanup did not use global or date-based pruning.

Local raw records remain under the task's `target/architecture-plan` and `target/windows-vm-native73-*` paths. The fresh workflow is under `target/tasks/architecture-cleanup/workflow`. The migration reconciliation, Python decisions, validation summary and cleanup receipt reference the retained originals. These private local records are not release attachments or CI artifacts.

## Corrections and limits

Before integration, master advanced to `08886cf` with the separate Actions redesign. Its Checks/Acceptance/Release/Maintenance behavior was retained, and the consolidated uninstall selector was updated to the reorganized paths. Incoming Python formatting preserved ASTs; regression cases cover the moved cleanup, ownership, recovery and UI entry points. The three upstream Python additions bring the combined inventory to 217 files. Final PR/master checks qualify that integrated tree; these infrastructure changes do not alter the recorded controller/worker inputs.

The first integrated Windows run hit the shared ten-second test deadline in two multi-agent scheduling fixtures. The same revision passed all 232 local Rust tests, with 31 environment-dependent ignores. The test helper now allows 30 bounded seconds for a complete multi-turn scenario and reports role/status/turn/generation on timeout. Semantic assertions and the separate cancellation/wait timing checks remain unchanged; this does not change production execution deadlines. The failed hosted attempt is retained in [run 43676701828](https://github.com/mark0-cn/Wukong/actions/runs/43676701828).

The next integrated [run 43678220841](https://github.com/mark0-cn/Wukong/actions/runs/43678220841) failed a Linux Ghidra lock test and a Windows multi-agent fixture; remaining builds were cancelled. The lock test assumed immediate kernel release after dropping one handle. On Unix, a duplicated or transiently inherited descriptor can retain the lock. The test now retains a duplicate deterministically and waits at most five seconds for release, while still checking exclusion, the same lock file and preservation of the uncertain-analysis marker. Windows lock tests and Clippy passed locally; a Linux primitive-level reproduction passed separately and does not replace the repository's Linux suite. Production locking and process reaping are unchanged.

Failed observations remain separate: the initial image build hit an Ubuntu HTTP 502 and passed on retry; the first network run selected a workbench image whose implicit volumes violated the network inventory check, so the dedicated network Dockerfile was built and passed without weakening checks. Cold Linux doctor testing exposed a mock that also intercepted `platform.platform()`; the test now intercepts only its intended probe, and the cold test plus full workbench suite passed. A stale acceptance-script package path and a task-local archive invocation were corrected before successful final observations.

This qualifies trusted repository fixtures and the listed combinations, not arbitrary target exploitation or every failure interleaving. File hashes do not establish mapped-memory identity. Packaged Python's executable-location warning remains in original stderr. Real Windows Docker Desktop installation was not rerun; Linux container acceptance does not cover it. This record describes implementation and acceptance, with no version change, tag or official publication.
