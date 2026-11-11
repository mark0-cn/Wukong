# Built-in tool catalog

English · [简体中文](tool-catalog.zh-CN.md)

Generated from the registry by `wukong tool-catalog`. Do not edit manually.

Rig provides native coordination tools `task`, `message`, `wait`; MCP tools depend on configuration. This table lists model-visible business tools and lifecycle visibility. Execution still checks target identity, evidence, session ownership and backend capabilities. Translated descriptions are documentation only and do not change the model protocol.

| Tool | Visible roles | Side effects | Purpose |
|---|---|---|---|
| `agent_finish` | analyst / verifier | yes | Finish a child task with a structured result accounting for its contract, evidence and gaps. summary is shorthand for tasks without required outputs and is ignored when result is provided (result.answer is the canonical summary). Active descendants must finish first. |
| `analysis_verify` | analyst / verifier | yes | Verifier only: record an independent general-analysis verdict with expected and observed results, scope, counterexamples and verifier-owned evidence. Does not promote security impact. Records are immutable and appear in shared notes. |
| `angr_solve` | analyst / verifier | yes | Bounded symbolic input reconstruction for stdin validation paths. Supply module-relative success/failure addresses and expected input length. Returns a background job with candidate input and exploration statistics. Prefer direct inversion for simple reversible transforms; slice or use explicit constraints for large state spaces. Always replay candidate bytes on the real target. |
| `artifact_stage` | analyst / verifier | yes | Stage a hash-verified same-run evidence artifact into your assigned environment using bounded resumable transfer; returns the worker file and original evidence ID. |
| `binary_inspect` | analyst / verifier | no | Inspect the exact run target, architecture, sections, imports, symbols and mitigations. |
| `binary_strings` | analyst / verifier | no | Search printable strings with file-offset address references and pagination. |
| `context_read` | all | no | Read a bounded character range of an exact same-run shared document as JSON, including amendments. Use kind and id from context_search; follow content.next_offset until the required content is complete. |
| `context_search` | all | no | Search same-run shared notes, todos and optional threat models by text, kind, author scope or agent_id. Returns bounded previews and next_offset; use context_read for exact content including amendments. |
| `coverage_list` | all | no | List all recorded assessment coverage. |
| `coverage_record` | all | yes | Record inspected scope, outcome and supporting evidence. needs_follow_up blocks root completion. |
| `coverage_update` | all | yes | Update a coverage record's outcome, notes and evidence. |
| `crash_replay` | analyst / verifier | yes | Start a persistent clean-directory crash reproduction job. |
| `crash_snapshot` | analyst / verifier | no | Capture registers, PC/SP, fault address, instructions, stack and module identities from an owned stopped debugger; alternatively inspect an in-run core file. Strong signatures require observed module hashes. Unknown identity remains unresolved. |
| `cyclic_offset` | analyst / verifier | no | Find a unique offset in an in-run cyclic pattern. Requires the original alphabet/n and exactly n observed bytes in memory order. Rejects mismatched generation parameters; never guesses endianness or truncates a register implicitly. |
| `debug_close` | analyst / verifier | yes | Close your debugger session, or clean up an in-run session whose owner is terminal. |
| `debug_command` | analyst / verifier | yes | Execute exactly one GDB/MI command in your owned session; stable request IDs prevent blind replay. |
| `debug_control` | analyst / verifier | yes | Control your owned live debugger using typed breakpoint and execution actions. Dump sessions reject execution control; stable request IDs prevent blind replay. |
| `debug_inspect` | analyst / verifier | no | Inspect bounded registers, stack, memory, modules, threads, instructions, exception or execution state in your owned debugger session; full engine transcripts remain evidence. |
| `debug_start` | analyst / verifier | yes | Start an owned debugger session on the staged target, or inspect a separately staged dump. Engine selection never falls back to another platform. |
| `differential_test` | analyst / verifier | yes | Compare the staged target and reference on bounded stdin vectors in an isolated background job. Preserve raw outputs, hashes and mismatches as artifacts. Timeout or truncated output is inconclusive, never a match. |
| `emulate_run` | analyst / verifier | no | Bounded Unicorn emulation of the staged target from a start address: maps the ELF segments plus a stack, executes up to count instructions or until the PC reaches the until address (whichever first), and returns registers plus requested memory dumps (data_base64). The target never runs natively — this is a pure simulation for VM-style or hand-crafted code (interpreter loops, x87/FPU blocks) without hand-building Unicorn harnesses. Use init_regs for register preconditions and memory to inject bytes. The watchdog timeout bounds runaway loops; unsupported instructions report the failing address in error. |
| `environment_doctor` | all | no | Probe a registered environment's real platform and tools; children may probe only their assigned environment. |
| `environment_list` | all | no | List registered analysis environments without connecting; configuration is not proof of readiness. |
| `evidence_read` | all | no | Read a bounded byte range from an integrity-checked evidence artifact. Non-UTF8 is base64. |
| `exec_command` | analyst / verifier | yes | Run ONE bounded shell command synchronously in the worker and return its stdout/stderr/exit code in the result. The command runs in a fresh temporary directory: pass target=<staged path from stage_target> and the resolved file is available at $TARGET in the environment ("printf '<payload>' \| $TARGET"). Prefer this for quick inspection and stdin-driven PoCs; long-running work must go to job_start with output redirected to a log file instead of blocking here. Worker paths (files/...) resolve only as direct argv entries, never inside shell strings; use $WUKONG_STATE_DIR for the state root. |
| `file_get` | analyst / verifier | no | Retrieve a declared in-run worker file and persist original bytes as evidence. |
| `file_put` | analyst / verifier | yes | Create an immutable worker file from base64 bytes (data_base64), UTF-8 text (text — preferred for scripts: no base64 encoding needed) or an existing evidence artifact. evidence_id must be the CONTENT evidence id — the record whose bytes you actually want (a tool result's content_evidence_id), NOT the tool-result envelope id: passing the envelope id writes the envelope JSON, not the bytes. Use executable=true when the file will be executed as a program. |
| `finding_file` | all | yes | File a suspected or reproduced finding with evidence. Verified impact requires a separate immutable verifier record. severity critical/high requires a reproduced-or-better stage. |
| `finding_get` | all | no | Read a finding and its independent verification record. |
| `finding_update` | all | yes | Update an existing finding. Impact promotion is enforced using independent execution evidence. |
| `findings_list` | all | no | List this assessment's findings and recorded stages. |
| `finish_scan` | root | yes | Root only: finish the assessment after children and coverage are resolved. |
| `follow_up_agent` | all | yes | Explicitly assign a follow-up; reopens completed work in a new generation while preserving history. Use recipient_generation to guard against stale assignments. |
| `function_decompile` | analyst / verifier | no | Ghidra: decompile a function by MODULE-RELATIVE offset. |
| `function_instructions` | analyst / verifier | no | Ghidra: paginated function instructions and control-flow edges, including calls. Addresses are module-relative and bound to target SHA256. Optional target_evidence_id selects an in-run baseline binary. |
| `functions_search` | analyst / verifier | no | Ghidra: search analyzed functions, paginated. |
| `fuzz_start` | analyst / verifier | yes | Start AFL++ QEMU fuzzing after staging target and nonempty seed files. |
| `fuzz_status` | analyst / verifier | no | Observe a bound AFL++ job and its real statistics/crash files. |
| `input_minimize` | analyst / verifier | yes | Minimize a crashing input with fresh module-hash, PC-offset and frame signatures; requires GDB. allow_signal_only explicitly permits weaker signal-only equivalence. Poll job_status and collect job_artifacts. |
| `interactive_close` | analyst / verifier | yes | Close your interactive session and terminate its process tree; also permits cleanup when its in-run owner is terminal. |
| `interactive_send` | analyst / verifier | yes | Write bytes to an owned interactive session's stdin and return its newly produced output. The session stays alive between calls. |
| `interactive_start` | analyst / verifier | yes | Start a persistent interactive process (REPL-style program) owned by this agent; returns a session_id. Worker paths resolve as direct argv entries. |
| `job_artifacts` | analyst / verifier | no | List and import original artifacts produced by an in-run job. |
| `job_cancel` | analyst / verifier | yes | Request cancellation of an owned in-run job. |
| `job_start` | analyst / verifier | yes | Start a persistent process job in the configured worker; returns a job_id immediately. argv entries may use {target} as a placeholder for the staged target path (resolved by the harness and staged in isolated jobs); worker paths like files/... resolve only as direct argv entries, never inside shell strings. Jobs may read WUKONG_STATE_DIR and WUKONG_JOB_DIR from the environment. |
| `job_status` | analyst / verifier | no | Observe an in-run job, refresh persisted status and import raw observations. |
| `libc_identify` | analyst / verifier | no | Compare same-process symbol leaks against up to 16 supplied in-run ELF libc candidates, or only the worker's libc when omitted. Preserve candidate hashes, Build IDs, architecture, inconsistent leaks and ambiguous matches. Candidates never confirm the mapped runtime; no match describes only the searched corpus. |
| `load_skill` | all | yes | Activate a named capability or method (built-in or configured custom skill). Role instructions root/analyst/verifier are automatic and cannot be loaded here. Initial and active capabilities share a limit of 5 distinct names. Exact content/hash is persisted for the next fresh model boundary; replaces an initial copy of the same skill and survives compaction/resume. Loading never changes role, permissions or task completion criteria. |
| `message_status` | all | no | Inspect a sent/received message, its delivery state and correlated reply. |
| `mitigation_profile` | analyst / verifier | no | Separate binary declarations, worker environment support and owned process/thread protection observations; optional address checks the actual containing mapping. |
| `note_save` | all | yes | Save a shared note linked to this task. |
| `notes_list` | all | no | List shared notes for this assessment. |
| `poc_verify` | analyst / verifier | yes | Independently replay a finding in fresh trial directories. Use argv/stdin delivery, or a bounded declarative network plan with the host-prepared launch_spec. Network trials use fresh TCP connections, capture/patch ASLR-dependent values and hash raw transcripts; server state is not reset and deployed runtime identity remains unconfirmed. Worker observations determine success. |
| `py_run` | analyst / verifier | yes | Run a Python script in the worker in ONE step: pass the source text directly (no base64, no file_put). The worker syntax-checks it and returns the compiler error inline before spending a job; then runs it as a background job (job_status for stdout/stderr). Optional argv entries arrive as sys.argv[1..]; an optional target=<staged path> is appended LAST to argv and staged automatically in isolated jobs — read it from sys.argv[-1]. Prefer this over file_put+job_start for any script longer than one line. |
| `runtime_doctor` | all | no | Inspect the configured worker's real platform, versions and available capabilities. |
| `runtime_prepare` | analyst / verifier | yes | Create an immutable execution copy of supplied target, libc and loader and return direct launch argv. Requires all three ELF files with matching architecture. Does not execute or modify originals; verify actual mappings separately. |
| `runtime_profile` | analyst / verifier | no | Inspect SHA256, ELF Build ID and architecture of in-run target/libc/loader files. Optionally observe actual mappings of an owned live debugger session. File candidates never imply a confirmed mapped runtime. |
| `seccomp_analyze` | analyst / verifier | no | Decode an in-run raw classic-BPF seccomp filter into instructions, branch targets and actions; optionally evaluate concrete architecture/syscall/argument/IP probes. An artifact is not a confirmed installed policy, and probes are not an exhaustive allowlist. |
| `send_message_to_agent` | all | yes | Queue information, a question, reply, result or blocked notice. Information does not interrupt waits. Only follow_up_agent reopens completed work. Queued/included is not proof of processing. |
| `stage_target` | analyst / verifier | yes | Copy the run's SHA256-verified original target into the worker's immutable files area. |
| `stop_agent` | all | yes | Stop yourself or your descendants, with a recorded reason. |
| `think` | all | yes | Record a concise audited scratch note for this agent. The original content remains in tool-call history and may be inherited by children; this is neither a private channel nor a context-saving mechanism. Use note_save for shared conclusions. |
| `threat_model_amend` | all | yes | Append a durable amendment to a shared threat model. |
| `threat_model_get` | all | no | Read the shared threat model documents. |
| `threat_model_save` | all | yes | Create a shared threat model with a stable document ID. |
| `todo_save` | all | yes | Create a shared to-do item; status defaults to pending. |
| `todo_update` | all | yes | Update the body or status of an existing shared to-do. |
| `todos_list` | all | no | List shared to-do items. |
| `verification_record` | analyst / verifier | yes | Verifier only: ingest a completed poc_verify job, its raw evidence and real trial counts. A crash verifies only crash behavior. |
| `view_agent_graph` | all | no | List this assessment's agent tree, scopes, states and results. |
| `wait_for_agents` | all | yes | Park on selected agents with policy all_terminal (default), all_success or any_success; return a satisfied/blocked outcome or timeout. Empty list means children. |
| `xrefs_query` | analyst / verifier | no | Ghidra: references to/from a MODULE-RELATIVE offset. |
