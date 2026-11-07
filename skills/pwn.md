# Pwn workflow

Goal: turn a confirmed overflow into a verified control-flow hijack with a
minimal, reproducible payload. Load alongside `reverse` or `exploit`; keep the
total skill count small.

## Know the layout before writing bytes

- Buffer-to-saved-RIP offset: start from the reverse handoff, confirm at
  runtime. Generate a cyclic pattern (pwntools `cyclic(N)` in a `file_put`
  script), feed it via `job_start` or `crash_replay`, observe the faulting RIP
  under `debug_start`/`debug_command`, and recover the offset with
  `cyclic_find`. Cite both the static layout and the runtime observation.
- Non-PIE: use the link-time address from `function_decompile` directly.
  PIE/ASLR: leak a pointer or use partial overwrites — never guess bases.
- A global canary import does not mean THIS function is protected: check the
  actual prologue/epilogue of the vulnerable function before assuming no
  canary.

## The stack-alignment trap (glibc 2.35+, incl. Ubuntu 24.04)

Entering a function via a corrupted `ret` leaves rsp 8 bytes off the ABI's
call-entry alignment (a `call` would have pushed a return address). Modern
glibc's `system()` (posix_spawn path) and several libm/libc helpers execute
`movaps` against the stack and SIGSEGV on misalignment: the payload reaches
the function, prints its first output, then dies INSIDE libc. Fix: prepend a
single `ret` gadget before the target address (padding + ret_gadget +
target). Find gadgets with ropper (see tooling); one `ret` is enough to
realign. Confirm the fix with a real run, not by reasoning about the crash
site.

## Choose the win marker from the target's own output

The verification predicate must be an observation that ONLY the reached
function produces — decompile it and cite the exact string. A prompt printed
by the vulnerable function itself is not a win marker. Record the exact
expected stdout/stderr bytes.

## Verify with the real binary

- `angr_solve` (see tooling) produces a hypothesis input; replay it against
  the real target with `job_start`/`crash_replay` and cite BOTH the solver
  job and the replay job.
- "RIP points where I want" is not control-flow evidence and not code
  execution: only an actual run demonstrating the win marker counts.
- Hand the final payload to a separate `verifier` agent with the predicate.

Finish with `agent_finish` naming the payload bytes, offset, gadget/function
addresses, and the verification evidence IDs.
