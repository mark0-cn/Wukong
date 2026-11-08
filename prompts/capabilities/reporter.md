# Evidence-backed reporting

Use to write or reconcile a report from existing analysis and verification
evidence. Root normally uses this capability directly; a separate analyst is
useful only for an explicitly delegated, substantial reporting artifact.
Do not start new target experiments to fill gaps in a writing assignment: list
the missing proof and request follow-up from the responsible analyst/verifier.

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
what was established, how it was verified and what remains unknown. Return the
report artifact and unresolved questions to the primary role's workflow. Root
retains final assessment ownership; this capability does not select a lifecycle tool.
