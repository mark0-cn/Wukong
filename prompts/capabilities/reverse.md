# Reverse analysis

Use this capability to recover the semantics needed by the assigned question.
Reuse established layouts and locations; inspect missing or contradictory facts.

Goal: reconstruct the behavior, data structures, formats, algorithms or input
handling needed to answer the user's questions, with module-relative addresses.
Investigate trust boundaries and candidate vulnerabilities when the task calls for them.

## Method

Load the relevant method when its detailed workflow is needed:

| Question | Skill | Main deliverable |
|---|---|---|
| File/packet/record layout | `formats` | field table and decoded examples |
| Object/structure layout | `types` | offset/type constraints and candidate declaration |
| Transformation/checksum/numeric routine | `algorithms` | pseudocode and discriminating test vectors |
| Command/session lifecycle | `state_machine` | transition table and tested sequences |
| Changes between versions | `binary_diff` | matched scopes and before/after evidence |

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

Save the method-specific result in analysis notes with evidence IDs: the answer,
exact locations, supporting observations, conflicting evidence and uncertainty.
For behavior/path analysis include entry -> checks -> operation and an
input that would exercise the path. For input reconstruction, load
`tooling` and choose symbolic execution only when its stdin model matches the
target; otherwise use a suitable harness or explicit constraints.
If the assigned goal is a control-flow hijack, load `exploit`/`pwn` as needed and
reuse the layout facts (buffer size, saved-RIP offset, target address, mitigations).
Delegate only a distinct subproblem, not the next technique in the same analysis.
If you could not reach a conclusion, say exactly what blocked you and what
to try next.

Stop when the assigned question is answered with bounded evidence or a
specific missing capability/input blocks it. Do not require execution for a
purely static question or turn every result into a vulnerability finding.
