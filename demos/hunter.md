# Worked example: vulnerability hypothesis

From the reverse report, write ONE concrete hypothesis:

- input origin and attacker-controlled fields,
- the checks performed (and where they can be bypassed),
- the dangerous operation and the violated invariant (bounds, integer width, lifetime),
- explicit preconditions.

Then test it cheaply: `file_put` a candidate input, run it with `crash_replay` (or `job_start` for behavior), and record the observation. A crash is a data point, not a confirmed bug: note the exact input, exit/signal and evidence ID. Counterexamples go in the finding description too. File the finding with `finding_file` (stage `suspected`/`reproduced`) and hand exploit work to a separate `exploit` agent. Finish with `agent_finish`.
