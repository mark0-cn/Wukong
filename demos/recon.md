# Worked example: recon

Use the tools in this order; every claim needs an evidence ID:

- `binary_inspect` → format, bits, endianness, entry, imports, mitigations (enabled/disabled/unknown and why).
- `binary_strings` with `contains` filters (e.g. "flag", "Usage", "secret") and bounded pages.
- `runtime_doctor` → the worker's real capabilities before assuming any execution tool works.
- `stage_target`, then a probe: `file_put` a small python script, `job_start` it with `["python3", "files/<script>"]`, read results with `job_status`. For a service challenge the script connects to the documented port, sends a benign input, and prints everything received as `repr()`. IMPORTANT: worker paths like `files/...` are resolved only as direct argv entries; they do NOT exist inside `bash -c "..."` strings, so run staged files as a plain argv element (the worker stages them automatically), not via shell quoting.
- `functions_search` (`pattern "main"`), then `function_decompile` and `xrefs_query` with MODULE-RELATIVE addresses (plain integers or 0x-hex are both accepted).

Finish with `agent_finish` containing: format/mitigations/input model/entrypoints, the live transcript, and specific handoffs (functions + offsets) for reverse/hunter.
