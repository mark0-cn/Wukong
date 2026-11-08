# Vulnerability research

Use for requested vulnerability discovery or investigation of a security boundary.
Apply the methods of a senior vulnerability research engineer: discover candidate
invariant violations, formulate falsifiable hypotheses and test them against the
exact target. A ready-made reverse handoff or hypothesis is not required.

## Discover within the assigned scope

- Start from existing target facts, entry points and notes. Use `reverse` when
  data/control flow is unclear rather than making another analyst rediscover it.
- Trace externally influenced values and state through checks, transformations
  and sensitive operations. Look for violated bounds, integer-width/sign rules,
  type assumptions, ownership/lifetimes and state preconditions.
- Select the most promising supported candidate and investigate it at a time.
  A dangerous import or a crash alone does not establish a vulnerability.

## Make each hypothesis testable

Record the input origin and controlled fields, path checks and possible bypass,
dangerous operation, violated invariant, and explicit trigger preconditions.
Label missing facts unknown. Seek alternative explanations and counterexamples.

When execution is allowed and supported, use `file_put` and `crash_replay` or
`job_start` for a small discriminating input. Record target, environment, input,
expected and observed behavior, and evidence for and against the hypothesis.
For crashes, use `crash` to reproduce, minimize and investigate the root cause as
needed within the same assignment. Static-only work stays static and bounded.

## Output and scope

Use `finding_file` only for an evidence-backed candidate, at the actual stage
(`suspected` or `reproduced`). Save rejected hypotheses and the bounded search
coverage in notes rather than inventing a finding when none is supported.
If the contract includes exploit experiments, continue with `exploit` after the
root cause is established; otherwise report the next action to root. Independent
verification belongs to a separate verifier. Return these results to the primary
role's workflow; this capability does not add work beyond the contract.
