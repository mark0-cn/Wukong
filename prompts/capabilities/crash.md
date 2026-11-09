# Crash triage

Use for crash reproduction, reduction and root-cause investigation within the
assigned question. Reuse campaign inputs, environment and prior reverse evidence.

Goal: from a crash artifact to a classified, minimized, root-caused-or-not
record. A crash is not a bug until you can say what failed and why.

## Reproduce before you believe

- `crash_replay` the recorded input against the recorded target and
  environment. Only an actual reproduction counts.
- Under the debugger (`debug_start` / `debug_command`): exception/signal,
  faulting instruction, registers, stack, module mapping. Record the module
  map, not raw ASLR addresses.

## Minimize

`input_minimize` with a predicate that preserves the ORIGINAL failure
signature. Reject reductions that changed the signal/exception or turned the
crash into a timeout.

## Classify

Determine which of these it is, with evidence:

- real invalid operation (bounds, use-after-free, bad dereference...),
- intended assertion/abort (the program checked and died on purpose),
- resource exhaustion (OOM/timeout artifacts),
- unknown — say so; silence is not a verdict.

Group crashes by normalized root cause; two crashes with different ASLR
addresses may be one bug.

## Output

Save per-crash: input hash, minimized input, signature,
classification, root-cause hypothesis and confidence, evidence IDs. Do not
infer exploitability from the signal alone.
Continue the primary assignment if it also requires explanation or experiments;
do not create a new agent solely to switch analysis techniques.

Use `crash_snapshot` with an owned stopped debugger session (or an in-run core)
for registers, PC/SP, fault address, frames and raw evidence. For a cyclic pattern,
record the original file, alphabet and n; give `cyclic_offset` exactly n observed
bytes in memory order. Do not silently truncate a register or guess endianness.

`input_minimize` defaults to signal + mapped-module hash + PC file offset +
resolved frames. Each candidate is replayed natively and observed in a fresh GDB
session. Unresolved identity stops baseline minimization; `allow_signal_only=true`
is an explicit weaker fallback, not proof of the same root cause. Collect trial
snapshots from `job_artifacts`. A stdin debugger replay uses file redirection,
whereas the native replay uses a pipe: check channel-sensitive differences.
