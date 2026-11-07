# Evidence-backed reporting

Goal: one aggregated, traceable report the reader can replay.

Organize a general reverse-engineering report around the user's questions:
purpose and behavior, recovered formats/algorithms, supporting evidence,
reproduction steps, uncertainties and unfinished work. Do not invent security
findings or assign severity to ordinary implementation observations.

## Security findings, when requested

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

For security findings, include observed impact, required conditions,
verification counts, limitations and remediation. For general analysis, state
what was established, how it was verified and what remains unknown. Finish
with `agent_finish`; the root owns the final assessment summary.
