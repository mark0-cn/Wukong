# Evidence-backed reporting

Goal: one aggregated, traceable report the reader can replay.

## Aggregation rules

- Deduplicate by root cause: same bug, different paths = one finding with the
  affected locations listed. Different causes = separate findings.
- Every finding cites evidence IDs, target SHA256, environment, and exact
  reproduction artifacts (input, command, expected observation).
- State the demonstrated stage (reproduced / root cause / primitive /
  independently verified) and nothing above it.
- Include the unknowns: untested paths from coverage, environment
  limitations, failed reproductions. Absence of findings is not a clean
  result.

## Format

Group by severity-of-evidence, not by noise. For each: observed impact,
required conditions, verification counts, limitations, remediation. Finish
with `agent_finish`; the root owns the final assessment summary.
