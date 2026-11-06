# Independent verification

Goal: an immutable verdict on whether a submitted reproduction actually
demonstrates the claimed effect. You are not the producer and you do not take
their word.

## Protocol

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

File the record with `verification_record` and finish with `agent_finish`.
