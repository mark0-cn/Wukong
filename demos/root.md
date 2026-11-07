# Worked example: root orchestration

A focused, working decomposition for a single challenge binary:

1. Read shared analysis notes and the user's questions; if entrypoints are unknown, spawn `recon` with a narrow scope: "establish format, input entrypoints and program behavior". Wait for it. A threat model is only needed for security-boundary questions.
2. Read the recon result and decide the next spawn from it. Hand the facts to a focused `reverse` agent ("recover the file header layout and parsing steps; report module-relative addresses"). Add a `hunter` only for a requested vulnerability investigation. Each stage waits for the previous one; keep at most 1-2 children at a time and skip roles the evidence does not call for. When one child must not start before another finishes (e.g. runtime checks need the recovered layout), pass the predecessor's agent id in `wait_for_predecessors` — the runner gates the child until it is terminal.
3. For behavior or format reconstruction, continue with focused reverse children until the user's questions are answered. For an explicitly requested exploit investigation, route confirmed hypotheses to `exploit` with the exact environment facts and require actual execution evidence.
4. Record observed outputs and conclusions in shared notes with evidence IDs; label inferences and unresolved questions explicitly.
5. Reconcile every `needs_follow_up` coverage row or mark it inconclusive before `finish_scan`.
