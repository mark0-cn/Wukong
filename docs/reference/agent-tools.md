# Agent tool interfaces

English · [简体中文](agent-tools.zh-CN.md)

The [generated catalog](tool-catalog.md) describes registered tools. The model sees the intersection of role, selected groups and observed backend support. Tool selection and Skills never change roles. Raw debugger and Windows Lab operations are enabled by default for roles permitted by the catalog. Lab operations additionally require the VM to match the assigned environment.

`tool_catalog` reports groups and availability in pages. `tool_select` replaces optional groups for the next model request. Core tools and resource observation/cleanup remain available. Initial selection includes static analysis and typed debugging. Fuzzing, symbolic execution, raw debugger, Lab and configured MCP tools require selecting `fuzzing`, `symbolic`, `raw_debugger`, `windows_lab` or `external` respectively; this only changes model context. Readiness is rechecked on every invocation.

Only verifier may call `analysis_verify`, `poc_verify` and `verification_record`. Certification and finding promotion also check the verifier role in the business layer.

For security findings, a valid passing `verification_record` and its `finding.verification_id` link commit in one transaction. The first valid passing link wins: replay can repair an unlinked old record, but a different valid passing link is a conflict and is never silently replaced. Failed verdicts do not set the success pointer. Linking does not promote the finding stage; `finding_get` immediately resolves the linked record without a root `finding_update`.

## Execution and results

`launch_prepare` creates a hash-bound immutable layout from primary, bundle, staged files/dependencies or Linux target/libc/loader. Set args, cwd, environment, input mode and timeout there. For bundles, optional `source.entry` and `source.members` select the executable and member subset before upload. Reuse the returned `launch_spec` in job_start, interactive_start or debug_start; omit argv/target/environment/timeout overrides. The schema rejects conflicting choices before worker dispatch. Preparation records identity, but actual worker execution remains the acceptance evidence.

`file_put` accepts exactly one text/base64/evidence source. `debug_start` separates launch and dump inputs, `debug_control` requires the fields appropriate to its action, and `crash_snapshot` requires exactly one session or core. `artifact_validate.interpreter` is python3 or bash. Interactive input is bounded to 64 KiB. `debug_inspect` kind=events exposes bounded event snapshots when the backend advertises debug.events; other backends report unavailable.

`findings_list` and `coverage_list` return filtered summaries, at most 20 per page. Reuse next_cursor as after. `evidence_search` finds same-run evidence by producer, target/content hash, job, source or media type. `resource_list` lists only the caller's owned jobs and sessions from controller records, including last-observed state. It does not adopt unknown worker resources or grant cleanup rights.

Ordinary `coverage_record` calls upsert the caller's whitespace-normalized, case-sensitive scope/kind row. Root may additionally pass `supersedes` with up to 128 distinct same-run coverage IDs whose normalized scope/kind matches and whose owners are terminal. The root resolution and old rows' transition to `superseded` commit atomically. Superseded rows retain their IDs, notes and evidence for audit but do not block completion and are not current conclusions; correct contradicted inferences in the root resolution. Non-root callers, active owners, cross-run references and scope/kind mismatches are rejected.

`resource_read` returns bounded output chunks with evidence IDs and next_offset. at_snapshot_end means no more bytes in the current observation; eof additionally requires terminal state. Backends may publish streams only after completion. A missing stream is an explicit error, not empty output. Evidence content remains available through evidence_read independently of worker resources.

`differential_test` compares two real prepared LaunchSpecs using 1–8 same-run input evidence files, each at most 64 KiB. Both manifests must record 1–10 second timeouts. Choose exact_output, exit_status or stdout_json and expected exit codes. Runs are sequential to support single-resource workers. It retains manifest, input, job-status and output evidence. Timeout, missing/truncated output, unexpected exit or invalid JSON is inconclusive. The aggregate call has a 300-second budget; unfinished jobs remain owned and discoverable.

`trace_run` starts a declarative function entry/return or call-site trace as an ordinary durable job, using an immutable LaunchSpec and one same-run input. `trace_compare` accepts two caller-owned `trace/run.json` + `trace/events.jsonl` evidence pairs and compares normalized observations offline, without contacting the worker. Only complete, compatible, settled and evidence-ready nonzero traces can return match/mismatch. See [Dynamic function tracing](../user/dynamic-trace.md) for the schema and Linux/ABI limits.
