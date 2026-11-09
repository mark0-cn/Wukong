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

## Input reconstruction: hand the transform chain to angr_solve

When the assigned question is "what input passes this chain of checks or
transformations" (a validation routine comparing the input against embedded
constants is the textbook case):

1. From the decompiled path, pick the SUCCESS address — code reached only when
   the validation passes (right after the final check) — and the FAILURE
   addresses (error prints, aborts). Module-relative values.
2. Call `angr_solve` with find=<success>, avoid=<failure addresses> and
   stdin_length close to the expected input size. It runs symbolic execution
   as a background job: poll job_status for
   {"solved": true, "input_base64": ...} (also persisted as job artifacts).
   The longer the transform chain, the more this beats hand inversion —
   hand-written inverse-transform scripts are the fallback, not the plan.
3. Reproduce the solved input against the real binary (exec_command/job_start
   feeding the bytes) before accepting it: angr's answer is a hypothesis
   until the real run agrees.
4. Fall back to loading `algorithms` and inverting the transform yourself only
   when angr dead-ends (network input, threads, unsupported syscalls, or an
   input size too large for symbolic stdin).

## VM / hand-crafted / self-decoding code: use emulate_run

When the target embeds an interpreter, JIT or self-decoding payload (RWX
mmap + signal handler, tiny hand-crafted ELFs, x87/FPU blocks), prefer
`emulate_run` over hand-building a Unicorn harness: it maps the ELF segments
plus a stack, runs from `start` for `count` instructions or until the PC
reaches `until`, and returns registers and `dump` ranges. Set `init_regs`
for preconditions and `memory` to inject bytes. It is a pure simulation —
the target never runs natively. Hand-written harnesses have historically
burned turns on hook-page and endianness bugs; this tool is those bugs
pre-fixed.

## Control-flow flattening — stop static deobfuscation early

When a check function is control-flow-flattened (a large dispatcher/state
machine with junk blocks): identify the SEMANTIC CORE — the comparison
constant and the success value — and then STOP static deobfuscation.
Reconstructing a >50KB flattened function statically is a budget trap.
Switch to dynamics: debug_start + breakpoints on the final compare/return,
run candidate inputs, observe the compared values; or angr_solve when the
flattening is small enough. The comparison value is the deliverable, not
the deobfuscated source.

## Output

Save the method-specific result in analysis notes with evidence IDs: the answer,
exact locations, supporting observations, conflicting evidence and uncertainty.
For behavior/path analysis include entry -> checks -> operation and an
input that would exercise the path. For input reconstruction follow the
angr_solve checklist above before hand-writing anything.
If the assigned goal is a control-flow hijack, load `exploit`/`pwn` as needed and
reuse the layout facts (buffer size, saved-RIP offset, target address, mitigations).
Delegate only a distinct subproblem, not the next technique in the same analysis.
If you could not reach a conclusion, say exactly what blocked you and what
to try next.

Stop when the assigned question is answered with bounded evidence or a
specific missing capability/input blocks it. Do not require execution for a
purely static question or turn every result into a vulnerability finding.
