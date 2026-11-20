<a id="实现契约"></a>

# Implementation contract

English · [简体中文](implementation-contract.zh-CN.md)

The architecture in [the historical plan](../history/strix-rig-binary-agent-plan.md) is the requirement baseline. This document records the Rust implementation contract and the evidence required for acceptance.

<a id="模块归属"></a>

## Module ownership

`domain` defines durable records; `storage` owns transactional SQLite persistence and content-addressed artifacts; `application/coordinator` implements business task, contract and mailbox semantics; `prompts` registers built-in metadata/resources and `application/agents` renders selected role/skill prompts; `adapters/models` adapts provider connections; `application/assessment/runner` owns run lifecycle through supervision and business services for leases, usage accounting and events; `application/agent_runtime` integrates the native Rig ECS execution, session logs and subagent protocol with business admission/completion gates; `application/tools` dispatches typed operations using Rig schemas; `adapters/binary` and `adapters/worker` implement external backends; `adapters/mcp` uses rmcp for third-party protocol handling; `interfaces/report` exports evidence-backed findings; `interfaces/cli` exposes CLI commands; `interfaces/desktop` and `ui/src` provide the native desktop workspace.

<a id="必须保持的不变量"></a>

## Required invariants

New tasks default to a private conversation containing their brief and evidence references; `inherit_context=true` explicitly opts into closed parent history. Explicit follow-ups retain the same task's own history. Agent graph queries default to eight compact entries per page (maximum sixteen); follow `next_offset`, or request `agent_id` for full details. Coordination resolves full IDs or unique prefixes of at least eight characters within the caller's run before applying permission checks, and persists canonical IDs. A task cannot delegate its own or an ancestor's already-owned scope.

1. Child creation persists and returns before child work completes. Dependency admission defaults to all_success; all_terminal and any_success are explicit. Unsatisfiable requirements block successors before model execution and notify the parent. Business runtime waits and creation dependencies share cycle validation; native request/wait lifecycle checks remain Rig-owned. Parent/child identities, scope and skills survive restart.
2. Messages are durable; included receipts acknowledge acceptance into the native conversation/context, not model execution or processing. Terminal child notifications are deduplicated transactionally per activation and bound to native reports. In an active run, a completed child accepts explicit follow_up_agent assignments with prior history and cumulative usage retained; generation separates new effects and notices from old receipts. Stopped/failed branches and terminal assessments cannot be reopened by messages. Information does not wake waits or reopen tasks. Included mail is distinct from a correlated reply; stale generation replies remain audit records and are not injected.
3. Root orchestration never invokes target-analysis/execution tools. A root can only finish after active descendants terminate or are explicitly accounted for.
4. An analysis shares notes, to-dos, findings and coverage; threat models are optional for security-boundary work. Each agent has a separate checkpoint and conversation. Compaction includes every shared document kind and retains the original checkpoint as evidence; coordinator messages enter compressible conversation history, never an ever-growing system preamble.
5. Agent and tool state transitions are durable. Completed side-effecting calls are not blindly re-executed. Ambiguous in-flight effects require worker reconciliation.
6. Debugger mutation is serialized by session owner. Agents close owned debugger and interactive sessions before completion; root completion checks all sessions. Another in-run agent may close a terminal owner's orphaned session, but cannot send it commands. Long-running fuzz jobs use job handles and can be reconciled independently of model state.
7. Addresses carry target/module identity and address space. Cache and evidence refer to target/tool/configuration identities.
8. Finding impact promotion requires an independent verifier record and linked raw evidence. Static suspicion or a crash alone does not prove an exploit primitive.
9. Tool results are bounded in model context and retain complete hashed artifacts for later retrieval.
10. Models never receive an execution path outside configured tool backends; model credentials remain on the controller.
11. Task contracts declare inputs, constraints, deliverables and completion conditions. Successful results account for required outputs and link original evidence; non-success results explain gaps. Each generation's result is archived. Shape/provenance validation is not independent verification of truth. This increment targets fresh stores/tasks; no previous-schema migration is provided.
12. Native task creation returns before predecessor completion; business dependency gates hold model IO and supply released predecessor results. Rig owns generic effects, retries, conversation pairing and subagent reports. Business tool reconciliation uses stable wire call identities, not restored ECS entity/effect IDs. Inherited context contains only closed parent turns.
13. Resolved model endpoint/configuration is saved before model IO. Resume uses that configuration even if endpoint environment defaults change. Role output budgets remain distinct from model capacity. Missing usage/cost information preserves the response and pauses before tool IO. Remote MCP read-only hints cannot authorize replay of uncertain effects.
14. Runtime roles are root, analyst and verifier. Capability skills supplement methods and artifacts without changing the assigned role, permissions or completion protocol. Role instructions cannot be added as skills. Analyst work in the same active scope shares a deduplication owner across capabilities; independent verification uses a separate verifier instance. Ordinary reporting belongs to root.
15. Execution completion and evidence readiness are distinct. `job_execution_finished` is informational; `job_completed` requires confirmed cleanup, complete evidence and postprocessing. Verifier certification and root success require ready evidence. Missing evidence must remain an explicit gap in an inconclusive result.

<a id="验收证据"></a>

## Acceptance evidence

Use unit/integration tests for transactional lifecycle, concurrent messages, deduplication, checkpoint replay, finding gates, tool path confinement and real binary parsing. Exercise the actual CLI end to end. Exercise external adapters against their tools when installed; clearly label adapter tests and environment-dependent tests rather than treating a mock protocol as full Ghidra/GDB/AFL verification.

<a id="交付审计"></a>

## Delivery audit

Track CLI/config, dynamic scheduling, persistence/resume, static analysis, Ghidra, GDB/MI, AFL++, crash reduction, independent verification, skills, MCP, GUI, JSON/Markdown/HTML reports, replayable artifacts, Docker runtime, evaluation fixtures and regression tests. Do not mark the project complete on compilation alone.

<a id="结构化方法产物"></a>

## Structured method deliverables

Generated scripts advertising self-checks declare `artifact_acceptance: {"deliverable_name":"self_check_v1"}` in the task contract. The producer may finish before acceptance; successful root completion requires an independent verifier's passed record for the final content hash, target and environment. `follow_up_agent.contract` can assign new deliverables to a completed owned child, retaining its constraints, conversation and cumulative accounting. Stale messages cannot change its contract; generation result archives retain the contract used for that result. Existing contracts default to no added requirements and are not retroactively certified.

The `validation` method describes a standalone JSON evaluation protocol for Python and POSIX Shell. A task declaring `self_check_v1` automatically receives the effective `validation` instructions, including on follow-up, even when the caller omitted that skill. Explicit selection or an active snapshot takes precedence without duplicate injection; roles, permissions and skill quotas remain unchanged. `artifact_validate` requires verifier-owned `baseline.json` from the exact script's recorded successful job and a `validator_integration` check of shared real/injected evaluator wiring. A trusted worker driver tests abnormal exits/signals after expected text, timeout, incomplete or missing observations and contradictions. This establishes bounded predicate behavior, not whole-script correctness or real occurrence of simulated faults. Normal reproduction and independent target controls remain separate.

Canonical artifact names `formats`, `algorithms`, `state_machine`, `binary_diff` require a matching `structured.kind` and 1–256 rows. Every row references same-run `evidence_id`. Required row fields: formats: offset/width/encoding/meaning/confidence; algorithms: input/expected/observed/arithmetic; state_machine: from/action/to/expected/observed; binary_diff: baseline_sha256/target_sha256/before/after. Digests are 64 hexadecimal characters. The original artifact remains linked separately; structural completeness is not correctness. An independent verifier must challenge the semantic claims. Legacy noncanonical artifact names remain supported.

Ordinary deliverables need only the exact contract name and original `evidence_id`; omit `structured` when no method rows are required. Completed conditions use the exact contract labels. Validation identifies missing deliverable names and lists all missing fields in the first invalid row, so correction does not require guessing one field per turn.

Static Ghidra address tools accept the target-bound module-relative `AddressRef` emitted by binary inspection, or a legacy integer byte offset. References with another module hash or another address space are rejected; mapped-segment bounds diagnose common VA/offset mistakes before backend work. Integers cannot prove their original address space, so prefer references. Worker-file errors distinguish target staging, script inputs and evidence retrieval; immutable content conflicts suggest a content-specific new path.

Run lifecycle transitions are owned by the controller lease. A controller epoch uses one durable resume operation identity; replaying that identity neither resets agents nor emits another start/resume event, and changed arguments are rejected. Pausing an already paused run is a no-op and preserves the original structured cause (`operator`, `interrupted`, or `budget`). The stored `budget_paused` status remains compatible; operator/interruption events use `run.paused`, while budget gates retain `run.budget_paused`. Desktop event attachment precedes the initial snapshot; serial revision-based snapshots, polling and focus/visibility catch-up recover missed notifications without appending duplicate messages.
