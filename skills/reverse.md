# Reverse analysis

Goal: reconstruct the behavior, data structures, formats, algorithms or input
handling needed to answer the user's questions, with module-relative addresses.
Investigate trust boundaries and candidate vulnerabilities when the task calls for them.

## Method

1. `functions_search` with a pattern matching the interesting logic
   (bounded pages; default page is 25 entries, paginate with `offset`).
2. `function_decompile` the candidates. Decompiler names and types are
   guesses — cross-check against `binary_strings` and disassembly-level
   evidence before asserting anything.
3. `xrefs_query` direction "to" finds references to the specified address;
   direction "from" finds references originating at that one address. Query
   an actual call-site offset to inspect its outgoing reference. A query at
   the function entry does not enumerate all calls in the function body.
   Cross-references establish static relationships, not executed reachability.
4. Trace the relevant control/data flow: input or state -> checks and
   transformations -> output or state change. Record layouts, size/type
   conversions and algorithm steps relevant to the question.

## Address discipline

- File offsets, module-relative offsets and runtime addresses are different
  spaces. `function_decompile`/`xrefs_query` take module-relative values
  (image base from `binary_inspect`). Plain integers and `0x`-hex strings are
  both accepted.
- Always say which space a number lives in; never hand a runtime address to a
  static query.

## Output

`agent_finish` with: the reachable path (entry -> checks -> operation), the
exact offsets involved, the conversions that matter, counterevidence, and one
candidate input that would exercise the path. For input reconstruction, load
`tooling` and choose symbolic execution only when its stdin model matches the
target; otherwise use a suitable harness or explicit constraints.
If the goal is a control-flow hijack, hand the layout facts (buffer size,
saved-RIP offset, target address, mitigations) to a child that loads `pwn`.
If you could not reach a conclusion, say exactly what blocked you and what
to try next.
