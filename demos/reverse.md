# Worked example: reverse

Goal: input/state → checks and transformations → output/state change, with module-relative addresses.

1. `functions_search` with a pattern that matches the interesting logic (e.g. "main", "check", "verify"), bounded pages.
2. `function_decompile` the candidates; compare what the DECOMPILER shows against `binary_strings` evidence (decompiler names are guesses).
3. `xrefs_query` direction "to" on a relevant function or data address finds incoming references. Direction "from" inspects one specific instruction address, not the whole function's callees.
4. Addresses: file offsets, module-relative offsets and runtime addresses are different spaces. Decompile/xrefs take module-relative values (image base from `binary_inspect`). Record which space every number belongs to.
5. Report: the exact reachable path (entry → checks → operation), the size/type conversions involved, and a candidate input to test the path. Finish with `agent_finish`.
