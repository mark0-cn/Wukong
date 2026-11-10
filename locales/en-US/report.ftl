report-title = Binary analysis report
report-run = Run
report-target = Target SHA256
report-status = Status
report-task = Task
report-summary = Summary
report-no-summary = No final summary recorded.
report-findings = Findings
report-no-findings = No findings recorded. This does not prove the target is free of vulnerabilities.
report-stage = Recorded stage
report-unconfirmed = impact_verified (recorded claim; independent verification is missing or incomplete)
report-root-cause = Root cause
report-impact = Reported impact
report-severity = Severity
report-severity-rationale = Severity rationale
report-replay = Independent replay
report-attempts = successful attempts
report-verified-impact = Verified impact
report-evidence = Evidence IDs
report-coverage = Coverage
report-scope = Scope
report-kind = Kind
report-outcome = Outcome
report-notes = Notes
report-accounting = Accounting
report-accounting-body = { $agents } agents; { $calls } tool calls; { $evidence } evidence records; { $tokens } tokens; USD { $cost }.
report-raw-guide = See report.json and replay.json for environment, conditions, exact tool arguments, and evidence references.
report-stage-suspected = suspected
report-stage-reproduced = reproduced
report-stage-root_cause_confirmed = root_cause_confirmed
report-stage-impact_verified = impact_verified
report-stage-inconclusive = inconclusive
report-stage-rejected = rejected
report-status-queued = queued
report-status-running = running
report-status-waiting = waiting
report-status-budget_paused = budget_paused
report-status-completed = completed
report-status-stopped = stopped
report-status-crashed = crashed
report-status-failed = failed
report-bundle-readme = wukong evidence bundle

    report.md / report.html: human-readable recorded findings and verification status.
    report.json: transactional run snapshot and original evidence index.
    replay.json: exact recorded tool calls, verification environment and conditions.
    config.redacted.json: settings with credential fields redacted.
    target/sample.bin: original target bytes (not executed during export).
    evidence/: deduplicated original evidence bytes, addressed by SHA256.
    manifest.json / SHA256SUMS: all payload digests. Use wukong verify-bundle.

    A checksum verifies integrity, not publisher identity. Original evidence is not redacted.
    Redacted configuration and event metadata are not a credential backup.
