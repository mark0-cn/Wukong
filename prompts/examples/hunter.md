# Worked example: vulnerability hypothesis

Assigned analyst capabilities: `hunter`, with `reverse` as needed. Start from
existing notes and investigate an input path in the requested scope. Trace its
checks and sensitive operation to discover a candidate invariant violation; a
pre-existing hypothesis is not required. Write one concrete hypothesis at a time:

- input origin and attacker-controlled fields,
- the checks performed (and where they can be bypassed),
- the dangerous operation and the violated invariant (bounds, integer width, lifetime),
- explicit preconditions.

Then test it cheaply when execution is permitted: `file_put` a candidate input,
run it with `crash_replay` or `job_start`, and record the observation. A crash is
a data point, not a confirmed bug: note exact input, exit/signal and evidence ID.
Counterexamples go in the finding description too. File only a supported finding
at `suspected`/`reproduced`. Continue crash investigation or exploit experiments
in the same analyst when the assignment includes them; otherwise report the
next action to root. A separate verifier independently checks submitted claims.
