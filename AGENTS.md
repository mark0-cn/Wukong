<a id="项目协作约定"></a>

# Repository collaboration rules

English · [简体中文](AGENTS.zh-CN.md)

This file defines how to maintain the repository. Follow the linked documents for implementation and acceptance details; lockfiles determine tool versions.

<a id="任务范围与工作区"></a>

## Scope and workspace

- For consultation/review, provide findings and recommendations first. For implementation, complete authorized changes, validation, commits and integration.
- Before editing, inspect the branch, worktree and remote `master`. Preserve user/other-task changes; do not overwrite or roll back unrelated work.
- Stage only task-related files and review the diff before committing. Exclude temporary logs, build outputs, challenge attachments and machine-local configuration.
- Understand both sides before resolving relevant conflicts. Explain the specific conflict when user intent cannot be determined.

<a id="分支提交与集成"></a>

## Branches, commits and integration

- `master` is the main/default GitHub branch and release baseline.
- Commit each independent feature or fix separately.
- Use the [commit and release-description rules](docs/development/commits.md); PR titles and new non-merge commits must pass commitlint. Keep PRs English-only; maintain bilingual release fragments alongside user-facing changes.
- After relevant validation, merge ordinary implementation changes into `master` and push. PRs also target `master`. Version PRs follow the release rules below.
- Check whether the remote advanced before integration and retain existing commits. Never resolve integration through force-push or shared-history rewriting.

<a id="文档入口与模块边界"></a>

## Documentation and module boundaries

| Task | Read first |
| --- | --- |
| Architecture, responsibilities, state | [Architecture](docs/design/architecture.md), [maintenance](docs/development/maintenance.md) |
| Agents, Skills, task protocol, evidence | [Implementation contract](docs/design/implementation-contract.md), [report format](docs/reference/report-format.md) |
| Tool registration and invocation | [Tool catalog](docs/reference/tool-catalog.md), [catalog generation](docs/development/maintenance.md#tool-documentation) |
| Workers, debugging, isolation | [Worker guide](workers/runtime/README.md) |
| Pwn environments, containers, launch configuration | [Pwn workbench](docs/user/pwn-workbench.md) |
| Installation, CI, versions, release | [Online installation](docs/user/online-install.md), [releases](docs/development/releases.md), [GitHub maintenance](docs/development/github-maintenance.md) |

- Place implementation by module responsibility; do not concentrate business logic in assembly/bridge entry points.
- Reconcile documentation with code, tests and requirements. Historical validation is not evidence of current validation.
- Keep stable rules/entry points here, not current versions, temporary machine paths, CI run IDs or historical test counts.
- English is the primary human documentation: canonical paths use `.md`; complete Simplified Chinese counterparts use sibling `.zh-CN.md` files. Both link to each other and use same-language document links where available.
- Update both languages in the same commit when behavior, commands, limits or validation claims change. Check semantic equivalence, not just heading/file counts; preserve technical identifiers and evidence. See [documentation maintenance](docs/development/documentation.md).
- Runtime prompts, generated reports, raw evidence and legal/source identifiers are not translated copies of human guides. Regenerate catalogs through their generators. Keep historical dates and validation limits explicit.

<a id="产品内的-agentskill-与工具"></a>

## Product Agents, Skills and tools

These role constraints apply to the Wukong runtime; they do not instruct repository coding assistants to create subagents automatically.

- `root` orchestrates/summarizes; appropriate workers analyze/execute; an independent `verifier` verifies.
- Roles determine permissions/completion protocol. Skills add methods, inputs and artifacts, never permissions or completion-gate bypasses.
- For capability changes, check affected registrations, assembly, prompts and examples in `src/prompts.rs`, `src/agents.rs` and `prompts/`; validate selection/permission behavior.
- Unified registries such as `src/tools/catalog.rs` own definitions, schemas, role visibility and catalogs. Regenerate and check documentation after tool changes.
- Do not claim capabilities only in prompts: backend, permission checks and callable paths must exist.
- Preserve typed protocols/module boundaries. Reuse Rig for generic scheduling, sessions and retries; keep business rules in the business layer.

<a id="状态证据与完成条件"></a>

## State, evidence and completion

- Preserve durable task/message/session/effect semantics so recovery/retry cannot repeat completed side effects.
- Reconcile uncertain effects with the worker first. Ordinary messages never implicitly reopen terminal work or bypass dependencies.
- Preserve session/debugger ownership, serialized mutation and closing rules. Check active children/owned resources before completion.
- Bind evidence to target, module, configuration and content identity. Bound model-facing results while retaining full hashed, traceable artifacts.
- Static suspicion or a crash is not exploitation success. Independently verify impact according to the implementation contract and link original evidence.
- Schema validity proves structure, not semantics. Distinguish verified facts, inferences and gaps.

<a id="环境题目与凭据"></a>

## Environments, challenges and credentials

- The host manages sibling tool/challenge containers. Preserve per-challenge data/network boundaries; never mount the Docker socket in tools.
- Preserve original attachments by hash; modify/prepare copies. Distinguish original-file and executed-file identity.
- Keep binary, libc, loader, working directory and other execution conditions consistent through unified launch configuration. Verify final conclusions in the corresponding challenge environment.
- Run unknown targets in configured isolation. `trusted_local` is only for trusted development fixtures, never a fallback for unknown challenges.
- Keep model credentials on the controller, out of containers, images, releases, migration bundles and logs. Do not hardcode machine paths as project requirements.
- Clean only task-owned or explicitly authorized resources. Verify names, labels or paths; avoid unscoped Docker prune or directory deletion.

<a id="按改动选择验证"></a>

## Select validation by change

Use these entry points according to impact; workflows define precise checks/dependency installation.

| Change | Validation |
| --- | --- |
| Rust logic, protocol, state | `cargo fmt --all -- --check`; relevant tests; when full checks are needed, `cargo clippy --locked --all-targets -- -D warnings` and `cargo test --locked --all-targets` |
| Frontend | `npm ci`, `npm test`, `npm run build`; interaction changes also need UI verification |
| Python worker | Install worker requirements; relevant tests; full entry `python workers/tests/run.py unit -p 'test_*.py'`; analysis fixtures follow CI |
| Release scripts/installers | `python -m scripts.release.cli check`, `python -m unittest discover -s tests/automation -p 'test_*.py'`; installer changes run `tests/automation/test-online-package.ps1` and `tests/automation/test-installer.ps1` |
| Actions, Docker, toolchain | Workflow syntax, download checksums, Bake configuration; real builds/container acceptance/manual release rehearsal as affected |
| Documentation only | Content, language parity, link targets and `git diff --check`; no new behavior tests for prose-only edits |

- `--no-default-features` does not replace desktop-feature validation; mocks/adapters do not replace real Docker/GDB/Ghidra acceptance.
- Linux container acceptance does not cover real Windows Docker Desktop installation. Record dependencies, skips and uncovered areas.
- For defects, prioritize the failing scenario/invariants. After checks pass, broaden/repeat only for new changes, failures or unresolved concerns.

<a id="ci版本与发布"></a>

## CI, versions and releases

- Prefer standard GitHub-hosted runners and online-only distributions; follow release documentation.
- Synchronize versions through version preparation. Keep version PRs version-only. Tags are `vMAJOR.MINOR.PATCH`, match project versions and point to commits merged into `master`.
- **Merging a version PR triggers official publication and requires explicit release authorization.** Ordinary merge authorization does not imply release authorization; existing explicit session authorization need not be requested again.
- Pipeline configuration/validation does not request a release. Use manual rehearsals when publication is not requested.
- Never move or recreate published tags, overwrite fixed-version images or immutable Release attachments. Retry as documented without bypassing checks.
- Reused CI/artifacts must match commit/trusted origin and retain integrity checks. Cache hits are not validation; do not mix PR and production artifacts.
- Reuse qualified results or rerun failed jobs; avoid repeated heavy CI/image builds/full rehearsals for the same commit.

<a id="交付"></a>

## Delivery

- Report changes, main-branch commit, checks actually run and CI links. Distinguish success, failure, expected skips and running jobs.
- Follow task-triggered acceptance CI to its outcome; started/running is not passed.
- State uncovered areas/material limits; distinguish merged code, successful rehearsal and published release.
- Update relevant behavior/configuration/tool/release documentation, not just this file.
- Follow [desktop updates](docs/development/desktop-updates.md). Native installer install/uninstall tests run only on isolated runners or dedicated test environments, never over the user's installation.
