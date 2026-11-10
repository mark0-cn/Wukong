# Agent / Skill paired evaluation

Run the existing analysis fixture tasks under two conditions, changing exactly one variable:

- Skill ablation: selected method instructions versus an empty custom override of that method. Preserve role instructions and permissions.
- Coordination: one analyst versus several scoped analysts, same model and total budget.
- Verification: submit deliberately incorrect claims (length includes header, wrong endian, wrong overflow width, reset preserves state) to a separate verifier. Keep expected verdicts in reviewer data, outside the agent prompt.
- Capability loss: repeat with Ghidra or execution unavailable; independently review whether the answer limits its scope and identifies the missing capability.

Use at least three repeats per case. Export real report.json files; independently adjudicate every question, including unresolved ones. The comparison does not call a model or fabricate verdicts. Record model, budget and target hash from the run configuration, not from memory. Treat a declared experimental condition as a hypothesis: inspect the actual agent/skill trace to ensure it was applied.

`python -m evals.analysis.compare_runs manifest.json --output comparison.json`

The manifest is a JSON array. Each row has `case`, `repeat`, `condition` (baseline/candidate), `model`, `budget`, `target_sha256`, `questions` (IDs), `report` (relative path), and `reviews` (relative path). Review files are arrays containing question_id, verdict (correct/incorrect/unresolved), rationale and evidence_ids. False-claim reviews additionally contain injected_false_claim=true and accepted_by_agent=true/false. Missing reviews, unpaired runs and mismatched model/budget/hash/question sets are rejected.

Metrics include evidence-backed accuracy, incorrect/unresolved rate, tokens, configured cost, duration when present, failed calls, repeated read-call proxy, agent count, immutable general verification records and false-claim acceptance. Candidate-minus-baseline deltas are paired before averaging. These metrics do not automatically prove that skills improve quality.
