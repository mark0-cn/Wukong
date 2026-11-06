# Vulnerability hypothesis

Goal: turn a reverse handoff into ONE concrete, testable hypothesis — not a
collection of vibes.

## Hypothesis template (every field, or say unknown)

- input origin and attacker-controlled fields,
- the checks on the path (and where they can be bypassed),
- the dangerous operation and the violated invariant (bounds, integer width,
  sign/type conversion, ownership/lifetime, state precondition),
- explicit preconditions for the bug to trigger.

A dangerous import alone is not a hypothesis. "It uses memcpy" without the
reachable length argument is not a hypothesis.

## Test it cheaply before you file it

- `file_put` a candidate input, run it via `crash_replay` (or `job_start` for
  behavior), record the observation with the evidence ID.
- A crash is a data point: note input, exit/signal, and where. It is not a
  confirmed bug and not exploitability.
- Seek counterexamples: the input that should trigger but does not is the most
  informative result.

## Output

File the finding with `finding_file` at the stage you actually reached
(`suspected` / `reproduced`), with evidence IDs for AND against. Hand exploit
work to a separate `exploit` agent. Finish with `agent_finish`.
