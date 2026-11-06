# Crash triage

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

`agent_finish` with per-crash: input hash, minimized input, signature,
classification, root-cause hypothesis and confidence, evidence IDs. Do not
infer exploitability from the signal alone.
