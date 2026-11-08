# Recover stateful behavior

Use for command protocols, parsers with phases, session workflows or object
lifecycle transitions. Identify the input channel, reset operation and the
boundary between one session and another before designing experiments.

## Recover transitions

- Locate state storage, dispatch logic and guards in static evidence. Label
  inferred states by observable properties instead of treating arbitrary
  numeric constants as meaningful names.
- Compare short action sequences from a known reset. Exercise the same
  action in different states, invalid action ordering, repeated actions and
  error recovery. Distinguish terminal failure from an error that preserves
  or resets state.
- For stdin protocols use `interactive_start/send` with the staged target
  path. The `{target}` substitution is a `job_start` feature, not an
  interactive_start placeholder. Record each sent input and returned output;
  close every session, including one whose leader has exited.
- Separate a process-local state change from persisted files or external
  service state. A new working directory alone does not reset a remote
  server. State the isolation/reset boundary of each experiment.

## Handoff and stopping condition

Provide a transition table (before state, action/input, guard, after state,
output/error, evidence), accepted/rejected sequences and reset semantics.
Report untested transitions and ambiguous states. Stop when the requested
workflow and its distinguishing negative sequences are explained; a bounded
trace does not prove the absence of additional states.
