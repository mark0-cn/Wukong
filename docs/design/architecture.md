<a id="rust--rig-二进制逆向-agent-实现架构"></a>

# Rust / Rig binary-analysis Agent architecture

English · [简体中文](architecture.zh-CN.md)

This project implements Strix-inspired responsibilities and runtime semantics: root orchestration, dynamic children, messaging, independent conversations, sandboxed tools, Skills, evidence and independent verification. The [original plan](../history/strix-rig-binary-agent-plan.md) is a historical requirements baseline; [implementation contract](implementation-contract.md) records acceptance invariants. The controller is Rust; production execution uses Rig 0.46.0 ECS and native SubagentsPlugin.

<a id="模块与职责"></a>

## Modules and responsibilities

| Module | Responsibility |
|---|---|
| `domain` | Durable run/agent/message/event/tool/evidence/finding/verification/coverage records |
| `store` | SQLite transactions, events/mailboxes and content-addressed original artifacts |
| `coordinator` | Shared transaction entry; `runs`, `tasks`, `messages`, `waits`, `completion` manage runs, children, mail, dependencies and completion |
| `agents`, `prompts/` | Role/capability prompt assembly; `src/prompts.rs` owns builtin registration |
| `llm` | Provider configuration/native connections, typed tool summaries and summary usage hooks |
| `runner`, `bootstrap` | Run lifecycle and concrete dependency assembly |
| `kernel_host`, `model_catalog`, `usage` | Kernel-facing business APIs, role model selection, usage/budget accounting |
| `kernel` | Business integration with Rig execution/logging, child/message protocol, model/tool admission and native conversation projection |
| `tools` | One `catalog`; responsibility-grouped `args/`/`handlers/`; services own permissions/resources/evidence; `verification` centralizes independent admission/certification |
| `binary` | Address identity/SHA256; `profile` parses ELF/PE/Mach-O, `strings` extracts bounded strings, `ghidra` owns headless/cache/process lifecycle |
| `runtime`, `workers/runtime` | Isolation, durable worker, GDB/MI sessions, AFL++ jobs, crash replay/minimization |
| `mcp` | rmcp protocol, Rig definitions, namespaces, timeouts, environment and message-size limits |
| `report`, `interface` | Export, CLI, operator queries/intervention |
| `desktop`, `ui/src` | Tauri commands/workspace services, presentation adapters and React lifecycle |

<a id="一次任务如何运行"></a>

## A run's lifecycle

1. Import the target, record full SHA256/path/configuration, and create the root run/Agent.
2. Root invokes native `task` with description, prompt, role, scope and optional contract. Business persistence creates the task transactionally; Rig creates the asynchronous child and returns. Role/scope deduplication applies. Unfinished predecessors block only the child's model request, not its creator.
3. Each Agent owns a native Conversation. AgentPlugin drives model/tool effects; SessionLogPlugin stores recoverable logs; EffectLogPlugin stores effects. SQLite holds the business ledger and CLI/GUI conversation projection. Concurrency counts in-flight model requests; waiting parents consume no model slot.
4. SubagentsPlugin owns creation, requests, reports and cancellation. `wait` awaits one reachable Agent; `wait_for_agents` adds business success conditions. Completion is deduplicated per generation and bound to one native Report. Information does not release waits; new work after completion requires `follow_up_agent`.
5. Schemas/validation reuse Rig. Original results become evidence; models receive bounded summaries/IDs. Summaries use AgentRunner typed output and AgentHook metering; the host preserves identity/status. General analysis enters shared notes/todos/coverage; threat models are optional for security-boundary work.
6. Conversation retention, tool pairing and summary replacement reuse rig-memory, retaining shared-document/original-conversation evidence references. Inheritance copies only closed parent turns, never fabricating results for unresolved calls. Retries use Rig classification/Retry-After, costs use Rig Pricing, and business rules retain budget pauses.
7. Confirmed impact requires an independent verifier record. `agent_finish`/`finish_scan` pass business contract/evidence/session-cleanup gates before native completion reporting; ordinary prose cannot bypass them.

<a id="状态与恢复"></a>

## State and recovery

`TaskGraph`, `DependencyRules`, `CompletionRules`, `MessageRules` are pure domain rules independent of storage/execution. Coordination loads facts, applies rules and commits task/mail changes in one transaction; queries/generation archives remain in transaction-session implementations.

Agent states: queued, running, waiting, budget_paused, completed, stopped, crashed, failed. Completion/stop notify parents. Budget pauses retain checkpoints and resume scheduling later.

Within an active run, `follow_up_agent` (CLI `message --kind follow_up`) adds work to a completed child, preserving history/cumulative usage and starting a new generation. Calls/notices are generation-scoped. Completed parents are awakened too; stopped/failed branches need new tasks. Explicit predecessor dependencies combine with parent-completion dependencies; ancestor dependencies/indirect cycles are rejected.

Children close owned debug/interactive sessions before finishing; root resolves all sessions. Other Agents in the same run may close abandoned sessions of stopped/failed owners, but cannot issue debug commands/interactive input.

Native logs in `state_dir/rig/RUN_ID` contain model conversations, effects and provider metadata, not clients/credentials. Resolved provider/endpoint configuration is pinned before first model I/O and reused on resume. Per-role request output budgets do not overwrite model capacity. Keys use configured environment variables. Fresh tasks use schema 4; old databases/custom AgentRun checkpoints are not migrated.

`DurableEffects` centralizes admission, stable identities, transactional/worker receipts and recovery. Business call IDs combine Agent, generation, metered model turn and provider tool-call ID, not reassigned ECS IDs. Business state/receipts commit together. Worker replies persist before controller postprocessing; missing replies are reconciled using the same request ID against the durable worker ledger. Uncertain MCP writes remain for operator reconciliation, never automatic replay. Operator reconciliation also uses epoch leases. Native effects/database changes wake execution.

`KernelHost` exposes concrete business operations to Rig; the kernel does not access Store, Coordinator, ToolService or full configuration. `ModelCatalog` owns resolved role models; `UsageService` atomically updates budgets/usage; `bootstrap` creates concrete clients/services.

`RunSupervisor` independently manages controller leases, duration budgets and background worker observation; Rig frames do not await worker RPC. Leases carry persistent increasing versions. Runner/coordinator/tools share versioned store handles; every write transaction validates owner/version/expiry before mutation. Expired controllers stop and late results cannot overwrite a successor. Supervision ends with run completion/cancellation; lease release also checks version.

CLI/GUI use `AssessmentService` for start/resume/pause/stop/operator messages. `PreparedAssessment` retains resolved configuration/event subscriptions and closes controller tool connections on success/error. Frontends do not orchestrate leases or construct resume configuration. Resume preparation does not mutate tasks; budget changes commit only after the supervisor acquires the lease.

`FilesService`, `JobService`, `SessionsService`, `AnalysisService` hold scoped configuration/dependencies. `WorkerGateway` centralizes transport/stable receipts. Controller ownership uses `JobRecord`, `SessionRecord`, `WorkerFileRecord`, `RunScoped`; original worker extension payloads remain JSON.

`JobService` unifies automatic monitoring/manual status: check ownership, parse typed `JobStatus`, import terminal raw evidence/artifacts, then notify models. Per-job observation is serialized and merges the latest ledger to avoid overwriting artifacts/notices. `lost` is terminal; exit/verification flags come from `result`. Notices/completion marks commit together to the durable mailbox, sharing atomic checkpoint/acknowledgment behavior. Unacknowledged mail redelivers after restart; late results from old generations become stale. Worker RPC `error.data.kind` carries categories used by session/recovery reconciliation.

Runtime roles are only `root`, `analyst`, `verifier`. Root decomposes/dispatches/coordinates/delivers; analysts inspect targets; verifiers independently check other instances. Child `role` accepts only analyst/verifier. Roles determine models, budgets and permissions; Skills grant none.

`recon`, `reverse`, `hunter`, `fuzz`, `crash`, `exploit`, `reporter` are capabilities; `formats`, `types`, `algorithms`, `state_machine`, `binary_diff` and others are methods. An analyst may switch methods within scope without creating successor tasks solely for capability changes. Active same-scope analysts share deduplication; independent verifiers may use that scope separately. Hunters may start from a bounded research scope without a predetermined vulnerability hypothesis. Root handles ordinary reporting; delegate a reporter analyst only for large report artifacts.

Controller-installed role instructions cannot load as extra Skills. `load_skill` saves the local body/SHA256/original evidence as an active instruction snapshot, effective in the next fresh model request. Frozen requests remain unchanged; source edits do not mutate loaded snapshots. A same-name active snapshot replaces its initial body. Dynamic instructions bypass result summarization and survive compaction/recovery. Limit: 64 KiB per Skill, at most five distinct initial/dynamic names combined. Capabilities add methods/evidence/artifacts; the primary role owns completion. Successful recovery continues the original task.

`prompts/base.md` contains shared rules; `roles/` identity/lifecycle; `capabilities/` specialist abilities; `methods/` techniques; `support/` tools/recovery; `examples/` substantive demonstrations. `src/prompts.rs` registers names/categories/purposes/bodies/optional examples, deriving discovery, loading, errors and role-Skill restrictions. Discovery includes only names/purposes; bodies/examples assemble by role/selection. Redundant short fuzz/crash examples were removed.

Callable names remain `reverse`, `hunter`, etc., without directories. Flat `skills_dir/<name>.md` files override/add bodies; `demos_dir` defaults to `prompts/examples` or a custom flat directory. Missing directories skip examples; embedded builtin bodies do not need runtime repository files.

Each fresh model request selects at most six bounded shared-note/todo/optional-threat-model previews by author, parent, predecessor, scope and keywords. This heuristic does not guarantee full relevance coverage; prompts report omitted counts/search entry points. `context_search` supports text, kind, author scope, agent_id and pagination; `context_read` reads original document JSON including amendments by Unicode character offset. Both shrink pages to output-byte limits, retain continuation offsets and bypass model summaries. Reads stay within the run; `evidence_read` retrieves original evidence. Compaction retains the same previews, ledger counts and full-conversation references, never reinjecting every document/tree. Contracts still define plans/constraints; selection does not replace acceptance.

`src/tools/catalog.rs` registers builtins and their backend requirements. Before each model request, asynchronous environment preflight selects the role-visible tools supported by observed capabilities; root orchestration stays lazy. Failed preflight preserves controller tools and reports unavailable requirements. Native coordination tools are `task`, `message`, `wait`. Root uses `finish_scan`, children `agent_finish`; actual operations recheck backend capabilities, target identity, evidence, session ownership and independent review. Definitions drive execution, validation and the [generated catalog](../reference/tool-catalog.md), without persisting tool-discovery selections. Connected MCP definitions merge in; their capabilities still need probing. `wukong tool-catalog` generates Markdown; `wukong --json tool-catalog` exports schemas.

`prompts/coverage.json` associates method sources/tools/mechanism fixtures with `SkillManifest`; `wukong skill-catalog` generates [coverage](../reference/skill-coverage.md). Metadata does not change permissions, availability or completion gates.

Worker entry, transport, ledger, jobs, independent supervision, debugging, interaction, files and command execution are separate components. `Runtime` only assembles/routes; the ledger executes callbacks/reconciles session liveness independently of a concrete runtime.

Named [analysis environments](cross-platform-environments.md) connect lazily and bind immutably to child tasks. Resource identities include the worker; shared artifacts move through hash-verified controller storage with bounded resumable transfers.

External processes recover separately from model state. Reconcile debugger/fuzz/job liveness; saved PIDs/SQLite do not prove survival. Reuse completed effects; never blindly replay uncertain ones. MCP readOnlyHint follows Rig hint semantics and does not authorize uncertain remote replay. GDB mutation is owner-serialized; parallel experiments use separate processes. The native kernel registers only allowed business tools, not host shell/general coding tools.

<a id="桌面读取与持久化边界"></a>

## Desktop reads and persistence boundaries

`queries` supplies GUI-independent `WorkspaceQueries`, `AssessmentQueries`, `ReportQueries` for summaries, incremental cursors, conversation/report snapshots. SQL lives in `store/workspace`/`store/report_queries`. CLI/GUI share query models; chat parsing/layout caches remain GUI adapters.

`desktop/workspace` reads light run/Agent summaries through `store/desktop_queries` in one snapshot. Response signatures combine epoch, `entity_revisions`, event sequence, query scope and operator mail; unchanged data is not resent, while independent CLI changes remain observable. Active history defaults to latest 500, expandable to 10,000; only calls referenced by that window load. `useWorkspace` rejects stale responses and reloads after task/storage changes.

Business writes use `StateSession`, not exposed SQLite connections. Storage `StoredRecord` binds entity type/location/identity/conversation policy. Business code uses `load`, `records`, `save_record`, typed document/conversation categories and specific result-archive operations. Message replacement, old-generation archives and unread counts remain encapsulated. Receipts/state share lease validation and one IMMEDIATE transaction.

SQLite uses WAL, independent read-only connections and one writer. Reads see consistent transactions; writes remain IMMEDIATE. Indexes cover run/Agent/event-kind/sequence queries. `entities` Agent JSON contains metadata only; `agent_conversations` separately stores checkpoints/inherited history/revisions. Budgets/state neither read nor rewrite conversations. Full reads combine them for inheritance, follow-ups, reports and recovery checks; native Rig logs still drive execution recovery.

`AgentMetadata` excludes checkpoints/history and is the metadata read/write type. `AgentConversation` is the optional conversation payload. `AgentSnapshot` explicitly combines them for inheritance/recovery/reports/chat. Full snapshots use flat JSON; hot paths use metadata.

`desktop/presentation` combines public progress/operator messages/tool results into DTOs. Frontend `Conversation` uses assistant-ui and virtualization. `ui/src/commands.ts` maps command names/arguments/results; `bridge` invokes them. Type checking does not replace Rust permission/epoch validation. `useDesktopLifecycle` handles close confirmation/native drops/keyboard listeners; `useStartupUpdates` handles startup checks. Styles retain fixed import order with separate color/layout/component ownership. See [maintenance](../development/maintenance.md) for boundaries/benchmarks.

<a id="地址与静态分析"></a>

## Addresses and static analysis

Address references carry sample SHA256 and file_offset, module_relative or runtime space. Runtime references also identify run instance/load_base. module_relative starts at the profile's image_base; file offsets are not debugger virtual addresses.

Rust `object` parses actual ELF/PE/Mach-O architecture, width, endianness, sections, symbols, dependencies and imports. Mach-O `LC_MAIN` file offsets map to module addresses before module-relative output. Inputs/lists/strings are bounded; overflow/parse errors are explicit.

Mitigations report enabled, disabled, partial, unknown with raw basis: ELF program headers/dynamic flags, PE DLL characteristics, Mach-O header flags. Canary symbols prove symbol presence, not universal function protection; static ASLR flags do not establish OS runtime policy. Uncertainty remains unknown.

Ghidra uses real headless import/analysis and a Java bridge. Functions use the function manager; decompilation uses `DecompInterface`; xrefs use the reference manager. Analysis timeout, no functions, decompilation failure and hash mismatch are errors. Full decompilation is a separate `.c` artifact.

Cache identity includes target SHA256, Ghidra version/configuration and launcher/bridge hashes. First import persists a project; later queries use `-process -noanalysis`. File locks enforce one writer; each request retains parameters, response, stdout/stderr. External/register/stack addresses retain Ghidra spaces, never pretend to be module-relative.

<a id="动态验证与评测边界"></a>

## Dynamic verification and evaluation limits

`tools/verification` centralizes admission/evidence certification; `jobs` owns general query/artifact import. `verification_protocol` types worker identity, counts, network bindings and per-attempt observations. Full original JSON remains evidence; valid types do not establish impact. Network-binding extensions participate in identity comparison rather than discarding unparsed constraints.

Record static hypotheses, crash reproduction, root cause, exploitation primitives and final impact separately. Replay binds target hash, arguments, input and environment. Minimization reruns predicates, never substituting input length or a string for actual reproduction.

`fixtures/` contains symbol/stripped vulnerable, fixed and normal samples. Builders generate ELF; behavior harnesses replay and save hashes/exits. Accuracy uses independent reports/explicit review labels, not crashes as Agent success. See [fixtures](../../fixtures/README.md).

Dynamic tools target Linux x86_64 ELF; PE/Mach-O static profiling is available, but DbgEng/LLDB dynamic backends are not implemented. Current platform acceptance follows CI and [desktop update validation](../development/desktop-updates.md); historical native-kernel checks do not establish current Windows Docker Desktop coverage. Providers/engines need actual dependencies; missing dependencies produce explicit errors. Local protocol tests do not establish live-provider/engine acceptance.

<a id="类型化协调消息"></a>

## Typed coordination messages

Messages carry kind, correlation/reply IDs, recipient generation, sender generation,
current task identity and evidence references. `send_message_to_agent` defaults to
information: it neither reopens completed work nor interrupts a wait. Questions,
replies, results and blocked notices can interrupt a waiter without erasing its
wait condition. Only explicit `follow_up_agent` reopens completed work.

`queued` means durably enqueued; `included` means accepted into the native
conversation/context and journal, not that a model request ran or acted on it.
`replied` records an explicit
correlated reply (not an assessment of correctness). Old-generation replies are
retained as `stale` and never injected as current work. `message_status` exposes
these states. In-flight model requests keep their original mailbox snapshot.
This increment uses a fresh schema 4 store; no old-database migration is provided.
Informational mail left on completed work becomes
stale when a new generation starts. CLI messages default to information; GUI
operator input defaults to information; the explicit follow-up option reopens completed work.

The native `message` tool opens a request or sends a reply to a reachable active
agent. Use typed `send_message_to_agent` for informational updates and correlated
mail, and `follow_up_agent` for additional work after completion. Native wait
delivery and typed mailbox receipts are projected together; receipt inclusion
does not prove that the model processed or answered the message.

<a id="任务契约和结果"></a>

## Task contracts and results

`task.contract` optionally declares an objective, input evidence IDs,
constraints, required named deliverables and completion-condition labels. A task
without extra requirements uses its task text as the objective.
The contract is installed in the specialist prompt and survives checkpoints.
An active role/scope cannot silently be reused with a conflicting contract.

`agent_finish`/`finish_scan` accept `result`: outcome (success, inconclusive,
blocked, cancelled), answer, evidence IDs, named evidence artifacts, completed
condition labels, unknowns and next actions. Summary-only completion is a shorthand
for tasks without required artifacts or conditions. Success checks required artifact names, linked raw
evidence integrity and explicit coverage of declared conditions. These are
completeness/provenance checks, not proof that a model's conclusion is correct.
Non-success outcomes must identify unknowns or next actions and may omit missing
outputs. Cancelled work becomes stopped; blocked/inconclusive completion records
retain their distinct outcome for dependency evaluation.

Results are attached to the terminal notification and agent graph, and archived
per task/agent ID and generation in exported `snapshot.task_results`. A follow-up
clears the current result without deleting the archived previous generation.
The existing agent ID is the task identity in this increment; independent task
assignment/worker attempts remain a later extension.

<a id="依赖准入与等待"></a>

## Dependency admission and waits

New children default to `dependency_policy=all_success`. `all_terminal` admits a
collector after every prerequisite ends, including failures. `any_success` admits
an alternative consumer after one succeeds and blocks only when all alternatives
end without success. A success requires completed execution plus a success task
result; inconclusive/blocked results do not qualify. Missing prerequisites are
errors, never assumed successes.

Before model scheduling, a transaction either releases the gate or records a
blocked result, fails the unstarted child, archives its result and notifies the
parent. Released models receive the predecessor results in their request context.
Failure propagates through dependent children without model calls. The
release event records producer generations/outcomes; once admitted, later producer
follow-ups do not revoke the consumer. New dependent work evaluates the latest
producer result. This is admission ordering, not automatic live dataflow: required
input evidence still belongs in the task contract.

`wait_for_agents.policy` defaults to all_terminal and also supports all_success
and any_success. Its return/wakeup includes a condition with state pending,
satisfied or blocked, separate from timeout. Both creation gates and runtime wait
edges join the parent-completion dependency graph; cyclic waits are rejected
transactionally. Wait parking and tool receipts commit together, so replay cannot
park the same turn a second time after a lost response.

The desktop transcript projects persisted calls, worker jobs and bounded public model replies. `progress` stores public snapshots using existing transaction/revision notifications; the kernel reads Rig `Partial.text` and delegates persistence through `KernelHost`. Rig retains streaming/retry ownership. Reply identity uses agent/generation/metered turn, not ECS entity IDs; late generations/turns cannot replace newer replies. Start/final events reference one reply while intermediate snapshots replace its record. Lease expiry participates in desktop revisions and marks unfinished text interrupted. Public text is presentation only and never releases completion or evidence gates.
