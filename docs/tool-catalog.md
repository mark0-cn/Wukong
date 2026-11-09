# 内置工具目录

由 `re-agent tool-catalog` 从工具注册表生成。不要手动编辑。

原生协调工具 `task`、`message`、`wait` 由 Rig 提供；MCP 工具取决于配置。下表列出模型可见的内置业务工具及生命周期可见性，具体执行仍校验目标、证据、会话归属和后端能力。

| 工具 | 可见角色 | 有副作用 | 用途 |
|---|---|---|---|
| `agent_finish` | analyst / verifier | 是 | Finish a child task with a structured result accounting for its contract, evidence and gaps. summary is shorthand for tasks without required outputs and is ignored when result is provided (result.answer is the canonical summary). Active descendants must finish first. |
| `angr_solve` | 全部 | 是 | Input reconstruction through a validation/transform chain (the classic symbolic-execution case): a binary reads stdin, transforms each byte and compares it against embedded constants. Do NOT hand-invert the transform — pass the success address (code reached only when validation passes) as find, the failure print/abort addresses as avoid, and the expected input length. Runs as a background job; job_status returns {"solved": true, "input_base64": ...} plus exploration stats. Reproduce the solved input against the real binary before believing it. Falls short on network/threaded/huge inputs — hand-write z3 constraints then. |
| `binary_inspect` | 全部 | 否 | Inspect the exact run target, architecture, sections, imports, symbols and mitigations. |
| `binary_strings` | 全部 | 否 | Search printable strings with file-offset address references and pagination. |
| `context_read` | 全部 | 否 | Read a bounded character range of an exact same-run shared document as JSON, including amendments. Use kind and id from context_search; follow content.next_offset until the required content is complete. |
| `context_search` | 全部 | 否 | Search same-run shared notes, todos and optional threat models by text, kind, author scope or agent_id. Returns bounded previews and next_offset; use context_read for exact content including amendments. |
| `coverage_list` | 全部 | 否 | List all recorded assessment coverage. |
| `coverage_record` | 全部 | 是 | Record inspected scope, outcome and supporting evidence. needs_follow_up blocks root completion. |
| `coverage_update` | 全部 | 是 | Update a coverage record's outcome, notes and evidence. |
| `crash_replay` | 全部 | 是 | Start a persistent clean-directory crash reproduction job. |
| `debug_close` | 全部 | 是 | Close your debugger session, or clean up an in-run session whose owner is terminal. |
| `debug_command` | 全部 | 是 | Execute exactly one GDB/MI command in your owned session; stable request IDs prevent blind replay. |
| `debug_start` | 全部 | 是 | Start an owned GDB/MI session on the staged target. |
| `evidence_read` | 全部 | 否 | Read a bounded byte range from an integrity-checked evidence artifact. Non-UTF8 is base64. |
| `exec_command` | 全部 | 是 | Run ONE bounded shell command synchronously in the worker and return its stdout/stderr/exit code in the result. The command runs in a fresh temporary directory: pass target=<staged path from stage_target> and the resolved file is available at $TARGET in the environment ("printf '<payload>' \| $TARGET"). Prefer this for quick inspection and stdin-driven PoCs; long-running work must go to job_start with output redirected to a log file instead of blocking here. Worker paths (files/...) resolve only as direct argv entries, never inside shell strings; use $RE_AGENT_STATE_DIR for the state root. |
| `file_get` | 全部 | 否 | Retrieve a declared in-run worker file and persist original bytes as evidence. |
| `file_put` | 全部 | 是 | Create an immutable worker file from base64 bytes or an existing evidence artifact. |
| `finding_file` | 全部 | 是 | File a suspected or reproduced finding with evidence. Verified impact requires a separate immutable verifier record. severity critical/high requires a reproduced-or-better stage. |
| `finding_get` | 全部 | 否 | Read a finding and its independent verification record. |
| `finding_update` | 全部 | 是 | Update an existing finding. Impact promotion is enforced using independent execution evidence. |
| `findings_list` | 全部 | 否 | List this assessment's findings and recorded stages. |
| `finish_scan` | root | 是 | Root only: finish the assessment after children and coverage are resolved. |
| `follow_up_agent` | 全部 | 是 | Explicitly assign a follow-up; reopens completed work in a new generation while preserving history. Use recipient_generation to guard against stale assignments. |
| `function_decompile` | 全部 | 否 | Ghidra: decompile a function by MODULE-RELATIVE offset. |
| `functions_search` | 全部 | 否 | Ghidra: search analyzed functions, paginated. |
| `fuzz_start` | 全部 | 是 | Start AFL++ QEMU fuzzing after staging target and nonempty seed files. |
| `fuzz_status` | 全部 | 否 | Observe a bound AFL++ job and its real statistics/crash files. |
| `input_minimize` | 全部 | 是 | Start crash-preserving delta reduction using observed crash signatures. |
| `interactive_close` | 全部 | 是 | Close your interactive session and terminate its process tree; also permits cleanup when its in-run owner is terminal. |
| `interactive_send` | 全部 | 是 | Write bytes to an owned interactive session's stdin and return its newly produced output. The session stays alive between calls. |
| `interactive_start` | 全部 | 是 | Start a persistent interactive process (REPL-style program) owned by this agent; returns a session_id. Worker paths resolve as direct argv entries. |
| `job_artifacts` | 全部 | 否 | List and import original artifacts produced by an in-run job. |
| `job_cancel` | 全部 | 是 | Request cancellation of an owned in-run job. |
| `job_start` | 全部 | 是 | Start a persistent process job in the configured worker; returns a job_id immediately. argv entries may use {target} as a placeholder for the staged target path (resolved by the harness and staged in isolated jobs); worker paths like files/... resolve only as direct argv entries, never inside shell strings. Jobs may read RE_AGENT_STATE_DIR and RE_AGENT_JOB_DIR from the environment. |
| `job_status` | 全部 | 否 | Observe an in-run job, refresh persisted status and import raw observations. |
| `load_skill` | 全部 | 是 | Activate a named capability or method (built-in or configured custom skill). Role instructions root/analyst/verifier are automatic and cannot be loaded here. Initial and active capabilities share a limit of 5 distinct names. Exact content/hash is persisted for the next fresh model boundary; replaces an initial copy of the same skill and survives compaction/resume. Loading never changes role, permissions or task completion criteria. |
| `message_status` | 全部 | 否 | Inspect a sent/received message, its delivery state and correlated reply. |
| `note_save` | 全部 | 是 | Save a shared note linked to this task. |
| `notes_list` | 全部 | 否 | List shared notes for this assessment. |
| `poc_verify` | 全部 | 是 | Independently replay a finding's input in fresh directories; worker observations determine trial success. input_mode selects delivery: "argv" (default, AFL-style @@ file path) or "stdin" (payload bytes piped to the target's stdin — use this for programs that read(0, ...)). |
| `runtime_doctor` | 全部 | 否 | Inspect the configured worker's real platform, versions and available capabilities. |
| `send_message_to_agent` | 全部 | 是 | Queue information, a question, reply, result or blocked notice. Information does not interrupt waits. Only follow_up_agent reopens completed work. Queued/included is not proof of processing. |
| `stage_target` | 全部 | 是 | Copy the run's SHA256-verified original target into the worker's immutable files area. |
| `stop_agent` | 全部 | 是 | Stop yourself or your descendants, with a recorded reason. |
| `think` | 全部 | 是 | Record a concise audited scratch note for this agent. The original content remains in tool-call history and may be inherited by children; this is neither a private channel nor a context-saving mechanism. Use note_save for shared conclusions. |
| `threat_model_amend` | 全部 | 是 | Append a durable amendment to a shared threat model. |
| `threat_model_get` | 全部 | 否 | Read the shared threat model documents. |
| `threat_model_save` | 全部 | 是 | Create a shared threat model with a stable document ID. |
| `todo_save` | 全部 | 是 | Create a shared to-do item; status defaults to pending. |
| `todo_update` | 全部 | 是 | Update the body or status of an existing shared to-do. |
| `todos_list` | 全部 | 否 | List shared to-do items. |
| `verification_record` | 全部 | 是 | Verifier only: ingest a completed poc_verify job, its raw evidence and real trial counts. A crash verifies only crash behavior. |
| `view_agent_graph` | 全部 | 否 | List this assessment's agent tree, scopes, states and results. |
| `wait_for_agents` | 全部 | 是 | Park on selected agents with policy all_terminal (default), all_success or any_success; return a satisfied/blocked outcome or timeout. Empty list means children. |
| `xrefs_query` | 全部 | 否 | Ghidra: references to/from a MODULE-RELATIVE offset. |
