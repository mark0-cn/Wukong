# Transport and recovery

Goal: keep working correctly when the worker transport, a job, or a tool call
misbehaves. Load when a runtime tool fails in a way static facts do not
explain.

## Read the error before acting

- `runtime_doctor` reports actual worker capabilities and paths. A capability
  error means the backend is fine but the operation is unsupported: adapt
  (the worker executes ELF only — PE/Mach-O targets are static-only), do not
  retry.
- A transport error on the very first call is usually environmental (backend
  down, container stopped); a failure mid-run is a state question. Diagnose
  the observed blocker, not a generic guess.

## The worker survives the transport

- Jobs, fuzz campaigns, debug sessions and the daemon keep running after the
  stdio client dies; a reconnect reuses the same state directory. A dropped
  transport is NOT lost work: reconnect and read `job_status`/`fuzz_status`
  instead of restarting jobs.
- A job is `lost` only when its supervisor process is provably gone.
  Uncertain observations stay active with a reconciliation note. Do not
  restart them and do not report them as finished.

## Never blindly repeat a mutating call

- A timed-out or failed mutating request (job start, send, cancel, fuzz start,
  file put) may or may not have happened. Do not re-send it unchanged —
  duplicated side effects are worse than the timeout. Record the request
  details and report to root; the operator can `reconcile` with a reviewed
  result.
- Read-only calls (status, list, evidence reads, doctor) are safe to retry
  once.

## Timeouts have different meanings

- Worker request timeout (`runtime.request_timeout_secs`): a transport
  deadline. The operation may still complete later — check status before
  retrying anything.
- Tool compute timeout (`angr_solve.timeout_secs`, fuzz per-exec timeouts):
  the solver/campaign itself is bounded. A dead-end is a result: adjust
  find/avoid addresses or loosen `printable_only`, do not treat it as a
  transport failure.
- A GDB/MI command that times out invalidates that debug session (its side
  effects are unknown): close the session and start a fresh one instead of
  continuing to drive it.

Finish with `agent_finish` describing the blocker, what was retried, what
remains uncertain, and the evidence IDs.
