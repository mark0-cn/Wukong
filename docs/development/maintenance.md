<a id="维护与性能检查"></a>

# Maintenance and performance checks

English · [简体中文](maintenance.zh-CN.md)

<a id="模块边界"></a>

## Module boundaries

- `application/agent_runtime` assembles Rig and drives lifecycle. `admission` controls admission; `models` manages requests/metering; `coordination` projects messages/state; `business_tools` integrates effects. `application/agent_runtime/host` exposes business operations; `application/assessment/supervisor` manages heartbeats/background observation independently; `application/model_catalog`, `application/usage`, `bootstrap` own model selection, accounting and assembly.
- `application/tools` retains the execution pipeline/shared validation. `application/effects` owns durable receipts; `catalog` registers schemas, roles, recovery policy and handlers. `handlers` routes business operations; `coordination` handles shared documents/tasks; `analysis` static queries; `execution` worker dispatch; `verification` independent admission/certification; `files`, `jobs`, `sessions` their resources. Argument/handler subdirectories group by responsibility while one catalog registers them.
- `queries` defines neutral DTOs. Desktop, CLI and report code use the Store-backed facades in `application/queries`. The `storage/queries` modules own SQL and read a complete workspace snapshot in one transaction. Desktop presentation transforms it after the transaction without further queries and keeps a job's execution result, cleanup confirmation and blocking reason separate. The architecture checks reject direct imports from `storage/queries` in `interfaces`. `ui/src/shared/api/commands.ts` constrains command arguments/responses; lifecycle effects belong in corresponding hooks.
- `application/windows_worker_setup/inventory` projects read-only deployment diagnostics: `application/worker_media` owns the generation-record scan, the setup state module owns the recovery record and imported receipt, and the configuration owns the current registration. The desktop consumes the projected report and never reads these records or runs the import/verification workflow itself.
- `application/coordinator` separates runs/tasks/messages/waits/completion use cases, committing through the same Store transactions. `adapters/binary` retains public address/hash APIs; profiling, strings and Ghidra lifecycle have separate modules.
- `storage` uses IMMEDIATE write transactions and independent read-only WAL snapshots. `storage/conversations` owns conversation payloads/revisions. Current state schema is 5, without legacy migration.

See [worker components](../../workers/runtime/README.md). Deploy the Linux runtime directory and generated protocol schema, and install the independent `workers/analysis` package; entry files do not re-export modules. Windows packaging uses its explicit component inventory instead of copying the Linux runtime.

Keep cross-module calls typed and ownership/completion gates centralized. General metadata queries do not read checkpoints. Inheritance, follow-ups and complete export explicitly request full reads. Metadata writes must not clear conversations; checkpoints/mailbox acknowledgments still commit atomically.

`adapters/files` owns host paths, directory ownership markers, link/reparse checks and durable JSON publication. Storage uses these filesystem capabilities directly. Maintenance still owns configuration registration, inventory and cleanup policy; moving helpers does not change locks, adoption rules or deletion scope.

`bootstrap` implements the public service constructors and assembles concrete dependencies. Assessment use cases receive a runner factory; start/resume select their configuration before invoking it. Runner and tool business implementations never call bootstrap. Public constructor signatures and saved-configuration behavior remain unchanged.

Project Python uses the development-only Ruff version pinned in `scripts/dev/requirements.txt` and `ruff.toml`. Run `python -m pip install -r scripts/dev/requirements.txt`, then `python -m ruff format --check workers scripts tests evals fixtures` and `python -m ruff check workers scripts tests evals fixtures`. Format changes separately from logic changes. Static checks cover syntax, control flow, invalid comparisons and undefined names; optional dependency imports still require review. Runtime bundles do not install Ruff. Generated outputs, external benchmarks and cached third-party dependencies are excluded.

Workbench operations hold `OperationLock` for their complete ownership scope. The guard explicitly unlocks before closing its file; do not replace it with a bare file whose drop only closes a descriptor. On Unix, an unrelated concurrent fork can temporarily inherit the open file description before exec, extending a close-only lock past its owner's scope. The clone-descriptor regression checks immediate successor admission while preserving mutual exclusion.

Maintenance admission, update-cache access and cleanup's workbench locks use `adapters/files/locks::FileLock` with the same explicit-release rule. Acquire the guard before any policy check that may return early, and retain it for the full protected operation. Assessment clones share one guard through `Arc`; the last owner releases it. Shared and exclusive modes retain their existing admission rules. Do not apply scope-based release to analyzer locks whose ownership deliberately extends until child processes are reaped.

## Local files and task artifacts

The checked-in `.vscode/settings.json` hides dependency caches and generated outputs in the file explorer. It does not delete files or change build inputs. New task-specific logs, inventories and receipts belong under ignored `target/tasks/<task-id>/`; tool-owned build/cache locations keep their existing paths. Preserve older evidence at its recorded path and hash. Keep source, fixtures, licenses, release fragments and bilingual guides visible and versioned; cleanup still requires explicit ownership and retention checks.

Python build/release CLIs reuse `scripts._lib` for repository discovery, checksums and GitHub access. Keep CLI parsing separate from reusable functions. Build commands inherit the CI job deadline; fixture execution and network downloads have operation-specific limits. Package verification repeats hashes at independent trust boundaries intentionally. The shared analysis package receives admitted paths/bytes and keeps numerical algorithms separate from worker ownership; its annotations document internal inputs and outputs without replacing the generated protocol schema.

The test `support.client.Client` owns only its stdio relay. It bounds both request writes and response waits, verifies response IDs, and closes the relay after an uncertain transport result. Tests explicitly reconnect and reconcile persistent jobs or shut down the daemon; closing the relay never implies daemon/resource cleanup.

<a id="桌面界面样式"></a>

The frontend groups workspace, conversation, agents, evidence, settings and maintenance under `ui/src/features/`. Shared API contracts, UI primitives, localization and test support live under `ui/src/shared/`; `ui/src/app/` owns composition and desktop lifecycle. Keep feature tests beside their implementation and cross-feature browser flows under `ui/e2e/`. Global style import order remains centralized.

## Desktop styling

See [localization](localization.md) for shared bilingual UI/native/report resources and acceptance. User language preferences must not use workspace-configuration saving.

`ui/src/styles.css` imports base, workspace, chat, Agent, evidence, settings, responsive and maintenance styles in fixed order; preserve cascade order when moving rules. Reuse Radix, assistant-ui and Lucide. Define colors centrally in `ui/src/theme.css`; components use semantic variables rather than scattered values. `ui/src/workspace-layout.css` contains only column containers/resize handles; put component sizes, spacing and borders in corresponding `styles/` files and responsive rules in `styles/responsive.css`, without another legacy override layer. Columns use `react-resizable-panels`, the same base as shadcn/ui, retaining keyboard resizing/persistence. `useViewPreference` stores device-local WebView preferences; aliases are scoped by workspace/run ID, never task instructions or target identity. Settings/storage drafts stay in component state; directory operations retain epoch/maintenance-lock checks.

Use neutral charcoal, gold for primary/selected actions, blue for running, green for success, yellow for waiting/warnings and red for failures, always with icons/text. Fonts use system sans-serif and system monospace for code. Central size tokens live in `ui/src/theme.css`: root/body/composer 16px; navigation/buttons/forms/code 14px; auxiliary times/paths at least 13px. Express sizes in rem; do not reintroduce small hardcoded component sizes. Check scrolling, long paths and draft retention at minimum window size. Color hierarchy references [Radix Colors](https://www.radix-ui.com/colors/docs/palette-composition/understanding-the-scale); workspace organization references [VS Code](https://code.visualstudio.com/docs/editing/getting-started/userinterface).

<a id="工具文档"></a>

## Tool documentation

Definitions, parameter schemas, roles, model visibility and display message IDs share one registry; bilingual text uses Fluent. `Visibility::Internal` marks internal entries rather than name blacklists; internal transactions retain permission checks. Desktop aliases exist only for native Rig tools/legacy events. New builtins require catalog message IDs and both translated titles. Generate Markdown from the registry with the developer-only example:

```sh
cargo run --example export_catalogs -- tool-catalog > docs/reference/tool-catalog.md
cargo run --example export_catalogs -- --locale zh-CN tool-catalog > docs/reference/tool-catalog.zh-CN.md
cargo run --example export_catalogs -- --json tool-catalog
cargo test --test tool_catalog
```

Export needs no model credentials and reads neither configuration nor state. Tests compare generated output with committed documentation to catch stale catalogs.

<a id="skill-覆盖清单"></a>

## Skill coverage

`prompts/coverage.json` associates builtin methods with tools, sources and mechanism fixtures using `SkillManifest.related_tools`, `sources`, `fixtures`. Regenerate [coverage](../reference/skill-coverage.md):

```sh
cargo run --example export_catalogs -- skill-catalog > docs/reference/skill-coverage.md
cargo run --example export_catalogs -- --locale zh-CN skill-catalog > docs/reference/skill-coverage.zh-CN.md
cargo run --example export_catalogs -- --json skill-catalog
cargo test --test tool_catalog
```

These are maintenance/query fields, not permissions or proof of backend availability, original-challenge reproduction or Agent solve rates. Unregistered means no recorded association. Custom manifests may set these fields; overriding a builtin body does not inherit builtin fixture claims. Unit tests verify referenced tools/source files/fixture IDs exist. Export reads no credentials/state.

<a id="共享校验与内容身份"></a>

## Shared validation and content identity

`domain/claims` parses flag-shaped claims centrally. Completion tools reject placeholders; coordinator applies independent verification to specific claims. Parsing classifies text, not exploitation success.

Rust file hashes/actual byte counts live in `adapters/files/content`; `adapters/binary` keeps its public hash API. Workers use `wukong_worker.artifacts.common.sha256_file`; build/release scripts use `scripts._lib.checksums.sha256`. Independently injected sandbox scripts remain self-contained without controller imports. Path limits, size budgets and independent controller/worker identity checks remain at call boundaries; shared hashing does not merge/remove them.

<a id="验证"></a>

## Validation

```sh
cargo fmt --all -- --check
cargo clippy --offline --all-targets -- -D warnings
cargo test --offline --all-targets
python workers/tests/run.py unit -p test_bridge.py
```

Install `workers/analysis` and `workers/runtime/requirements.txt` in a virtual environment and make `python` available before worker tests. Ordinary tests use trusted local fixtures/loopback services; ignored real Ghidra/Docker acceptance requires each test's documented environment.

Sibling-container network acceptance builds `tests/acceptance/network.Dockerfile` with the repository root as its context, installing the analysis package and the same pinned Linux worker requirements before entering the isolated network. A bare Python image does not provide the worker's metadata-publication dependencies.

<a id="可重复性能基准"></a>

## Repeatable performance baseline

```sh
cargo test --offline --lib workspace_history_performance_baseline -- --ignored --nocapture
```

The benchmark generates 1,000, 10,000 and 100,000 events in temporary directories, reporting initial-read time and the mean of 100 idle reads. Each loads only the latest 500 events; idle reads do not repeat unchanged history/Agent lists. Measurements are observed values, with no hardware-speed assertions.

Local baseline on 2026-11-08 (Darwin arm64, Rust 1.99.1, unoptimized test build; one initial read, 100 idle reads averaged):

| Historical events | Initial read / ms | Mean idle read / ms | Loaded events |
|---:|---:|---:|---:|
| 1000 | 3.769 | 0.135 | 500 |
| 10000 | 3.723 | 0.139 | 500 |
| 100000 | 3.761 | 0.134 | 500 |

Use these for later same-environment comparisons, not end-to-end frame timing or a comparison against the previous implementation.

Frontend virtualization bounds chat DOM size. `npm test`, `npm run build` and `npm run test:e2e` cover types/components, task switching, drafts, close confirmation, file drops and minimum-window interaction. Browser tests simulate Tauri and do not prove native installation. Storage tests check committed reads during writes and that metadata/repeated checkpoints do not rewrite conversations.

Record data scale, build mode and hardware when evaluating changes. Observe frame time, query waiting, memory and database writes separately. UI throttling must not lose audit, evidence or recovery persistence.

See [documentation maintenance](documentation.md) for pairs, semantic review and CI checks.

<a id="readme-截图"></a>

## README screenshots

Screenshots render the current frontend with fixed illustrative tasks, without a model or worker. Captions identify sample data; screenshots do not establish analysis results or native desktop acceptance. Avoid manually assembled interface mockups that can drift from the implementation.

With root npm dependencies and Playwright Chromium installed, run `npm run dev` in one terminal, serving `http://127.0.0.1:1420`. In another terminal at the repository root, run:

```sh
node scripts/dev/capture-readme.mjs
```

The script reuses frontend fixture structures with a bounded native-command adapter and writes `docs/assets/workspace.en-US.png` and `docs/assets/workspace.zh-CN.png`. Viewport, timezone and sample data are fixed; local configuration/credentials are not read. Failures, unhandled errors or horizontal overflow stop generation. Inspect both images for clipping, fonts, tool rows and the collaboration tree, then commit them with their README editions. Install a missing browser with `npx --no-install playwright install chromium`.

To inspect the desktop projection of a real CLI execution, set `WUKONG_CAPTURE_WORKSPACE` to `target/runtime-workspace.json` and run `cargo test --locked --test cli_workflow cli_runs_dynamic_agents_real_rig_and_exports_evidence`. The test reuses its local scripted provider, real Rig scheduler, SQLite and static binary inspection; it does not call a paid model or execute the fixture. On Windows, use an MSVC developer shell and an isolated test `LOCALAPPDATA` directory. With the frontend server running, render that captured DTO:

```sh
node scripts/dev/capture-readme.mjs target/runtime-workspace.json
```

These diagnostic images go to `target/runtime-workspace/`, keeping the README's illustrative images separate. The adapter supplies desktop transport only: agents, text, tool outcomes, targets and evidence IDs come from the captured backend response. This acceptance establishes the CLI-to-desktop presentation path, not live-model reasoning quality or real Ghidra/debugger execution. Native streaming acceptance additionally holds an HTTP response open until a public partial reply can be read from the desktop snapshot; it checks final-message deduplication and excludes private reasoning.

The recovery test also pauses an in-flight response, restores the native journal and checks that the same public message ID receives the final text.
