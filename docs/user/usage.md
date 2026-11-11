<a id="使用指南"></a>

# User guide

English · [简体中文](usage.zh-CN.md)

[Project home](../../README.md) · [Language settings](languages.md)

This guide covers the desktop workspace, models, task recovery and collaboration. See [Quick start](../../README.md#quick-start) for initial builds and configuration.

<a id="目录"></a>

## Contents

- [Desktop workspace](#desktop-workspace)
- [Configuration and models](#configuration-and-models)
- [Task management and recovery](#task-management-and-recovery)
- [Budgets and usage](#budgets-and-usage)
- [Agent roles and capabilities](#agent-roles-and-capabilities)
- [Reports and review](#reports-and-review)

<a id="桌面工作区"></a>

## Desktop workspace

The desktop uses Tauri and React. Double-click `wukong.exe` or open a window with these commands; no separate browser is needed:

```sh
wukong
wukong gui
wukong gui RUN_ID
```

`gui RUN_ID` selects a saved task. Existing records remain readable without model credentials; starting analysis requires a working model configuration.

<a id="日常操作"></a>

### Everyday operation

1. Choose **New analysis**, select or drop a binary, edit the task and start.
2. Search/switch tasks in the left list. Read public replies and tool operations; expand cards for original parameters/results.
3. Select an Agent in **Collaboration tasks** to read its conversation. Task overview presents a summary; findings link to evidence.
4. Add information through the composer. Use **Assign follow-up** for new work on a completed Agent. Paused analysis must be resumed manually.
5. Use top-level actions to pause, resume or export; stop a branch in the collaboration panel. The resume dialog can adjust cumulative budget limits.

Task budgets bound total tokens, cost, duration, Agent count and collaboration depth.

The three columns contain task history, analysis content and collaboration details. Drag separators or focus them and use arrow keys to resize; widths are saved locally. Narrow windows hide collaboration details by default to preserve reading space; manual visibility choices take precedence. **Toggle task list** and **Collaboration tasks** hide either side; **Focus mode** hides both and restores the previous visibility when toggled again, retaining unsent drafts. **Updates**, **Tool environment**, **Challenge environment** and **Workspace settings** are at the bottom left; reveal the task list to access them in focus mode. The theme uses charcoal and gold, blue for running, green for success, yellow for waiting/warnings and red for failure, always with icons/text.

Conversations follow new progress until you scroll upward; **Latest progress** returns to the bottom. New task output language follows preferences and task content; existing records keep their original text.

Each window drives one task at a time while allowing other tasks to be viewed. Closing waits for that window's analysis to pause/save and for file operations such as exports to finish; the close dialog identifies unsent drafts.

<a id="阅读与导航"></a>

### Reading and navigation

- The pencil beside the title sets a task alias; an empty alias restores the filename. Aliases live in device-local WebView storage, scoped by workspace path/task ID. They do not change instructions, databases, reports or target files and are not migrated. Clearing WebView data removes aliases/layout preferences.
- Search history by name, path or task text; date groups can be filtered to active or historical/paused tasks. Panel widths and visibility remain local.
- The top status line shows the current operation. Footer **Model and usage** expands model, tokens, cost and Agent count. Connection failures change both the indicator and text.
- Successful tool operations use compact rows. Large groups can expand all records; failed/unresolved operations remain visible. Parameters/raw results load on expansion.
- The composer grows with content and can expand manually. **Add information** does not reopen finished work; **Assign follow-up** explicitly does. Paused tasks still require resuming. Recipient and generation remain bound to the displayed Agent.
- Reading older messages never jumps to the bottom. **Latest progress** appears only away from the bottom and counts new messages.
- Findings prioritize verification results and evidence gaps. One artifact previews automatically; multiple artifacts use a list. Source, SHA-256 and evidence ID are copyable. Narrow windows switch via **Back to findings** instead of squeezing list and details together.

<a id="协作任务树"></a>

### Collaboration task tree

The right panel draws connectors from actual parent IDs. Lead, Analysis and Verify labels describe roles; color describes execution state. Roles alone never create hierarchy or execution order.

- Click a node to show its conversation and inspector; its arrow folds/expands without selecting.
- Nodes show name, role, status and scope in two rows, with another row for errors. Full task/error/summary remain in details.
- Folded branches summarize all descendants, excluding the parent: total, running, waiting/paused, failed and completed. Stopped descendants count toward the total. Folding neither stops work nor hides failure indicators.
- The selected node shows its ancestry; click ancestors to navigate. A folded ancestor indicates that it contains the selected task.
- Folding preferences persist per local workspace/task across switches and restarts. Beyond four visible levels, use **Focus branch**, **Up one level** and **Full task tree** to avoid excessive indentation.
- Up/Down move focus; Left/Right fold/expand or move to parent/child; Home/End jump to the ends. Enter/Space selects. Keyboard focus alone does not switch conversation.
- Missing/cyclic parent records are marked and shown separately without changing records or inferring dependencies.

<a id="修改设置"></a>

### Editing settings

**Workspace settings** includes model, API URL, credential environment variable name, budgets, concurrency, execution backend and storage paths. Categories are language/model, task budgets, storage and advanced configuration. The advanced TOML editor includes role models, pricing and MCP.

- Switching categories or closing retains configuration/storage drafts. Settings cannot close during directory changes; save configuration drafts before switching directories.
- Saving validates configuration and preserves original comments/unchanged fields.
- Externally changed files are not overwritten; reload before editing.
- Open another TOML, save/apply it for new tasks.
- Pause active analysis and wait for current operations before applying configuration.
- Resumed tasks use their saved configuration.
- A temporary `--state` override takes precedence and is not written to configuration.

API key values are not stored in TOML: enter the environment variable name, not the secret. Windows reads process variables first, then user/system variables when absent, allowing desktop launches to use saved credentials. PowerShell `$env:...` values apply only to that process and its children.

<a id="windows-发布与启动"></a>

### Windows deployment and launch

Normal installations use official Release `-setup.exe` files; see [online installation](online-install.md). The following applies only to manually deploying source-built desktop executables.

An EXE can inherit a build directory's Low integrity label and then fail to access Docker Desktop's named pipe. The [deployment script](../../scripts/dev/publish-gui.ps1) copies it to a normal application directory outside the workspace and creates a shortcut to the selected configuration.

Run at the repository root, using the quick-start release binary and root configuration:

```powershell
.\scripts\dev\publish-gui.ps1 -Destination D:\wukong-bin\desktop-gui -BinaryPath .\target\release\wukong.exe -ConfigPath .\wukong.toml
```

Launch `Wukong.lnk` in the destination thereafter. Configuration still saves to the original selected file; the script does not change workspace or Docker permissions. Close the old window and rerun the script after rebuilding.

<a id="配置与模型"></a>

## Configuration and models

See [wukong.example.toml](../../wukong.example.toml) for all fields. Relative file paths resolve against the configuration directory.

<a id="配置查找顺序"></a>

### Configuration discovery

| Entry point | Priority |
| --- | --- |
| Explicit `--config PATH` | Always uses that file first |
| Desktop | `wukong.toml` beside the executable → current directory → default user-data configuration |
| CLI | Current-directory `wukong.toml` → executable directory |

Desktop defaults are Windows `%LOCALAPPDATA%/WukongData/wukong.toml` and Linux `$XDG_DATA_HOME/Wukong/wukong.toml` (default `~/.local/share/Wukong/wukong.toml`), created as needed at first launch. Switch task data/update cache in **Workspace settings → Storage locations**; see [installation](online-install.md#updates).

```sh
wukong --config ./wukong.toml --state ./saved-state gui
wukong --config ./wukong.toml analyze ./sample.elf --task "Recover the record format and verify field boundaries"
```

<a id="模型服务"></a>

### Model services

| `model.provider` | Interface | Default credential variable |
| --- | --- | --- |
| `openai` | Chat Completions | `OPENAI_API_KEY` |
| `openai_responses` | Responses | `OPENAI_API_KEY` |
| `anthropic` | Anthropic | `ANTHROPIC_API_KEY` |
| `ollama` | Native Ollama `/api/chat` | `OLLAMA_API_KEY` (optional) |

Set custom endpoints with `model.base_url` and credential variable names with `model.api_key_env`. Ollama defaults to `http://localhost:11434`; omit `api_key_env` for a credential-free local service. Remove that field when switching the example configuration to Ollama.

Before the first model request, the resolved endpoint, including environment-supplied defaults, is saved. Resume uses the task's saved model/backend configuration.

<a id="角色模型"></a>

### Role models

Root always uses `model.model`. `model.role_model` selects a shared child model for analyst/verifier; `model.role_models` overrides each role's model and per-turn output budget. They share provider, endpoint and credentials.

```toml
[model]
provider = "openai_responses"
model = "your-root-model"
api_key_env = "OPENAI_API_KEY"
max_tokens = 8192
role_model = "your-role-model"
role_max_tokens = 16384

[model.role_models]
verifier = { model = "your-verifier-model", max_tokens = 16384 }
```

Priority: role-specific configuration → `role_model` → root model. `role_max_tokens` is the default per-turn output limit for children with explicitly configured role models; role-specific `max_tokens` wins. For `finish_reason=Length` truncation, increase the relevant limit within provider support.

<a id="执行后端"></a>

### Execution backends

Execution tools default to disabled. See [execution worker](../../workers/runtime/README.md) for Docker/SSH/VM and [Ghidra backend](../../workers/ghidra/README.md) for local Ghidra. `doctor` reports actual capabilities; model services/engines depend on the environment.

`trusted_local` runs processes directly on its host, only for explicitly selected trusted fixtures. Use an isolated worker for ordinary dynamic analysis.

<a id="任务管理与恢复"></a>

## Task management and recovery

Replace `RUN_ID`, `AGENT_ID`, `CALL_ID` and `EVIDENCE_ID` with actual IDs.

<a id="查看与干预"></a>

### Inspect and intervene

```sh
wukong runs
wukong status RUN_ID --events 30
wukong gui RUN_ID
wukong message AGENT_ID "Record this additional analysis constraint"
wukong message AGENT_ID "Continue investigating this function's boundary conditions" --kind follow_up
wukong stop AGENT_ID --reason "Stop this branch"
```

Ordinary messages add information without reopening completed work. `--kind follow_up` assigns more work to a completed Agent, preserving history and cumulative budgets. Paused analysis still requires explicit resume. Stopping a branch stops its subtasks too; recorded evidence remains available.

<a id="暂停与恢复"></a>

### Pause and resume

```sh
wukong pause RUN_ID
wukong resume RUN_ID --max-tokens 1000000
```

CLI Ctrl+C requests scheduling to stop at a persistence boundary, retaining resumable state. `resume` uses saved configuration/checkpoints; `--max-tokens` and `--max-usd` change cumulative limits. A lease excludes simultaneous active controllers.

Native Rig logs live in state-directory `rig/RUN_ID`. State schema is currently 3, without legacy database migration. Old directories return a version message; choose a new state directory for new tasks.

Reconcile external jobs/debuggers separately. Checkpoints cannot restore vanished debugger processes or live sessions lost with a container/VM.

<a id="未决外部调用"></a>

### Unresolved external calls

Third-party MCP side effects with unknown results remain unresolved rather than replaying blindly. After reviewing the actual outcome and attaching evidence, reconcile:

```sh
wukong reconcile CALL_ID --result-file ./reviewed-result.json --evidence-id EVIDENCE_ID
```

<a id="预算与用量"></a>

## Budgets and usage

Token budgets accumulate provider-reported usage. When required usage is absent, the response is saved and work pauses; unknown consumption is never counted as zero.

Already-dispatched parallel requests may exceed observed thresholds; canceled/retried requests may incur unreported charges. Configured USD prices are estimates, not billing guarantees.

Set actual per-million input/output prices in `[pricing]` before using cost budgets. Default zero prices disable USD accounting; `max_usd` requires corresponding pricing. See the [configuration example](../../wukong.example.toml).

<a id="agent-角色与能力"></a>

## Agent roles and capabilities

<a id="角色分工"></a>

### Roles

| Role | Responsibility |
| --- | --- |
| `root` | Decompose, dispatch, coordinate evidence and summarize delivery |
| `analyst` | Analyze targets and choose/switch methods within scope |
| `verifier` | Independently review another Agent's conclusions with actual verification evidence |

These are the only runtime roles. Create children with `role="analyst"` or `role="verifier"`; select capabilities through `skills=["recon", "reverse"]`. `hunter`, `fuzz`, `crash`, `exploit` and `reporter` are also capability names. Role instructions install automatically; `load_skill` does not change identity.

<a id="协作与生命周期"></a>

### Collaboration and lifecycle

Root orchestrates; dynamic child Agents perform target operations. Notes, todos, findings and coverage are shared within the run; threat models are used when analyzing security boundaries. Each Agent has its own conversation. Complete tool output is stored as hashed artifacts; models receive bounded results/evidence IDs.

| Tool | Purpose |
| --- | --- |
| `task` | Create an asynchronous child task |
| `wait` | Wait for a reachable Agent report |
| `wait_for_agents` | Collect reports under success/termination conditions |
| `send_message_to_agent` | Send information without ending waits early or reopening work |
| `follow_up_agent` | Assign more work to a completed Agent, retaining history/cumulative budget |
| `view_agent_graph` | Inspect collaboration and sessions awaiting cleanup |

Same-scope tasks deduplicate only against active Agents. Another verifier performs independent review; root summarizes ordinary reports. Argument validation, structured output, context retention/compaction, retry classification and usage accounting reuse Rig; MCP reuses rmcp. Business logic checks evidence, permissions and worker-session cleanup.

Children close their debug/interactive sessions before completion; root resolves all leftover sessions before finishing. Other Agents in the run may close sessions abandoned by terminated owners, but may not send commands to them.

<a id="自定义提示词"></a>

### Custom prompts

Prompts live in [`prompts/`](../../prompts), with shared rules in `base.md`:

| Directory | Contents |
| --- | --- |
| `roles/` | Role instructions and lifecycle |
| `capabilities/` | Specialist capabilities |
| `methods/` | Formats, types, algorithms, state machines, binary differences and other methods |
| `support/` | Tools and recovery guidance |
| `examples/` | Analysis examples |

[`src/prompts.rs`](../../src/prompts.rs) registers builtin names, purposes, bodies and example references. Models receive the catalog and selected bodies only. Directory categories do not change callable skill names.

Override builtin bodies or add methods in `skills_dir/<name>.md`; provide examples in `demos_dir/<name>.md`. The default examples directory is `prompts/examples`; missing directories skip examples. Builtin bodies are embedded and do not require runtime repository files.

<a id="报告与复核"></a>

## Reports and review

```sh
wukong report RUN_ID --output ./reports/assessment-01
wukong verify-bundle ./reports/assessment-01
```

The destination must be new. Bundles contain target snapshots, original evidence, configuration/reproduction metadata, JSON/Markdown/HTML and a SHA256 manifest. Export does not execute the target. `verify-bundle` checks file/mapping integrity, not publisher identity. Original evidence is not redacted; exported configuration redacts credential fields and is not a credential backup. Headings/status follow export language without rewriting existing content. See [report format](../reference/report-format.md).

Independent review requires a different Agent and actual worker results. Static suspicion or a crash alone never becomes verified impact automatically.

[`fixtures/`](../../fixtures/README.md) defines local builds and behavior regressions, separating ground truth, execution observations and Agent detection conclusions. Passing behavior regression does not establish Agent analysis quality.

<a id="自定义-skill-元数据"></a>

### Custom Skill metadata

Custom `skills_dir` Markdown files appear in the capability catalog. Optional matching `.json` files accept `description`, `applies_to`, `excludes`, `requires`, `deliverables`, `validation`, `version`; all except description/version are string arrays. The catalog records effective-body SHA256/source, including override hashes. Metadata guides routing, never permissions; actual backend capabilities remain checked. Limits: 16 KiB per metadata file, 256 catalog entries.

<a id="共用分析工具"></a>

### Shared analysis tools

`function_instructions` returns paginated instructions, block starts, branch/call flow and fallthrough, with module SHA256 on all addresses. Ghidra queries may set `target_evidence_id` to another binary evidence item in this run as baseline, without changing the task target. Missing baselines must be reported, never replaced with the current target.

`differential_test` takes registered worker `target`/`reference`, `reference_python`, `inputs_base64` and a 1–10 second per-case timeout, with at most 64 stdin inputs of 64 KiB each. It uses existing isolated job lifecycle. `job_artifacts` imports `differential-results.json`, preserving inputs, original outputs, exit codes, both hashes and differences. Timeout, truncation and nonzero exit are inconclusive. The protocol compares stdin only; argv/stateful sessions require a separate harness.

<a id="pwn-方法堆分析"></a>

### Pwn method: heap analysis

Builtin `heap_ptmalloc` interprets allocation/free timelines using actual allocator identity, ABI and version, requiring chunk snapshots, primitive evidence, counterexamples and unknowns. Its version table is an upstream checkpoint, not a substitute for distribution patches/runtime observation. See [Pwn knowledge](../reference/pwn-knowledge.md).

<a id="固定-pwn-运行环境"></a>

### Pinning a Pwn runtime

Register supplied libc/loader with `file_put`; `runtime_profile` reads target/library SHA256, ELF Build ID and architecture. By default it confirms file identity only. An owned debugger `session_id` or `interactive_session_id` permits reading actual mappings and marking each `mapped_hash_confirmed`; arbitrary PIDs are rejected. Exited processes/unreadable modules do not imply a match.

`runtime_prepare` requires three registered ELF files: target/libc/loader. It creates independent content-addressed copies and returns direct loader/library-path/target argv for `interactive_start` or `job_start`; never concatenate a shell string. Originals remain unchanged. glibc versions, extra dependencies or loader ABI can still prevent startup; record actual results. `libc_identify` compares supplied ELF candidates against multiple leaks from the same process; without candidates it checks only the worker's own libc. A match is `candidate_only`, not mapped identity; no match does not prove a different target libc. See [Pwn evidence tools](../reference/pwn-tools.md#libc_identify) for ambiguity and limits.

<a id="结构化崩溃证据"></a>

### Structured crash evidence

`crash_snapshot` reads an owned stopped debugger session or registered core, retaining registers, PC/SP, fault address, stack, mappings and raw debugger receipts. `cyclic_offset` requires original pattern, alphabet, n and exactly n memory-order bytes, avoiding false offsets from defaults/register truncation. `input_minimize` normally distinguishes crashes by signal, module hash, PC file offset and resolved frames, refusing to weaken the baseline when GDB/module identity is absent. Explicit `allow_signal_only=true` permits weak signal-only criteria, labeled accordingly. Native stdin replay uses a pipe; debug replay uses file redirection. Check this difference for channel-sensitive targets. Snapshots are available through `job_artifacts`.

<a id="pwn-方法按需加载"></a>

### Load Pwn methods on demand

`pwn` provides shared evidence/routing. `heap_ptmalloc` covers allocator lifecycle; `rop_analysis` covers ABI, stack state, gadget semantics and path preconditions; `format_string` separates format control, argument access, leaks, writes and final triggers. Load methods for the current subtask instead of injecting every technique. Prove advanced-path preconditions first; helper-generated payloads are not proof of success.

<a id="缓解机制证据分层"></a>

### Mitigation evidence layers

`mitigation_profile` separates static declarations, worker support and owned-process runtime observations; `mitigation_analysis` interprets their experimental constraints. Debugger/interactive sessions are supported, not arbitrary PIDs. Optional `address` queries only its mapping permissions. CET ELF notes do not prove runtime enablement; SHSTK is per-thread, and missing fields remain unknown. Seccomp mode 2 does not imply a syscall allowlist. Reobserve after environment/mapping changes.

Tool rows derive filenames, module-relative addresses and decompiled function names from recorded data. Available decompiler results show a C preview linked to the original evidence ID, capped at 12 lines/1600 characters with explicit truncation. Raw parameters/results still expand on demand; previews do not establish independent verification.

Background jobs update their originating tool row and the active-operation bar from persisted worker status. Returning a job ID does not mean execution has finished. Elapsed time uses worker timestamps; attempt/success counts appear only when reported. A failed validation predicate remains a failed outcome even if the worker process completed. Jobs are bound to the run, call, agent generation and target hash; earlier-generation work is marked and excluded from current activity. These views do not change job ownership or independent verification gates.

Public narration streams into one stable message per agent generation/model request, with at most one persisted snapshot per 250 ms and a 64 Ki character display bound. Final text replaces that message. Paused, failed or disconnected controllers leave clearly marked partial text; legacy completed events remain readable. Only public text is projected, never reasoning. This does not guarantee a particular sentence, number of agents or successful analysis result.
