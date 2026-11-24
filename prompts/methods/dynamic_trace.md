# Dynamic function tracing and offline trace comparison

Use when a question requires observations inside a function rather than process-level output alone. The current backend is Linux x86_64 ELF with GDB/MI and the SysV AMD64 ABI. `environment_doctor` must confirm `trace.start` before execution; a controller-only static analysis remains the fallback when it is unavailable.

## Declare one logical experiment

1. Use static analysis to choose only the functions or call sites that can discriminate the current hypothesis. Give each a stable `point_id`; when comparing versions, keep the id and capture semantics identical even if symbol addresses or module offsets differ.
2. Prepare immutable LaunchSpecs with the same arguments, environment, working directory, input mode, loader and non-target files. Supply identical input evidence. `trace_run` does not accept launch overrides, PTY or network input.
3. Declare entry/return or call-site points using module-qualified symbols or module-relative addresses. A function return requires its entry, so the worker can save the invocation context. Do not use arbitrary debugger expressions, shared-library points, stack arguments or unsupported value types.
4. Capture scalar arguments, return values and bounded buffers by ABI argument index. Buffer addresses and lengths are saved at entry for return capture. Every point and capture name must be unique within its scope. Set event, hit, memory and stop-wait budgets deliberately; the overall timeout belongs to the LaunchSpec.

`trace_run` starts an ordinary background job. Continue other work while it runs, then use `job_status` and `job_artifacts` to obtain `trace/run.json` and `trace/events.jsonl`. The worker owns the private debugger; do not search for or issue commands to that session.

## Compare normalized observations

Call `trace_compare` with the run/events evidence pair from each of two owned trace jobs. The controller authenticates the jobs and input identity offline without reconnecting the worker. A logical plan signature excludes locations and limits, not ABI, point kinds, phases or capture semantics.

- `match`: all selected normalized observations agree within the compared scope.
- `mismatch`: complete comparable observations differ, including a missing event on one side.
- `incomparable`: input, ABI, plan, worker/backend or non-target execution conditions differ.
- `inconclusive`: evidence is missing or incomplete, the job is unsettled, a limit or capture error occurred, a return is open, or either side has zero events.

Completion of the JSONL, target exit, debugger cleanup and controller evidence readiness are separate facts. A partial or truncated trace never establishes equality. Do not compare raw ASLR addresses, PIDs, timestamps or breakpoint IDs as semantic values. Missing events on complete traces can be meaningful differences; missing evidence is not.

## Interpret and hand off

Inspect the first divergence in the original events and retain the comparison evidence ID. Report the exact compared points, captures, inputs and limitations. Match does not prove whole-program equivalence, all-path coverage, absence of vulnerabilities or impact. A crash may be a complete observation when all requested observations closed, but it is never automatically code execution.

For security claims, a verifier independently repeats the required traces and comparisons and uses the ordinary finding/verification path. For general analysis, preserve the plan/run/events/comparison evidence and use `analysis_verify` for the final claims.
