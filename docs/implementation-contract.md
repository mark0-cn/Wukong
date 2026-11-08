# Implementation contract

The architecture in `strix-rig-binary-agent-plan.md` is the requirement baseline. This document records the Rust implementation contract and the evidence required for acceptance.

## Module ownership

`domain` defines durable records; `store` owns transactional SQLite persistence and content-addressed artifacts; `coordinator` implements business task, contract and mailbox semantics; `prompts` registers built-in metadata/resources and `agents` renders selected role/skill prompts; `llm` adapts provider connections and typed summaries; `runner` owns leases, usage accounting and events; `kernel` integrates the native Rig ECS execution, session logs and subagent protocol with business admission/completion gates; `tools` dispatches typed operations using Rig schemas; `binary` and `runtime` implement external backends; `mcp` uses rmcp for third-party protocol handling; `report` exports evidence-backed findings; `interface` exposes CLI and TUI.

## Required invariants

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

## Acceptance evidence

Use unit/integration tests for transactional lifecycle, concurrent messages, deduplication, checkpoint replay, finding gates, tool path confinement and real binary parsing. Exercise the actual CLI end to end. Exercise external adapters against their tools when installed; clearly label adapter tests and environment-dependent tests rather than treating a mock protocol as full Ghidra/GDB/AFL verification.

## Delivery audit

Track CLI/config, dynamic scheduling, persistence/resume, static analysis, Ghidra, GDB/MI, AFL++, crash reduction, independent verification, skills, MCP, TUI, JSON/Markdown/HTML reports, replayable artifacts, Docker runtime, evaluation fixtures and regression tests. Do not mark the project complete on compilation alone.
