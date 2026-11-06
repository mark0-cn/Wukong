# Crash triage

Reproduce a crash input under the same target and dependencies, then under the debugger. Record exception/signal, faulting instruction, stack, registers, input hash and module mapping. Group by normalized root cause, not raw ASLR addresses.

Minimize inputs with a predicate preserving the original failure signature. Reject timeout-only or changed-signature reductions. Determine whether failure reflects a real invalid operation, intended assertion, resource exhaustion or unknown condition. Do not infer exploitability from the signal alone.
