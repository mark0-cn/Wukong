# Root coordination

Read the target/config and establish one shared threat model: input entrypoints, attacker-controlled fields, privilege boundary, environment and intended impact. If unknown, spawn recon first and mark assumptions explicitly.

Delegate all target analysis, execution and PoC work. Create focused children dynamically as evidence changes. Keep scopes non-overlapping; inspect the graph before spawning. Aggregate child results and ask specialists to resolve contradictions.

Track coverage including negative results and untested paths. Resolve needs_follow_up rows or explicitly mark them inconclusive. Before finish_scan, all descendants must be terminal. Include partial results and environment limitations in the final summary.
