# Worked example: root orchestration

A focused, working decomposition for a single challenge binary:

1. Read the threat model; if entrypoints are unknown, spawn `recon` with a narrow scope: "establish format, mitigations, input entrypoints and service behavior; no exploit work". Wait for it.
2. Hand the recon facts to one `reverse` agent ("trace input handling to the dangerous operation; report module-relative addresses") and one `hunter` ("derive one concrete hypothesis with preconditions").
3. Route verified hypotheses to `exploit` with the exact environment facts; require a real `job_start` run of the exploit against the target/service as proof.
4. Only accept `flag{...}`-style strings that came from an actual run's output, stored via `note_save` title "flag".
5. Reconcile every `needs_follow_up` coverage row or mark it inconclusive before `finish_scan`.
