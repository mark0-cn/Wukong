# Agent tool interfaces

English · [简体中文](agent-tools.zh-CN.md)

The [generated catalog](tool-catalog.md) describes registered tools. The model sees the intersection of role, explicit task grants, selected groups and observed backend support. Tool selection cannot grant permissions. `contract.tool_grants` accepts `raw_debugger` and `windows_lab`; root may assign them to a task, and a child may delegate only grants it already has. Skills never grant permissions. Lab operations additionally require the VM to match the assigned environment.

`tool_catalog` reports groups and availability in pages. `tool_select` replaces optional groups for the next model request. Core tools and resource observation/cleanup remain available. Initial selection includes static analysis and typed debugging. Fuzzing, symbolic execution and configured MCP tools require selecting `fuzzing`, `symbolic` or `external`; raw debugger and Lab groups also require their task grant. Readiness is rechecked on every invocation.

## Interface migration

Old names and competing input forms are rejected; there are no aliases.

| Removed interface | Current interface |
|---|---|
| `fuzz_status` | `job_status` |
| `runtime_doctor` | `environment_doctor`; environment_id defaults to the task assignment |
| Native `message` / `wait` | `send_message_to_agent` / `wait_for_agents`; native `task` remains |
| `notes_list`, `todos_list`, `threat_model_get` | `context_search` then `context_read` |
| `think` | Save concise shared conclusions with `note_save` |
| Model `lab_prune` | Operator CLI only |
| `bundle_stage`, `runtime_prepare` | `launch_prepare` with source.kind=bundle or linux_runtime |
| `target_evidence_id` | `target_ref={kind:"evidence",evidence_id:"..."}` on every static tool |
| Bare static address | A target-bound module_relative AddressRef |
| `artifact_stage.artifact_id` | `evidence_id` |
| Completion `summary` | `result.answer` in the structured result |
| Lab vm/lab_id/generation input tuple | The opaque owned `lab_ref` returned by `lab_prepare` |
| `interactive_send.data_base64` | Tagged `source`: text, base64 or evidence |

Only verifier may call `analysis_verify`, `poc_verify` and `verification_record`. Certification and finding promotion also check the verifier role in the business layer.

## Execution and results

`launch_prepare` creates a hash-bound immutable layout from primary, bundle, staged files/dependencies or Linux target/libc/loader. Set args, cwd, environment, input mode and timeout there. Reuse the returned `launch_spec` in job_start, interactive_start or debug_start; omit argv/target/environment/timeout overrides. The schema rejects conflicting choices before worker dispatch. Preparation records identity, but actual worker execution remains the acceptance evidence.

`file_put` accepts exactly one text/base64/evidence source. `debug_start` separates launch and dump inputs, `debug_control` requires the fields appropriate to its action, and `crash_snapshot` requires exactly one session or core. `artifact_validate.interpreter` is python3 or bash. Interactive input is bounded to 64 KiB. `debug_inspect` kind=events exposes bounded event snapshots when the backend advertises debug.events; other backends report unavailable.

`findings_list` and `coverage_list` return filtered summaries, at most 20 per page. Reuse next_cursor as after. `evidence_search` finds same-run evidence by producer, target/content hash, job, source or media type. `resource_list` lists only the caller's owned jobs and sessions from controller records, including last-observed state. It does not adopt unknown worker resources or grant cleanup rights.

`resource_read` returns bounded output chunks with evidence IDs and next_offset. at_snapshot_end means no more bytes in the current observation; eof additionally requires terminal state. Backends may publish streams only after completion. A missing stream is an explicit error, not empty output. Evidence content remains available through evidence_read independently of worker resources.

`differential_test` compares two real prepared LaunchSpecs using 1–8 same-run input evidence files, each at most 64 KiB. Both manifests must record 1–10 second timeouts. Choose exact_output, exit_status or stdout_json and expected exit codes. Runs are sequential to support single-resource workers. It retains manifest, input, job-status and output evidence. Timeout, missing/truncated output, unexpected exit or invalid JSON is inconclusive. The aggregate call has a 300-second budget; unfinished jobs remain owned and discoverable.
