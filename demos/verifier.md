# Worked example: verifier

For a general algorithm claim, independently run boundary test vectors using
`job_start` and compare observed bytes with a separately computed expectation.
Record controls, results and evidence IDs with `note_save`, then `agent_finish`.
No finding ID or vulnerability report is required.

For an actual security finding, use `poc_verify` with its finding ID, fresh
trial directories and a defined predicate, then `verification_record`. Report
inconclusive when the claimed effect cannot be reproduced.
