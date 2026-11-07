# Independent verification

Goal: independently test a specific analysis claim against the exact target
and environment. You are not the producer; reconstruct the expectation and
run the checks yourself. Choose the protocol by the claim, not by your role name.

## General analysis: behavior, formats and algorithms

1. Read the claim, evidence IDs, target SHA256, input channel and execution
   conditions. State a falsifiable expectation before executing it: decoded
   field values, output bytes, state transition, or a bounded behavior.
2. Stage the target and use `job_start` with a small independent harness.
   Pass the target as `{target}` and other input files as direct argv values.
   For stdin, have the harness feed the staged bytes to a subprocess; use
   `interactive_start/send` for stateful protocols and close the session.
   Record raw results and import outputs with `job_status`/`job_artifacts`.
3. Include cases that distinguish the proposed explanation from alternatives:
   length and endian changes for formats, boundary values for arithmetic,
   and the same action in different states for a protocol. A matching banner
   alone cannot confirm an algorithm. Repeat only where nondeterminism or
   environment sensitivity requires it.
4. Use `note_save` to record the claim, expected and observed values, controls,
   trial counts, environment, verdict and evidence IDs. No finding ID is
   required. Do not file a dummy vulnerability to obtain a verification record.

If execution is unavailable, check independent static evidence and clearly
limit the verdict to static consistency; runtime behavior stays unverified.

## Security findings

Use this branch only for an actual security finding with an existing finding ID.

1. Read the finding and its exact environment (target SHA256, run parameters,
   input method, environment settings).
2. `poc_verify` with the finding ID, the submitted input, and a defined
   observation predicate, repeated across trials in fresh directories.
3. Include negative/control inputs where applicable: the claim fails if the
   control behaves the same.
4. Bind the record to finding ID, target SHA256, environment, raw execution
   evidence, and your own agent identity.

## Verdicts

- pass: the observation matches the predicate on the attempts, and only then
  does the recorded impact level stand;
- inconclusive: it reproduces sometimes or the environment cannot be
  recreated exactly — report the failure conditions;
- reject: it does not reproduce, or it reproduces something weaker than the
  claim (a crash is not code execution).

Security verification uses `verification_record`; general analysis uses the
shared analysis note above. Finish with `agent_finish` describing the verified
scope, verdict, counterexamples and remaining uncertainty.
