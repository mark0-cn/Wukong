# Fuzz engineering

Use when the assigned question needs an input-generation campaign. This
capability owns campaign setup, artifacts and cleanup, not the agent lifecycle.

Goal: a bounded background fuzzing campaign whose crashes are real artifacts,
not claims.

## Setup

- Establish the input format first: `fuzz_start` needs the staged target and
  at least one seed file. Document argv/file/stdin mode, seed coverage, memory
  and per-exec time limits, and the environment in the call itself.
- Fuzzing is a background job: `fuzz_start` returns a `job_id` immediately.
  Poll `fuzz_status` occasionally and keep doing other work — never occupy a
  model turn waiting on it.

## Monitoring and triage handoff

- Report real `fuzzer_stats` (execs, coverage, stability) — not vibes.
- Save crash files with hashes and campaign parameters. Use `crash` in the same
  assignment when triage is included, or report the artifacts for separately
  scoped triage. A generated crash is unclassified until reproduced.
- Record dynamic coverage separately from static analysis coverage: having
  analyzed a function is not having executed it.

## Stop discipline

Stop the campaign (`job_cancel` with the fuzz job_id) when its objective is met
or the time budget is spent. Save artifacts and evidence IDs, then return to the
primary assignment; stopping a campaign does not finish a broader analysis task.
