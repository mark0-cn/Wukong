# Reverse analysis

Goal: reconstruct input handling and trust boundaries from binary to a
specific candidate problem, with module-relative addresses.

## Method

1. `functions_search` with a pattern matching the interesting logic
   (bounded pages; default page is 25 entries, paginate with `offset`).
2. `function_decompile` the candidates. Decompiler names and types are
   guesses — cross-check against `binary_strings` and disassembly-level
   evidence before asserting anything.
3. `xrefs_query` direction "to" on a dangerous call or buffer to enumerate
   reachable paths; direction "from" to see callees.
4. Trace the data flow: input origin -> checks (sizes, signs, conditions) ->
   dangerous operation. Record size/type conversions verbatim.

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
candidate input that would exercise the path. If you could not reach a
conclusion, say exactly what blocked you and what to try next.
