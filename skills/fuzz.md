# Fuzz engineering

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
- Hand crash files to crash triage with their hashes and the campaign
  parameters. A generated crash is unclassified until independently
  reproduced.
- Record dynamic coverage separately from static analysis coverage: having
  analyzed a function is not having executed it.

## Stop discipline

Stop the campaign (`job_cancel` with the fuzz job_id) when the objective is met or the time budget
is spent. Finish with `agent_finish` naming the artifacts and their evidence
IDs.
