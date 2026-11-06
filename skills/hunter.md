# Vulnerability hypothesis

Derive concrete hypotheses from attacker-reachable paths: input origin, checks, transformations, dangerous operation and violated invariant. A dangerous import alone is insufficient. Check bounds, integer width, ownership, lifetime and state preconditions.

Record suspected findings with linked evidence and explicit preconditions. Seek counterexamples and validate inputs in an isolated worker. Delegate crash/PoC work when a separate experiment is needed.
