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

## Choose an input reconstruction method

Select the cheapest method that can distinguish the current hypotheses:
- A short reversible transform: derive an inverse with explicit machine widths and test it.
- Clear constraints over several paths: try bounded symbolic execution with angr_solve.
- Large state space: slice the routine, fix known input bytes, or model local constraints.
- Stateful, network or unsupported execution: use a concrete harness and explicit constraints.

Before each attempt record the hypothesis, expected observation, time/step budget and
fallback in your checklist. Do not retry an unchanged method after a timeout. Preserve
partial facts and switch methods when the budget expires or assumptions are contradicted.
For angr, identify success/failure addresses and input length first. A solver result is
only a candidate: replay it on the real target. A successful inverse or reference model
also needs discriminating real-target checks; neither method is privileged by this skill.

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

## Multi-gate argv validation: solve one gate at a time

When the target validates an input argument through several sequential
checks, each comparing a transformed copy against a per-gate constant:

1. Identify the gates in the order the target evaluates them; the target's
   own feedback (which check fails) is the oracle — use it.
2. Solve ONE gate at a time and verify incrementally against the real
   binary before moving on: a candidate passing earlier gates is the
   precondition for probing the next one.
3. Placeholder characters inside a gate's transform template refer to bytes
   from earlier positions of the same input — resolve them by position, not
   as literals.
4. Prefer whole-chain symbolic solving (angr_solve with find=success) only
   when the gates interleave; otherwise the per-gate order is cheaper and
   each step has its own evidence.

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
Use function_instructions for original instruction and control-flow evidence; paginate with next_offset. Calls and branches are static edges, not proof of execution.
