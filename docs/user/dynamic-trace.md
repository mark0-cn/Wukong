<a id="动态函数追踪与离线差分"></a>

# Dynamic function tracing and offline trace comparison

English · [简体中文](dynamic-trace.zh-CN.md)

`trace_run` and `trace_compare` let an analyst declare function-level observations once, have a worker execute them under an owned debugger, and compare the resulting normalized evidence offline. They are method-independent: the engine does not know encryption, parsing, patching, protocols or vulnerability classes.

The current backend is **Linux x86_64 ELF with GDB/MI and the SysV AMD64 ABI**. Windows and macOS do not advertise this capability. Run `environment_doctor` before planning execution; `trace.start` must be ready.

## Workflow

1. Use static analysis to select a small number of discriminating functions or call sites and assign stable `point_id` values.
2. Use `launch_prepare` for each target. Keep arguments, environment, cwd, input mode, timeout, loader and non-target files identical, and supply the same input evidence.
3. Declare points and captures in a `TraceSpec`, then call `trace_run` with `launch_spec`, `input` and `spec`.
4. Continue other work while the ordinary job runs. Use `job_status` and `job_artifacts` to obtain `trace/run.json` and `trace/events.jsonl`.
5. Call `trace_compare` with both run/events evidence pairs. The controller authenticates the producing jobs offline; it does not reconnect to the worker.

The worker owns the private debugger. It is not a public debug session and cannot be controlled through `debug_control` or `debug_command`.

## TraceSpec example

This example captures `parse_record(bytes, size, record)` without knowing the record algorithm:

```json
{
  "schema_version": 1,
  "abi": "sysv_amd64",
  "points": [
    {
      "kind": "function",
      "point_id": "parse",
      "location": {
        "kind": "symbol",
        "module_sha256": "<target-sha256>",
        "symbol": "parse_record"
      },
      "phases": ["entry", "return"],
      "capture": {
        "arguments": [
          {"name": "size", "index": 1, "value_type": "u64"}
        ],
        "buffers": [
          {
            "name": "input",
            "address_argument": 0,
            "length": {"kind": "argument", "index": 1}
          },
          {
            "name": "record",
            "address_argument": 2,
            "length": {"kind": "constant", "bytes": 16}
          }
        ],
        "return_value": "i32"
      }
    }
  ],
  "limits": {
    "max_events": 1024,
    "max_point_hits": 256,
    "max_buffer_bytes": 4096,
    "max_event_bytes": 65536,
    "max_events_bytes": 8388608,
    "stop_wait_ms": 10000
  }
}
```

Use the same logical point when comparing versions, even if addresses change. Locations may instead be `{ "kind": "module_offset", "module_sha256": "...", "offset": 4660 }`. Offsets are module-relative, not file offsets or runtime virtual addresses.

Capture rules:

- `arguments[*].index` and buffer address/length arguments use zero-based SysV integer/pointer argument positions 0–5.
- Integer types are `u8/u16/u32/u64/i8/i16/i32/i64/pointer`.
- Buffer length is `{ "kind": "constant", "bytes": N }` or `{ "kind": "argument", "index": N }`.
- Buffers are read at entry and return. Return capture uses the address and length saved at entry, not the overwritten argument registers.
- A function with `return` requires an entry activation. `call_site` emits `call_site` events and does not create one.
- `limits` controls events, hits, capture bytes, JSONL size and each stop wait. Overall timeout belongs to the LaunchSpec.

## Offline comparison

```json
{
  "left": {
    "run_evidence_id": "ev_run_left",
    "events_evidence_id": "ev_events_left"
  },
  "right": {
    "run_evidence_id": "ev_run_right",
    "events_evidence_id": "ev_events_right"
  },
  "comparison": {
    "capture_ids": ["parse:input", "parse:record", "parse:return_value"],
    "ignore": [],
    "max_differences": 16
  }
}
```

Both jobs must belong to the caller and the same run. The left and right jobs must differ. The controller finds them uniquely through `JobRecord.imported_artifacts`, verifies evidence hashes and parameters, and does not call `job_status`, `job_artifacts` or the worker. Previously synchronized evidence therefore remains comparable while the worker is offline.

Results have four meanings:

| Verdict | Meaning |
| --- | --- |
| `match` | Selected normalized observations agree within the declared scope. |
| `mismatch` | Complete comparable observations differ, including an event missing on one side. |
| `incomparable` | Input, ABI, logical plan, worker/backend or non-target execution conditions are incompatible. |
| `inconclusive` | Evidence is missing/incomplete, a job is unsettled, cleanup or evidence is not ready, a limit/capture error occurred, a return is open, or either side has zero events. |

A logical plan signature excludes locations and limits. It includes ABI, point kinds, phases, argument positions/types and buffer sources. A changed address does not prevent comparison; changed capture semantics are incomparable. Reaching any limit makes the result inconclusive.

Raw ASLR addresses, PIDs, timestamps, breakpoint IDs and job IDs do not participate in semantic equality. Non-module pointer values remain in original evidence but are not compared as integers. Integer width/sign and buffer bytes are compared. Buffer differences are bounded and identify the first differing offset rather than embedding the full buffers.

## Completion and recovery

These facts are independent:

1. The JSONL stream was closed and hashed.
2. The requested observations completed without open activations, limits or capture errors.
3. The job reached a terminal state and cleanup was confirmed.
4. The controller synchronized and integrity-checked the evidence.

A partial JSONL prefix is retained for diagnosis. It cannot produce `match`. Start replay uses the stable worker request identity and does not execute the trace again. Cancellation and timeout close the private debugger and preserve the partial run/events artifacts.

`trace/run.json` keeps source and executed target hashes separate, including patched runtime images. It also records address bindings, LaunchSpec, input and an environment signature covering worker/GDB, launch arguments, environment, loader and non-target files. Target member names and target hashes are not treated as environment differences; source and executed target identity is verified separately.

## Current limits

- Linux x86_64 ELF and SysV AMD64 only; no Windows/macOS backend.
- First six integer/pointer arguments only; no stack arguments, floating/vector values, aggregate returns or arbitrary expressions.
- Main target locations only; no shared-library points, watchpoints, GDB Python or automatic call-graph discovery.
- Recursive activations are supported. Multiple actual threads produce partial evidence. Tail calls, longjmp, exception unwinding and unclosed activations can also make a trace partial.
- Stripped PIE requires a unique mapped-file hash and PT_LOAD resolution; unresolved addresses are rejected before execution continues.
- `match` covers only declared captures and inputs. It does not prove whole-program equivalence, all-path coverage, security or exploit impact.

Raw captured memory and transcripts can contain sensitive target data. Original evidence is hashed and retained on the controller, while model-facing results are bounded. A crash is an observation, not proof of code execution. Security impact still requires the ordinary independent finding verification path.

<a id="验证"></a>

## Validation

Protocol and fake-MI tests cover schemas, canonical plan signatures, activation pairing and JSONL bounds. Real Linux Docker/GDB acceptance additionally covers nested and recursive calls, PIE/non-PIE/stripped addresses, call sites, buffer mutation, event limits, unreadable memory, timeout, cancellation, cleanup and replay. Real controller-to-worker acceptance covers LaunchSpec preparation, evidence import, same-target `match` and A/B `mismatch` across different target member names. Fixture success establishes these mechanisms, not Agent solve rates or whole-program equivalence.
